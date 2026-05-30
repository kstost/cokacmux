"""
Build executor for Rust projects with cross-compilation support.
"""
import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Tuple

from .config import BuildConfig
from .logger import Logger
from .targets import Target, TargetManager
from .tools import ToolInstaller


@dataclass
class BuildResult:
    """Result of a build operation."""

    target: Target
    success: bool
    binary_path: Optional[Path] = None
    error_message: Optional[str] = None


class BuildExecutor:
    """Executes Rust builds with cross-compilation support."""

    def __init__(
        self,
        config: BuildConfig,
        project_root: Path,
        tool_installer: ToolInstaller,
        target_manager: TargetManager,
        logger: Logger,
    ):
        self.config = config
        self.project_root = project_root
        self.tool_installer = tool_installer
        self.target_manager = target_manager
        self.logger = logger

        self.dist_dir = project_root / config.dist_dir
        self.target_dir = project_root / "target"

    def clean(self) -> bool:
        """Clean build artifacts."""
        self.logger.info("Cleaning build artifacts...")

        try:
            # Run cargo clean with proper environment
            env = self.tool_installer.get_env()
            cargo_path = self.tool_installer.get_cargo_path()
            cargo_cmd = str(cargo_path) if cargo_path else "cargo"
            result = subprocess.run(
                [cargo_cmd, "clean"],
                cwd=self.project_root,
                capture_output=True,
                text=True,
                env=env,
            )

            if result.returncode != 0:
                self.logger.warning(f"cargo clean failed: {result.stderr}")

            # Remove dist directory
            if self.dist_dir.exists():
                shutil.rmtree(self.dist_dir)
                self.logger.info(f"Removed {self.dist_dir}")

            self.logger.success("Clean complete")
            return True

        except Exception as e:
            self.logger.error(f"Clean failed: {e}")
            return False

    def build_target(self, target: Target) -> BuildResult:
        """Build for a specific target."""
        self.logger.info(f"Building for {target.friendly_name}...")
        cargo_path = self.tool_installer.get_cargo_path()
        cargo_cmd = str(cargo_path) if cargo_path else "cargo"

        # Determine build command
        if target.needs_xwin:
            cmd = [cargo_cmd, "xwin", "build"]
        elif target.needs_zigbuild:
            cmd = [cargo_cmd, "zigbuild"]
        elif target.needs_gnullvm and self.config.host_os == "windows":
            cmd = [cargo_cmd, f"+stable-{target.rust_target}", "build"]
        else:
            cmd = [cargo_cmd, "build"]

        # Add release flag
        if self.config.release:
            cmd.append("--release")

        # Add target (zigbuild Linux targets use .2.17 suffix for GLIBC compatibility)
        if target.needs_zigbuild and target.platform == "linux":
            cmd.extend(["--target", f"{target.rust_target}.2.17"])
        elif not (target.needs_gnullvm and self.config.host_os == "windows") and not target.is_native:
            cmd.extend(["--target", target.rust_target])

        # Get environment
        env = self.tool_installer.get_env()
        if target.needs_gnullvm and self.config.host_os == "windows":
            lib_dir = self.tool_installer.windows_import_lib_dir(target.rust_target).resolve()
            gnullvm_flags = f"-C linker=rust-lld -C target-feature=+crt-static -L native={lib_dir}"
            existing_flags = env.get("RUSTFLAGS", "").strip()
            if existing_flags:
                env["RUSTFLAGS"] = f"{existing_flags} {gnullvm_flags}"
            else:
                env["RUSTFLAGS"] = gnullvm_flags
            existing_host_flags = env.get("HOST_RUSTFLAGS", "").strip()
            if existing_host_flags:
                env["HOST_RUSTFLAGS"] = f"{existing_host_flags} {gnullvm_flags}"
            else:
                env["HOST_RUSTFLAGS"] = gnullvm_flags

            env["CARGO_TARGET_DIR"] = str(self.target_dir / target.rust_target)
            cc_wrapper = self.tool_installer.windows_cc_wrapper_path(target.rust_target)
            ar_wrapper = self.tool_installer.windows_ar_wrapper_path()
            if cc_wrapper and cc_wrapper.exists():
                env["CC"] = str(cc_wrapper)
            if ar_wrapper.exists():
                env["AR"] = str(ar_wrapper)

        # For Windows ARM64 cross-compilation, cargo-xwin passes /imsvc flags
        # (clang-cl syntax) via CFLAGS, but the ring crate uses plain clang
        # which doesn't understand /imsvc. A clang wrapper converts /imsvc to
        # -isystem so plain clang can process the MSVC include paths.
        clang_wrapper_dir = None
        if target.needs_xwin and "aarch64" in target.rust_target:
            clang_wrapper_dir = self._create_clang_wrapper()
            if clang_wrapper_dir:
                env["PATH"] = clang_wrapper_dir + os.pathsep + env.get("PATH", "")

        self.logger.debug(f"Running: {' '.join(cmd)}")

        try:
            result = subprocess.run(
                cmd,
                cwd=self.project_root,
                env=env,
                capture_output=True,
                text=True,
            )

            if result.returncode == 0:
                # Find the built binary
                binary_path = self._find_binary(target)
                self.logger.success(f"Built: {target.friendly_name}")

                return BuildResult(
                    target=target,
                    success=True,
                    binary_path=binary_path,
                )
            else:
                self.logger.error(f"Build failed for {target.friendly_name}")
                # Print stderr for debugging
                if result.stderr:
                    for line in result.stderr.split("\n")[:20]:
                        if line.strip():
                            self.logger.debug(f"  {line}")

                return BuildResult(
                    target=target,
                    success=False,
                    error_message=result.stderr,
                )

        except Exception as e:
            self.logger.error(f"Build failed: {e}")
            return BuildResult(
                target=target,
                success=False,
                error_message=str(e),
            )

    def _create_clang_wrapper(self) -> Optional[str]:
        """Create a clang wrapper that converts /imsvc to -isystem for plain clang."""
        try:
            wrapper_dir = os.path.join(tempfile.gettempdir(), "clang-xwin-wrapper")
            os.makedirs(wrapper_dir, exist_ok=True)
            wrapper_path = os.path.join(wrapper_dir, "clang")

            clang_path = shutil.which("clang")
            if not clang_path:
                return None

            wrapper_script = f"""#!/bin/bash
args=()
skip_next=false
for arg in "$@"; do
    if $skip_next; then
        args+=("-isystem" "$arg")
        skip_next=false
    elif [ "$arg" = "/imsvc" ]; then
        skip_next=true
    else
        args+=("$arg")
    fi
done
exec {clang_path} "${{args[@]}}"
"""
            with open(wrapper_path, "w") as f:
                f.write(wrapper_script)
            os.chmod(wrapper_path, stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
            return wrapper_dir
        except Exception:
            return None

    def _find_binary(self, target: Target) -> Optional[Path]:
        """Find the built binary."""
        profile = "release" if self.config.release else "debug"

        # Determine binary name (Windows targets produce .exe)
        binary_name = "cokacmux.exe" if target.platform == "windows" else "cokacmux"

        if target.is_native:
            binary_path = self.target_dir / profile / binary_name
        else:
            binary_path = self.target_dir / target.rust_target / profile / binary_name

        if binary_path.exists():
            return binary_path
        return None

    def copy_to_dist(self, results: List[BuildResult]) -> List[Tuple[Path, str]]:
        """Copy built binaries to dist directory."""
        self.dist_dir.mkdir(parents=True, exist_ok=True)

        copied: List[Tuple[Path, str]] = []

        for result in results:
            if not result.success or not result.binary_path:
                continue

            # Determine destination name (Windows binaries keep .exe extension)
            if result.target.platform == "windows":
                dest_name = f"cokacmux-{result.target.friendly_name}.exe"
            else:
                dest_name = f"cokacmux-{result.target.friendly_name}"
            dest_path = self.dist_dir / dest_name

            try:
                shutil.copy2(result.binary_path, dest_path)
                dest_path.chmod(0o755)
                checksum_path = dest_path.with_name(f"{dest_path.name}.sha256")
                digest = hashlib.sha256(dest_path.read_bytes()).hexdigest()
                checksum_path.write_text(f"{digest}  {dest_path.name}\n", encoding="utf-8")

                # Get file size
                size = dest_path.stat().st_size
                size_str = self._format_size(size)

                copied.append((dest_path, size_str))
                self.logger.debug(f"Copied {dest_path.name} ({size_str})")

            except Exception as e:
                self.logger.error(f"Failed to copy {result.binary_path}: {e}")

        return copied

    def _format_size(self, size: int) -> str:
        """Format file size in human-readable format."""
        for unit in ["B", "KB", "MB", "GB"]:
            if size < 1024:
                return f"{size:.1f}{unit}"
            size /= 1024
        return f"{size:.1f}TB"

    def build_all(self, targets: List[Target]) -> List[BuildResult]:
        """Build all specified targets."""
        results: List[BuildResult] = []

        # Ensure all targets are installed
        if not self.target_manager.ensure_targets(targets):
            self.logger.warning("Some targets could not be installed")

        # Windows GNU/LLVM builds need generated GNU import archives and Zig cc
        # for bundled SQLite's C build scripts.
        needs_gnullvm = any(
            t.needs_gnullvm and self.config.host_os == "windows"
            for t in targets
        )
        if needs_gnullvm:
            for target in targets:
                if target.needs_gnullvm and self.config.host_os == "windows":
                    if not self.tool_installer.install_windows_import_libs(target.rust_target):
                        return []

        # Check if we need cross-compilation tools
        needs_zigbuild = any(t.needs_zigbuild for t in targets)
        if needs_zigbuild:
            if not self.tool_installer.is_zig_installed():
                self.logger.error(
                    "Zig is required for cross-compilation. Run with --setup first."
                )
                return []

            if not self.tool_installer.is_cargo_zigbuild_installed():
                self.logger.error(
                    "cargo-zigbuild is required for cross-compilation. Run with --setup first."
                )
                return []

        # Check if we need Windows cross-compilation tools
        needs_xwin = any(t.needs_xwin for t in targets)
        if needs_xwin:
            if not self.tool_installer.is_cargo_xwin_installed():
                self.logger.error(
                    "cargo-xwin is required for Windows cross-compilation. Run with --setup-windows first."
                )
                return []

            if not self.tool_installer.is_clang_installed():
                self.logger.error(
                    "clang is required for Windows cross-compilation. Install with: apt install clang"
                )
                return []

            if not self.tool_installer.is_lld_installed():
                self.logger.error(
                    "lld is required for Windows cross-compilation. Install with: apt install lld"
                )
                return []

            if not self.tool_installer.is_llvm_lib_installed():
                self.logger.error(
                    "llvm-lib is required for Windows cross-compilation. Install with: apt install llvm"
                )
                return []

            if not self.tool_installer.is_clang_cl_installed():
                self.logger.error(
                    "clang-cl is required for Windows ARM64 cross-compilation. Install with: apt install clang-tools-18"
                )
                return []

            self.logger.info(
                "Note: cargo-xwin will download MSVC CRT/SDK on first build (requires internet)"
            )

        # Build each target
        total = len(targets)
        for i, target in enumerate(targets, 1):
            self.logger.step(i, total, f"Building {target.friendly_name}")
            result = self.build_target(target)
            results.append(result)

        return results


def run_build(
    config: BuildConfig,
    project_root: Path,
    targets: List[str],
    logger: Logger,
) -> bool:
    """
    Main entry point for running builds.

    Args:
        config: Build configuration
        project_root: Path to project root
        targets: List of target specifications
        logger: Logger instance

    Returns:
        True if all builds succeeded
    """
    tool_installer = ToolInstaller(config, project_root, logger)
    # Pass environment to target manager so rustup uses correct paths
    target_manager = TargetManager(config, logger, env=tool_installer.get_env())
    executor = BuildExecutor(
        config, project_root, tool_installer, target_manager, logger
    )

    # Clean if requested
    if config.clean:
        executor.clean()

    # Resolve targets
    resolved_targets = target_manager.resolve_targets(targets)

    if not resolved_targets:
        logger.error("No valid targets specified")
        return False

    logger.info(f"Building for {len(resolved_targets)} target(s):")
    for target in resolved_targets:
        logger.target(target.friendly_name, target.rust_target)
    logger.newline()

    # Check if cross-compilation setup is needed (zigbuild for macOS/Linux)
    needs_zigbuild_setup = any(t.needs_zigbuild for t in resolved_targets)
    needs_macos = any(t.platform == "macos" for t in resolved_targets)
    if needs_zigbuild_setup:
        missing_zig = not tool_installer.is_zig_installed()
        missing_zigbuild = not tool_installer.is_cargo_zigbuild_installed()
        missing_sdk = needs_macos and not tool_installer.is_macos_sdk_installed()
        if missing_zig or missing_zigbuild or missing_sdk:
            logger.header("Cross-compilation Setup Required")
            if missing_sdk:
                if not tool_installer.setup_cross_compile():
                    return False
            else:
                success = True
                if missing_zig and not tool_installer.install_zig():
                    success = False
                if missing_zigbuild and not tool_installer.install_cargo_zigbuild():
                    success = False
                if not success:
                    return False
            logger.newline()

    # Check if Windows cross-compilation setup is needed
    needs_xwin_setup = any(t.needs_xwin for t in resolved_targets)
    if needs_xwin_setup:
        if not tool_installer.is_cargo_xwin_installed() or not tool_installer.is_clang_installed():
            logger.header("Windows Cross-compilation Setup Required")
            if not tool_installer.setup_windows_cross():
                return False
            logger.newline()

    # Build all targets
    results = executor.build_all(resolved_targets)

    # Copy to dist
    if any(r.success for r in results):
        copied = executor.copy_to_dist(results)
        logger.results(copied)

    # Return success if all builds passed
    return all(r.success for r in results)
