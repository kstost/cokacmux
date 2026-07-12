"""
Rust target management for cross-compilation.
"""
import os
import shutil
import subprocess
from dataclasses import dataclass
from typing import List, Set, Optional, Dict
from pathlib import Path

from .config import (
    BuildConfig,
    RUST_TARGETS,
    RUSTUP_DIST_SERVER,
    RUSTUP_UPDATE_ROOT,
    TARGET_NAMES,
)
from .logger import Logger


@dataclass
class Target:
    """Represents a build target."""

    rust_target: str  # e.g., "aarch64-apple-darwin"
    friendly_name: str  # e.g., "macos-aarch64"
    platform: str  # "macos", "linux", or "windows"
    arch: str  # "aarch64" or "x86_64"
    needs_zigbuild: bool = False  # True for cross-platform macOS builds
    needs_xwin: bool = False  # True for Windows MSVC cross-compilation
    needs_gnullvm: bool = False  # True for Windows GNU/LLVM builds on Windows
    is_native: bool = False  # True if this is the native target

    @classmethod
    def from_rust_target(cls, rust_target: str, config: BuildConfig) -> "Target":
        """Create a Target from a Rust target triple."""
        friendly_name = TARGET_NAMES.get(rust_target, rust_target)

        # Parse platform and arch
        if "apple-darwin" in rust_target:
            platform = "macos"
        elif "linux" in rust_target:
            platform = "linux"
        elif "windows" in rust_target:
            platform = "windows"
        else:
            platform = "unknown"

        if "aarch64" in rust_target:
            arch = "aarch64"
        elif "x86_64" in rust_target:
            arch = "x86_64"
        else:
            arch = "unknown"

        # Determine if zigbuild is needed (not for Windows targets).
        # 1. macOS targets when building on a non-macOS host.
        # 2. All Linux targets, on every host, both for cross-linking and to
        #    pin GLIBC for broad compatibility.
        needs_zigbuild = (
            platform != "windows" and (
                (platform == "macos" and config.host_os != "macos") or
                platform == "linux"
            )
        )

        needs_gnullvm = rust_target.endswith("pc-windows-gnullvm")

        # Determine if cargo-xwin is needed (Windows MSVC cross-compilation)
        # Not needed when building on Windows natively
        needs_xwin = (
            platform == "windows" and
            config.host_os != "windows" and
            not needs_gnullvm
        )

        # Check if native (zigbuild targets are not native since --target is passed explicitly)
        is_native = (
            (platform == config.host_os) and
            (arch == config.host_arch) and
            not needs_zigbuild and
            not needs_gnullvm
        )

        return cls(
            rust_target=rust_target,
            friendly_name=friendly_name,
            platform=platform,
            arch=arch,
            needs_zigbuild=needs_zigbuild,
            needs_xwin=needs_xwin,
            needs_gnullvm=needs_gnullvm,
            is_native=is_native,
        )


