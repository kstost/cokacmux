"""
Build executor for Rust projects with cross-compilation support.
"""
import os
import re
import shlex
import shutil
import stat
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Tuple

from .config import BuildConfig, RUST_TARGETS, RUST_VERSION
from .logger import Logger
from .targets import Target, TargetManager
from .tools import ToolInstaller


def effective_auto_setup(config: BuildConfig, requested: bool) -> bool:
    """Resolve auto-setup policy, including the legacy build.py behavior."""
    if config.allow_release_auto_setup:
        return bool(requested)
    ci_build = os.environ.get("CI", "").strip().lower() in {
        "1",
        "true",
        "yes",
        "on",
    }
    return bool(requested and not config.release and not ci_build)


_ARTIFACT_COMPONENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_UNSAFE_BUILD_ENV_KEYS = {
    "AR",
    "CC",
    "CFLAGS",
    "CPPFLAGS",
    "CXX",
    "CXXFLAGS",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTDOC",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "LD",
    "LDFLAGS",
    "MACOSX_DEPLOYMENT_TARGET",
    "RUSTC",
    "RUSTC_BOOTSTRAP",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC_WRAPPER",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
    "SDKROOT",
}


def _rust_pin_error(config: BuildConfig) -> Optional[str]:
    """Return an error when a caller tries to bypass the release Rust pin."""
    if config.rust_version == RUST_VERSION:
        return None
    return (
        f"Configured Rust {config.rust_version!r} does not match the pinned "
        f"release toolchain {RUST_VERSION!r}."
    )


def _unsafe_build_environment_error() -> Optional[str]:
    """Reject inherited compiler/linker overrides in non-interactive builds."""
    unsafe: List[str] = []
    for raw_key, value in os.environ.items():
        if not value:
            continue
        key = raw_key.upper()
        target_override = key.startswith("CARGO_TARGET_") and key.endswith(
            ("_LINKER", "_RUNNER", "_RUSTFLAGS")
        )
        cc_override = key.startswith(
            ("AR_", "CC_", "CFLAGS_", "CXX_", "CXXFLAGS_")
        ) or key.endswith(("_AR", "_CC", "_CFLAGS", "_CXX", "_CXXFLAGS"))
        if (
            key in _UNSAFE_BUILD_ENV_KEYS
            or key.startswith("CARGO_PROFILE_")
            or target_override
            or cc_override
        ):
            unsafe.append(raw_key)
    if not unsafe:
        return None
    return (
        "Non-interactive builds refuse inherited compiler/linker overrides: "
        + ", ".join(sorted(unsafe, key=str.upper))
    )


def _normalize_target(target: Target, config: BuildConfig) -> Target:
    """Recompute security-relevant target flags from the Rust triple."""
    normalized = Target.from_rust_target(target.rust_target, config)
    normalized.friendly_name = target.friendly_name
    return normalized


def _target_name_is_safe(target: Target) -> bool:
    """Return whether a target name is one safe distribution component."""
    return bool(_ARTIFACT_COMPONENT.fullmatch(target.friendly_name))


def _target_is_supported(target: Target) -> bool:
    """Return whether the Rust triple is part of the release allowlist."""
    return target.rust_target in RUST_TARGETS.values()


def pinned_tool_receipt_error(
    config: BuildConfig,
    target: Target,
    auto_setup: bool,
) -> Optional[str]:
    """Return the fail-closed error for an unreceipted cross-build path."""
    if not _target_is_supported(target):
        return f"Unsupported release target: {target.rust_target}"
    target = _normalize_target(target, config)
    if auto_setup:
        return None
    if target.needs_zigbuild:
        return (
            "Reproducible Zig/cargo-zigbuild inputs are not configured. "
            "Release, CI, and --no-auto-setup cross-builds are blocked "
            "until a pinned tool receipt is available."
        )
    if target.needs_gnullvm:
        return (
            "Reproducible Windows GNU/LLVM inputs are not configured. "
            "Release, CI, and --no-auto-setup GNU/LLVM builds are blocked "
            "until a pinned tool receipt is available."
        )
    if target.needs_xwin:
        return (
            "Reproducible cargo-xwin SDK/CRT inputs are not configured. "
            "Release, CI, and --no-auto-setup Windows cross-builds are "
            "blocked until a pinned cache receipt is available."
        )
    if not target.is_native:
        return (
            f"Reproducible inputs for non-native target {target.rust_target} "
            "are not configured. Release, CI, and --no-auto-setup builds are "
            "blocked until a pinned tool receipt is available."
        )
    return None


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

    def _cargo_config_error(self) -> Optional[str]:
        """Reject mutable Cargo config files outside the release contract."""
        candidates: List[Path] = []
        current = self.project_root.resolve()
        while True:
            candidates.extend(
                (current / ".cargo" / "config.toml", current / ".cargo" / "config")
            )
            if current.parent == current:
                break
            current = current.parent

        cargo_home = Path(self.tool_installer.cargo_home)
        candidates.extend((cargo_home / "config.toml", cargo_home / "config"))
        present = [
            path
            for path in candidates
            if path.exists() or path.is_symlink()
        ]
        if not present:
            return None
        return (
            "Non-interactive builds refuse unreceipted Cargo config files: "
            + ", ".join(str(path) for path in present)
        )

    def _rustup_cargo_command(
        self,
        *args: str,
        require_pinned: bool,
    ) -> Optional[List[str]]:
        """Build a Cargo command through ToolInstaller's selected rustup."""
        pin_error = _rust_pin_error(self.config)
        if pin_error:
            self.logger.error(pin_error)
            return None
        if require_pinned:
            environment_error = _unsafe_build_environment_error()
            if environment_error:
                self.logger.error(environment_error)
                return None
            cargo_config_error = self._cargo_config_error()
            if cargo_config_error:
                self.logger.error(cargo_config_error)
                return None
        rustup_path = (
            self.tool_installer.get_pinned_rustup_path()
            if require_pinned
            else self.tool_installer.get_rustup_path()
        )
        if not rustup_path:
            self.logger.error(
                "Pinned project-local rustup is unavailable. "
                "Run --setup-rust first."
                if require_pinned
                else "rustup not found. Please run --setup-rust first."
            )
            return None
        if require_pinned and not self.tool_installer.verify_release_toolchain():
            self.logger.error(
                f"Pinned Rust toolchain {self.config.rust_version} failed identity verification"
            )
            return None
        return [
            str(rustup_path),
            "run",
            self.config.rust_version,
            "cargo",
            *args,
        ]

    def _pinned_tool_receipt_error(
        self,
        target: Target,
        auto_setup: bool,
    ) -> Optional[str]:
        """Return the fail-closed error for unreceipted cross-build tools."""
        return pinned_tool_receipt_error(self.config, target, auto_setup)

    def clean(self, auto_setup: bool = True) -> bool:
        """Clean build artifacts."""
        self.logger.info("Cleaning build artifacts...")
        auto_setup = effective_auto_setup(self.config, auto_setup)

        try:
            # Cargo must be selected by the same verified rustup installation
            # that owns the exact configured toolchain.
            cmd = self._rustup_cargo_command(
                "clean",
                "--locked",
                require_pinned=not auto_setup,
            )
            if cmd is None:
                return False
            env = dict(self.tool_installer.get_env())
            if not auto_setup:
                cmd.append("--offline")
                env["CARGO_NET_OFFLINE"] = "true"
            result = subprocess.run(
                cmd,
                cwd=self.project_root,
                capture_output=True,
                text=True,
                env=env,
            )

            cargo_cleaned = result.returncode == 0
            if not cargo_cleaned:
                self.logger.warning(f"cargo clean failed: {result.stderr}")

            # Remove dist directory
            if self.dist_dir.exists():
                shutil.rmtree(self.dist_dir)
                self.logger.info(f"Removed {self.dist_dir}")

            if cargo_cleaned:
                self.logger.success("Clean complete")
            return cargo_cleaned

        except Exception as e:
            self.logger.error(f"Clean failed: {e}")
            return False

    def build_target(
        self,
        target: Target,
        auto_setup: bool = True,
    ) -> BuildResult:
        """Build for a specific target."""
        if not _target_name_is_safe(target):
            message = f"Unsafe target artifact name: {target.friendly_name!r}"
            self.logger.error(message)
            return BuildResult(target=target, success=False, error_message=message)
        if not _target_is_supported(target):
            message = f"Unsupported release target: {target.rust_target}"
            self.logger.error(message)
            return BuildResult(target=target, success=False, error_message=message)
        target = _normalize_target(target, self.config)
        self.logger.info(f"Building for {target.friendly_name}...")
        auto_setup = effective_auto_setup(self.config, auto_setup)
        receipt_error = self._pinned_tool_receipt_error(target, auto_setup)
        if receipt_error:
            self.logger.error(receipt_error)
            return BuildResult(
                target=target,
                success=False,
                error_message=receipt_error,
            )

        # Determine build command
        if target.needs_xwin:
            cargo_args = ["xwin", "build"]
        elif target.needs_zigbuild:
            cargo_args = ["zigbuild"]
        else:
            cargo_args = ["build"]

        cmd = self._rustup_cargo_command(
            *cargo_args,
            require_pinned=not auto_setup,
        )
        if cmd is None:
            message = "The verified rustup executable is unavailable"
            return BuildResult(
                target=target,
                success=False,
                error_message=message,
            )

        # Dependency resolution must match the committed lockfile.
        cmd.append("--locked")
        if not auto_setup:
            cmd.append("--offline")

        # Add release flag
        if self.config.release:
            cmd.append("--release")

        # Add target (zigbuild Linux targets use .2.17 suffix for GLIBC compatibility)
        if target.needs_zigbuild and target.platform == "linux":
            cmd.extend(["--target", f"{target.rust_target}.2.17"])
        elif not target.is_native:
            cmd.extend(["--target", target.rust_target])

        # Get environment
        env = dict(
            self.tool_installer.get_env(
                include_zig=target.needs_zigbuild or target.needs_gnullvm,
                include_macos_sdk=(
                    target.needs_zigbuild and target.platform == "macos"
                ),
            )
        )
        if not auto_setup:
            env["CARGO_NET_OFFLINE"] = "true"

        xwin_clang_cl_link = None
        if target.needs_xwin:
            try:
                xwin_clang_cl_link = self._prepare_xwin_clang_cl_cache(env)
            except OSError as exc:
                message = f"Failed to prepare cargo-xwin compiler cache: {exc}"
                self.logger.error(message)
                return BuildResult(
                    target=target,
                    success=False,
                    error_message=message,
                )

        # For Windows ARM64 cross-compilation, cargo-xwin passes /imsvc flags
        # (clang-cl syntax) via CFLAGS, but the ring crate uses plain clang
        # which doesn't understand /imsvc. A clang wrapper converts /imsvc to
        # -isystem so plain clang can process the MSVC include paths.
        clang_wrapper_dir = None
        if target.needs_xwin and "aarch64" in target.rust_target:
            clang_wrapper_dir = self._create_clang_wrapper()
            if not clang_wrapper_dir:
                message = "Failed to create the clang wrapper required for Windows ARM64"
                self.logger.error(message)
                return BuildResult(
                    target=target,
                    success=False,
                    error_message=message,
                )
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
                if not binary_path:
                    message = (
                        f"Build command succeeded but the {target.friendly_name} "
                        "binary was not found"
                    )
                    self.logger.error(message)
                    return BuildResult(
                        target=target,
                        success=False,
                        error_message=message,
                    )
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
        finally:
            if clang_wrapper_dir:
                if xwin_clang_cl_link:
                    self._remove_owned_xwin_clang_cl_link(
                        xwin_clang_cl_link,
                        Path(clang_wrapper_dir) / "clang",
                    )
                shutil.rmtree(clang_wrapper_dir, ignore_errors=True)

    def _prepare_xwin_clang_cl_cache(self, env: dict) -> Optional[Path]:
        """Remove only cargo-xwin's dangling clang-cl cache link.

        cargo-xwin 0.23.0 checks ``Path::exists`` before replacing this link.
        That check follows symlinks, so a link left behind by our temporary
        ARM64 clang wrapper is seen as absent and the subsequent symlink call
        fails with EEXIST. Pin the cache directory explicitly and remove only
        a link whose target is already absent; regular files and live links
        are never touched.
        """
        raw_cache_dir = env.get("XWIN_CACHE_DIR")
        if raw_cache_dir:
            cache_dir = Path(raw_cache_dir).expanduser()
        elif env.get("XDG_CACHE_HOME"):
            cache_dir = Path(env["XDG_CACHE_HOME"]).expanduser() / "cargo-xwin"
        elif env.get("HOME"):
            cache_dir = Path(env["HOME"]).expanduser() / ".cache" / "cargo-xwin"
        else:
            return None

        cache_dir = cache_dir.resolve(strict=False)
        env["XWIN_CACHE_DIR"] = str(cache_dir)
        link = cache_dir / ("clang-cl.exe" if os.name == "nt" else "clang-cl")
        if link.is_symlink() and not link.exists():
            link.unlink()
            self.logger.debug(f"Removed dangling cargo-xwin link: {link}")
        return link

    def _remove_owned_xwin_clang_cl_link(
        self,
        link: Path,
        wrapper: Path,
    ) -> None:
        """Remove a cargo-xwin link only when it targets our exact wrapper."""
        if not link.is_symlink():
            return
        try:
            target = Path(os.readlink(link))
            if not target.is_absolute():
                target = link.parent / target
            if target.resolve(strict=False) != wrapper.resolve(strict=False):
                return
            link.unlink()
            self.logger.debug(f"Removed cargo-xwin wrapper link: {link}")
        except OSError as exc:
            self.logger.debug(f"Could not remove cargo-xwin wrapper link {link}: {exc}")

    def _create_clang_wrapper(self) -> Optional[str]:
        """Create a clang wrapper that converts /imsvc to -isystem for plain clang."""
        clang_path = shutil.which("clang")
        if not clang_path:
            return None

        wrapper_dir: Optional[str] = None
        try:
            wrapper_dir = tempfile.mkdtemp(prefix="cokacmux-clang-xwin-")
            wrapper_path = os.path.join(wrapper_dir, "clang")

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
exec {shlex.quote(clang_path)} "${{args[@]}}"
"""
            with open(wrapper_path, "x", encoding="utf-8") as f:
                f.write(wrapper_script)
            os.chmod(wrapper_path, stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP | stat.S_IROTH | stat.S_IXOTH)
            return wrapper_dir
        except Exception:
            if wrapper_dir:
                shutil.rmtree(wrapper_dir, ignore_errors=True)
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
        """Publish built binaries as a single rollback-capable batch."""
        self.dist_dir.mkdir(parents=True, exist_ok=True)
        prepared: List[Tuple[Path, Path, str]] = []
        destinations: set[Path] = set()

        try:
            for result in results:
                if not result.success or not result.binary_path:
                    continue

                if not _target_name_is_safe(result.target):
                    raise ValueError(
                        f"Unsafe target artifact name: {result.target.friendly_name!r}"
                    )
                if not _target_is_supported(result.target):
                    raise ValueError(
                        f"Unsupported release target: {result.target.rust_target}"
                    )

                if result.target.platform == "windows":
                    dest_name = f"cokacmux-{result.target.friendly_name}.exe"
                else:
                    dest_name = f"cokacmux-{result.target.friendly_name}"
                dest_path = self.dist_dir / dest_name
                if dest_path in destinations:
                    raise ValueError(f"Multiple targets produce {dest_name}")
                destinations.add(dest_path)
                if dest_path.exists() and not dest_path.is_file():
                    raise OSError(f"Distribution destination is not a file: {dest_path}")

                fd, temp_name = tempfile.mkstemp(
                    prefix=f".{dest_path.name}.",
                    suffix=".tmp",
                    dir=self.dist_dir,
                )
                os.close(fd)
                temp_path = Path(temp_name)
                try:
                    shutil.copy2(result.binary_path, temp_path)
                    temp_path.chmod(0o755)
                    prepared.append(
                        (temp_path, dest_path, self._format_size(temp_path.stat().st_size))
                    )
                except Exception:
                    temp_path.unlink(missing_ok=True)
                    raise
        except Exception as e:
            self.logger.error(f"Failed to stage distribution binaries: {e}")
            for temp_path, _, _ in prepared:
                temp_path.unlink(missing_ok=True)
            return []

        # Move old files aside so a failed commit can restore the complete
        # previous distribution rather than leaving mixed release versions.
        changes: List[Tuple[Path, Optional[Path], bool]] = []
        try:
            for temp_path, dest_path, _ in prepared:
                backup_path: Optional[Path] = None
                if dest_path.exists() or dest_path.is_symlink():
                    fd, backup_name = tempfile.mkstemp(
                        prefix=f".{dest_path.name}.",
                        suffix=".backup",
                        dir=self.dist_dir,
                    )
                    os.close(fd)
                    backup_path = Path(backup_name)
                    backup_path.unlink()
                    os.replace(dest_path, backup_path)

                changes.append((dest_path, backup_path, False))
                os.replace(temp_path, dest_path)
                changes[-1] = (dest_path, backup_path, True)
        except Exception as e:
            self.logger.error(f"Failed to publish distribution binaries: {e}")
            for dest_path, backup_path, installed in reversed(changes):
                try:
                    if installed:
                        dest_path.unlink(missing_ok=True)
                    if backup_path:
                        os.replace(backup_path, dest_path)
                except Exception as rollback_error:
                    self.logger.error(
                        f"Failed to restore {dest_path} from {backup_path}: "
                        f"{rollback_error}"
                    )
            return []
        finally:
            for temp_path, _, _ in prepared:
                temp_path.unlink(missing_ok=True)

        copied: List[Tuple[Path, str]] = []
        for (_, dest_path, size_str), (_, backup_path, _) in zip(prepared, changes):
            if backup_path:
                try:
                    backup_path.unlink(missing_ok=True)
                except OSError as e:
                    self.logger.warning(f"Could not remove backup {backup_path}: {e}")
            copied.append((dest_path, size_str))
            self.logger.debug(f"Copied {dest_path.name} ({size_str})")
        return copied

    def _format_size(self, size: int) -> str:
        """Format file size in human-readable format."""
        for unit in ["B", "KB", "MB", "GB"]:
            if size < 1024:
                return f"{size:.1f}{unit}"
            size /= 1024
        return f"{size:.1f}TB"

    def build_all(
        self,
        targets: List[Target],
        auto_setup: bool = True,
    ) -> List[BuildResult]:
        """Build all specified targets."""
        results: List[BuildResult] = []
        auto_setup = effective_auto_setup(self.config, auto_setup)

        pin_error = _rust_pin_error(self.config)
        if pin_error:
            self.logger.error(pin_error)
            return []

        normalized_targets: List[Target] = []
        for target in targets:
            if not _target_name_is_safe(target):
                self.logger.error(
                    f"Unsafe target artifact name: {target.friendly_name!r}"
                )
                return []
            if not _target_is_supported(target):
                self.logger.error(f"Unsupported release target: {target.rust_target}")
                return []
            normalized_targets.append(_normalize_target(target, self.config))
        targets = normalized_targets

        # Receipt policy precedes even read-only target/tool probes.  PATH
        # executables are themselves untrusted inputs until this gate passes.
        for target in targets:
            receipt_error = self._pinned_tool_receipt_error(target, auto_setup)
            if receipt_error:
                self.logger.error(receipt_error)
                return []

        if not auto_setup:
            environment_error = _unsafe_build_environment_error()
            if environment_error:
                self.logger.error(environment_error)
                return []
            cargo_config_error = self._cargo_config_error()
            if cargo_config_error:
                self.logger.error(cargo_config_error)
                return []

        rustup_path = (
            self.tool_installer.get_pinned_rustup_path()
            if not auto_setup
            else self.tool_installer.get_rustup_path()
        )
        if not rustup_path:
            self.logger.error(
                "Pinned project-local rustup is unavailable. Run --setup-rust first."
                if not auto_setup
                else "rustup not found. Run --setup-rust first."
            )
            return []
        self.target_manager.env = self.tool_installer.get_env()
        self.target_manager.rustup_path = Path(rustup_path)

        # Ensure all targets are installed
        if not self.target_manager.ensure_targets(
            targets, install_missing=auto_setup
        ):
            self.logger.error("Some required Rust targets could not be prepared")
            return []

        # Windows GNU/LLVM builds need generated GNU import archives and Zig cc
        # for bundled SQLite's C build scripts.
        needs_gnullvm = any(
            t.needs_gnullvm and self.config.host_os == "windows"
            for t in targets
        )
        if needs_gnullvm:
            for target in targets:
                if target.needs_gnullvm and self.config.host_os == "windows":
                    if not auto_setup:
                        if not self.tool_installer.is_windows_import_libs_installed(
                            target.rust_target
                        ):
                            self.logger.error(
                                f"Windows import libraries for {target.rust_target} "
                                "are missing. Run --setup-windows first."
                            )
                            return []
                        continue
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
            if not auto_setup:
                self.logger.error(
                    "Reproducible cargo-xwin SDK/CRT inputs are not configured. "
                    "Release, CI, and --no-auto-setup Windows cross-builds are "
                    "blocked until a pinned cache receipt is available."
                )
                return []

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
                "Debug setup may let cargo-xwin download MSVC CRT/SDK inputs."
            )

        # Build each target
        total = len(targets)
        for i, target in enumerate(targets, 1):
            self.logger.step(i, total, f"Building {target.friendly_name}")
            result = self.build_target(target, auto_setup=auto_setup)
            results.append(result)

        return results


def run_build(
    config: BuildConfig,
    project_root: Path,
    targets: List[str],
    logger: Logger,
    auto_setup: bool = True,
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
    # This boundary is also used outside build.py. Recompute the policy here,
    # while allowing build.py to opt into its legacy automatic setup behavior.
    auto_setup = effective_auto_setup(config, auto_setup)

    pin_error = _rust_pin_error(config)
    if pin_error:
        logger.error(pin_error)
        return False

    tool_installer = ToolInstaller(config, project_root, logger)
    # Target resolution itself must not need a tool environment.  In
    # particular, constructing a Rust-only environment must not probe Zig.
    target_manager = TargetManager(config, logger)
    executor = BuildExecutor(
        config, project_root, tool_installer, target_manager, logger
    )

    # Resolve targets
    resolved_targets = target_manager.resolve_targets(
        targets,
        allow_system_probe=auto_setup,
    )

    if not resolved_targets:
        logger.error("No valid targets specified")
        return False

    # Block unreceipted cross paths before clean, rustup target queries, or
    # any PATH-based tool availability/version probe.
    for target in resolved_targets:
        receipt_error = pinned_tool_receipt_error(config, target, auto_setup)
        if receipt_error:
            logger.error(receipt_error)
            return False

    if not auto_setup:
        environment_error = _unsafe_build_environment_error()
        if environment_error:
            logger.error(environment_error)
            return False

    rustup_path = (
        tool_installer.get_pinned_rustup_path()
        if not auto_setup
        else tool_installer.get_rustup_path()
    )
    if not rustup_path:
        logger.error(
            "Pinned project-local rustup is unavailable. Run --setup-rust first."
            if not auto_setup
            else "rustup not found. Run --setup-rust first."
        )
        return False
    target_manager.env = tool_installer.get_env()
    target_manager.rustup_path = Path(rustup_path)

    # Clean only after all non-mutating input gates have passed.
    if config.clean and not executor.clean(auto_setup=auto_setup):
        return False

    logger.info(f"Building for {len(resolved_targets)} target(s):")
    for target in resolved_targets:
        logger.target(target.friendly_name, target.rust_target)
    logger.newline()

    # Check if cross-compilation setup is needed (zigbuild for macOS/Linux)
    needs_zigbuild_setup = any(t.needs_zigbuild for t in resolved_targets)
    needs_macos = any(
        t.needs_zigbuild and t.platform == "macos" for t in resolved_targets
    )
    if needs_zigbuild_setup:
        missing_zig = not tool_installer.is_zig_installed()
        missing_zigbuild = not tool_installer.is_cargo_zigbuild_installed()
        missing_sdk = needs_macos and not tool_installer.is_macos_sdk_installed()
        if missing_zig or missing_zigbuild or missing_sdk:
            logger.header("Cross-compilation Setup Required")
            if not auto_setup:
                logger.error(
                    "Cross-compilation tools are missing. "
                    "Run --setup-cross first; automatic setup is available "
                    "only for local debug builds."
                )
                return False
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
        windows_tools_ready = (
            tool_installer.is_cargo_xwin_installed()
            and tool_installer.is_clang_installed()
            and tool_installer.is_lld_installed()
            and tool_installer.is_llvm_lib_installed()
            and tool_installer.is_clang_cl_installed()
        )
        if not windows_tools_ready:
            logger.header("Windows Cross-compilation Setup Required")
            if not auto_setup:
                logger.error(
                    "Windows cross-compilation tools are missing. "
                    "Run --setup-windows first; automatic setup is available "
                    "only for local debug builds."
                )
                return False
            if not tool_installer.setup_windows_cross():
                return False
            logger.newline()

    # Build all targets
    results = executor.build_all(resolved_targets, auto_setup=auto_setup)
    build_success = (
        len(results) == len(resolved_targets)
        and bool(results)
        and all(r.success for r in results)
    )

    # Never publish a subset of a multi-target release.  Keeping the previous
    # complete distribution is safer than mixing binaries from two versions.
    copied = []
    if build_success:
        copied = executor.copy_to_dist(results)
        if len(copied) == len(resolved_targets):
            logger.results(copied)
        else:
            logger.error("Builds succeeded, but the distribution was not published")

    copy_success = build_success and len(copied) == len(resolved_targets)
    return build_success and copy_success