class TargetManager:
    """Manages Rust targets and rustup operations."""

    def __init__(
        self,
        config: BuildConfig,
        logger: Logger,
        env: Optional[Dict[str, str]] = None,
        rustup_path: Optional[Path] = None,
    ):
        self.config = config
        self.logger = logger
        self.env = dict(env) if env is not None else os.environ.copy()
        # Read-only rustup queries must never provision a repository override,
        # including when TargetManager is used directly outside build.py.
        self.env["RUSTUP_AUTO_INSTALL"] = "0"
        self.env["RUSTUP_DIST_SERVER"] = RUSTUP_DIST_SERVER
        self.env["RUSTUP_UPDATE_ROOT"] = RUSTUP_UPDATE_ROOT
        self.env.pop("RUSTUP_TOOLCHAIN", None)
        self.env.pop("RUSTUP_OVERRIDE_HOST_TRIPLE", None)
        self.rustup_path = rustup_path
        self._installed_targets: Optional[Set[str]] = None
        self._installed_toolchains: Optional[Set[str]] = None

    def _path_value(self) -> Optional[str]:
        """Return PATH from the configured environment."""
        if not self.env:
            return None
        for key, value in self.env.items():
            if key.upper() == "PATH":
                return value
        return None

    def _rustup_command(self) -> str:
        """Resolve rustup to an absolute path when possible."""
        if self.rustup_path is not None:
            return str(self.rustup_path)
        path_value = self._path_value()
        for name in ("rustup.exe", "rustup"):
            resolved = shutil.which(name, path=path_value)
            if resolved:
                return resolved
        return "rustup"

    def get_installed_targets(self) -> Optional[Set[str]]:
        """Return installed targets, or ``None`` when rustup cannot answer."""
        if self._installed_targets is not None:
            return self._installed_targets

        try:
            result = subprocess.run(
                [
                    self._rustup_command(),
                    "target",
                    "list",
                    "--installed",
                    "--toolchain",
                    self.config.rust_version,
                ],
                capture_output=True,
                text=True,
                env=self.env,
            )

            if result.returncode == 0:
                targets = result.stdout.strip().split("\n")
                self._installed_targets = set(t for t in targets if t)
            else:
                self.logger.error(
                    f"Failed to inspect installed Rust targets: {result.stderr}"
                )
                return None

        except OSError as error:
            self.logger.error(f"Failed to inspect installed Rust targets: {error}")
            return None

        return self._installed_targets

    def is_target_installed(self, rust_target: str) -> Optional[bool]:
        """Return installed/missing, or ``None`` when inspection failed."""
        installed = self.get_installed_targets()
        if installed is None:
            return None
        return rust_target in installed

    def add_target(self, rust_target: str) -> bool:
        """Add a Rust target using rustup."""
        installed = self.is_target_installed(rust_target)
        if installed is True:
            self.logger.debug(f"Target {rust_target} is already installed")
            return True
        if installed is None:
            self.logger.error(
                f"Cannot add Rust target {rust_target}: installed targets are unknown"
            )
            return False

        self.logger.info(f"Adding Rust target: {rust_target}")

        try:
            result = subprocess.run(
                [
                    self._rustup_command(),
                    "target",
                    "add",
                    rust_target,
                    "--toolchain",
                    self.config.rust_version,
                ],
                capture_output=True,
                text=True,
                env=self.env,
            )

            if result.returncode == 0:
                self.logger.success(f"Target {rust_target} added")
                # Invalidate cache
                self._installed_targets = None
                return True
            else:
                self.logger.error(f"Failed to add target: {result.stderr}")
                return False

        except OSError as error:
            self.logger.error(f"Failed to add Rust target {rust_target}: {error}")
            return False

    def get_installed_toolchains(self) -> Optional[Set[str]]:
        """Return installed toolchains, or ``None`` when rustup cannot answer."""
        if self._installed_toolchains is not None:
            return self._installed_toolchains

        try:
            result = subprocess.run(
                [self._rustup_command(), "toolchain", "list"],
                capture_output=True,
                text=True,
                env=self.env,
            )
            if result.returncode == 0:
                self._installed_toolchains = {
                    line.split()[0]
                    for line in result.stdout.splitlines()
                    if line.strip()
                }
            else:
                self.logger.error(
                    f"Failed to inspect installed Rust toolchains: {result.stderr}"
                )
                return None
        except OSError as error:
            self.logger.error(f"Failed to inspect installed Rust toolchains: {error}")
            return None

        return self._installed_toolchains

    def is_toolchain_installed(self, toolchain: str) -> Optional[bool]:
        """Return installed/missing, or ``None`` when inspection failed."""
        installed = self.get_installed_toolchains()
        if installed is None:
            return None
        return toolchain in installed

    def add_toolchain(self, toolchain: str) -> bool:
        """Install a rustup toolchain."""
        installed = self.is_toolchain_installed(toolchain)
        if installed is True:
            self.logger.debug(f"Toolchain {toolchain} is already installed")
            return True
        if installed is None:
            self.logger.error(
                f"Cannot install Rust toolchain {toolchain}: installed toolchains are unknown"
            )
            return False

        self.logger.info(f"Installing Rust toolchain: {toolchain}")
        cmd = [self._rustup_command(), "toolchain", "install", toolchain]
        try:
            result = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                env=self.env,
            )
            if result.returncode == 0:
                self.logger.success(f"Toolchain {toolchain} installed")
                self._installed_toolchains = None
                return True
            self.logger.error(f"Failed to install toolchain: {result.stderr}")
            return False
        except OSError as error:
            self.logger.error(f"Failed to install Rust toolchain {toolchain}: {error}")
            return False

    def _msvc_linker_available(self) -> bool:
        """Check whether the linker on PATH is Microsoft's linker.

        Git/MSYS environments can put the unrelated GNU ``link`` utility on
        PATH.  Treating that program as MSVC makes generic Windows targets
        select a toolchain which is guaranteed to fail at link time.
        """
        linker = shutil.which("link.exe")
        if not linker:
            return False

        try:
            result = subprocess.run(
                [linker, "/?"],
                capture_output=True,
                text=True,
            )
        except (FileNotFoundError, OSError):
            return False

        banner = f"{result.stdout}\n{result.stderr}".lower()
        return "microsoft" in banner and "linker" in banner

    def _default_windows_spec(
        self,
        spec: str,
        allow_system_probe: bool = True,
    ) -> str:
        """
        Resolve generic Windows aliases to the best local toolchain.

        On Windows without a Visual Studio developer environment, the MSVC
        targets fail at link time. Prefer the rustup GNU/LLVM toolchains there.
        Explicit -msvc and -gnullvm aliases are always honored.
        """
        if self.config.host_os != "windows":
            return spec
        if spec not in ("windows-arm64", "windows-x86_64"):
            return spec
        # Non-interactive builds choose the deterministic native MSVC triple.
        # Local debug builds retain the convenience GNU/LLVM fallback.
        if not allow_system_probe:
            return spec
        if self._msvc_linker_available():
            return spec
        return f"{spec}-gnullvm"

    def _target_from_friendly_spec(
        self,
        spec: str,
        allow_system_probe: bool = True,
    ) -> Target:
        """Resolve a friendly target while preserving its release filename.

        A generic Windows alias may select GNU/LLVM internally on Windows,
        but it still has to produce the canonical artifact name consumed by
        ``manage.ps1``.  Explicit ``-msvc`` and ``-gnullvm`` aliases retain a
        suffix so callers can request both variants without a name collision.
        """
        resolved_spec = self._default_windows_spec(spec, allow_system_probe)
        target = Target.from_rust_target(RUST_TARGETS[resolved_spec], self.config)
        if spec.endswith("-msvc"):
            target.friendly_name = f"{TARGET_NAMES[RUST_TARGETS[spec]]}-msvc"
        elif resolved_spec != spec:
            target.friendly_name = TARGET_NAMES[RUST_TARGETS[spec]]
        return target

    def _replace_resolved_name(self, resolved: List[Target], target: Target) -> None:
        """Prefer a canonical alias name for an already-resolved triple."""
        for existing in resolved:
            if existing.rust_target == target.rust_target:
                existing.friendly_name = target.friendly_name
                return

    def resolve_targets(
        self,
        target_specs: List[str],
        allow_system_probe: bool = True,
    ) -> List[Target]:
        """
        Resolve target specifications to Target objects.

        Handles special values like:
        - "native" - current platform
        - "macos" - both macOS targets
        - "linux" - both Linux targets
        - "all" - all targets
        - "macos-arm64", "linux-x86_64" etc. - specific targets
        """
        resolved: List[Target] = []
        seen: Set[str] = set()

        for spec in target_specs:
            spec = spec.lower().strip()

            if spec == "native":
                # Add native target
                native_target = self._get_native_target(allow_system_probe)
                if native_target and native_target.rust_target not in seen:
                    resolved.append(native_target)
                    seen.add(native_target.rust_target)
                elif native_target:
                    self._replace_resolved_name(resolved, native_target)

            elif spec == "all":
                # Add all targets (excluding Windows — use --windows explicitly)
                for name, rust_target in RUST_TARGETS.items():
                    if "windows" not in name and rust_target not in seen:
                        target = Target.from_rust_target(rust_target, self.config)
                        resolved.append(target)
                        seen.add(rust_target)

            elif spec == "macos":
                # Add both macOS targets
                for name, rust_target in RUST_TARGETS.items():
                    if "macos" in name and rust_target not in seen:
                        target = Target.from_rust_target(rust_target, self.config)
                        resolved.append(target)
                        seen.add(rust_target)

            elif spec == "linux":
                # Add both Linux targets
                for name, rust_target in RUST_TARGETS.items():
                    if "linux" in name and rust_target not in seen:
                        target = Target.from_rust_target(rust_target, self.config)
                        resolved.append(target)
                        seen.add(rust_target)

            elif spec == "windows":
                # Add both Windows targets
                for name in ("windows-x86_64", "windows-arm64"):
                    target = self._target_from_friendly_spec(
                        name,
                        allow_system_probe,
                    )
                    rust_target = target.rust_target
                    if rust_target not in seen:
                        resolved.append(target)
                        seen.add(rust_target)
                    else:
                        self._replace_resolved_name(resolved, target)

            elif spec in RUST_TARGETS:
                # Direct friendly name (e.g., "macos-arm64")
                target = self._target_from_friendly_spec(
                    spec,
                    allow_system_probe,
                )
                rust_target = target.rust_target
                if rust_target not in seen:
                    resolved.append(target)
                    seen.add(rust_target)
                elif spec in ("windows-x86_64", "windows-arm64"):
                    self._replace_resolved_name(resolved, target)

            elif spec in RUST_TARGETS.values():
                # Direct Rust target (e.g., "aarch64-apple-darwin")
                if spec not in seen:
                    target = Target.from_rust_target(spec, self.config)
                    resolved.append(target)
                    seen.add(spec)

            else:
                self.logger.warning(f"Unknown target specification: {spec}")

        return resolved

    def _get_native_target(
        self,
        allow_system_probe: bool = True,
    ) -> Optional[Target]:
        """Get the native target for the current platform."""
        host_os = self.config.host_os
        host_arch = self.config.host_arch

        # Map architecture names (aarch64 <-> arm64)
        arch_aliases = {
            "aarch64": ["aarch64", "arm64"],
            "arm64": ["aarch64", "arm64"],
            "x86_64": ["x86_64"],
        }

        # Try direct match first
        native_key = f"{host_os}-{host_arch}"
        if native_key in RUST_TARGETS:
            return self._target_from_friendly_spec(
                native_key,
                allow_system_probe,
            )

        # Try with arch aliases
        for arch_name in arch_aliases.get(host_arch, [host_arch]):
            alias_key = f"{host_os}-{arch_name}"
            if alias_key in RUST_TARGETS:
                return self._target_from_friendly_spec(
                    alias_key,
                    allow_system_probe,
                )

        # Try searching by components
        for name, rust_target in RUST_TARGETS.items():
            if host_os in name:
                for arch_name in arch_aliases.get(host_arch, [host_arch]):
                    if arch_name in name:
                        return self._target_from_friendly_spec(
                            name,
                            allow_system_probe,
                        )

        return None

    def ensure_targets(
        self,
        targets: List[Target],
        install_missing: bool = True,
    ) -> bool:
        """Ensure all specified targets are installed.

        ``install_missing=False`` is used by ``--no-auto-setup`` and performs
        a read-only availability check instead of silently invoking rustup.
        """
        success = True

        for target in targets:
            installed = self.is_target_installed(target.rust_target)
            if installed is True:
                continue
            if installed is None:
                self.logger.error(
                    f"Cannot prepare Rust target {target.rust_target}: "
                    "installed targets are unknown"
                )
                success = False
            elif not install_missing:
                self.logger.error(
                    f"Rust target {target.rust_target} is not installed. "
                    "Run with --setup-rust first."
                )
                success = False
            elif not self.add_target(target.rust_target):
                success = False

        return success
