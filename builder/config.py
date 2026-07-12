"""
Build configuration for COKACDIR Rust project.
"""
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Tuple
import platform


RUST_VERSION = "1.96.1"
RUSTUP_VERSION = "1.29.0"
RUSTUP_DIST_SERVER = "https://static.rust-lang.org"
RUSTUP_UPDATE_ROOT = "https://static.rust-lang.org/rustup"
CARGO_ZIGBUILD_VERSION = "0.23.0"
CARGO_XWIN_VERSION = "0.23.0"
ZIG_VERSION = "0.13.0"
MACOS_SDK_VERSION = "14.0"
MACOS_SDK_SHA256 = (
    "5e4d3be6b445f0eacc0333ff2117e93e4433d8c4fe44053a14f735033a98aaa9"
)

RUSTUP_HOST_TARGETS: Dict[Tuple[str, str], str] = {
    ("linux", "aarch64"): "aarch64-unknown-linux-gnu",
    ("linux", "x86_64"): "x86_64-unknown-linux-gnu",
    ("macos", "aarch64"): "aarch64-apple-darwin",
    ("macos", "x86_64"): "x86_64-apple-darwin",
    ("windows", "aarch64"): "aarch64-pc-windows-msvc",
    ("windows", "x86_64"): "x86_64-pc-windows-msvc",
}

RUSTUP_SHA256: Dict[Tuple[str, str, str], str] = {
    (
        RUSTUP_VERSION,
        "linux",
        "aarch64",
    ): "9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792",
    (
        RUSTUP_VERSION,
        "linux",
        "x86_64",
    ): "4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10",
    (
        RUSTUP_VERSION,
        "macos",
        "aarch64",
    ): "aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1",
    (
        RUSTUP_VERSION,
        "macos",
        "x86_64",
    ): "33cf85df9142bc6d29cbc62fa5ca1d4c29622cddb55213a4c1a43c457fb9b2d7",
    (
        RUSTUP_VERSION,
        "windows",
        "aarch64",
    ): "3af309e6c3062aa11df0e932954f69d13b734d8a431e593812f3ecd9ff9e6ef6",
    (
        RUSTUP_VERSION,
        "windows",
        "x86_64",
    ): "86478e53f769379d7f0ebfa7c9aa97cb76ca92233f79aa2cc0dbee2efaac73c7",
}

ZIG_SHA256: Dict[Tuple[str, str, str], str] = {
    (
        ZIG_VERSION,
        "linux",
        "aarch64",
    ): "041ac42323837eb5624068acd8b00cd5777dac4cf91179e8dad7a7e90dd0c556",
    (
        ZIG_VERSION,
        "linux",
        "x86_64",
    ): "d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea",
    (
        ZIG_VERSION,
        "macos",
        "aarch64",
    ): "46fae219656545dfaf4dce12fb4e8685cec5b51d721beee9389ab4194d43394c",
    (
        ZIG_VERSION,
        "macos",
        "x86_64",
    ): "8b06ed1091b2269b700b3b07f8e3be3b833000841bae5aa6a09b1a8b4773effd",
    (
        ZIG_VERSION,
        "windows",
        "aarch64",
    ): "95ff88427af7ba2b4f312f45d2377ce7a033e5e3c620c8caaa396a9aba20efda",
    (
        ZIG_VERSION,
        "windows",
        "x86_64",
    ): "d859994725ef9402381e557c60bb57497215682e355204d754ee3df75ee3c158",
}


@dataclass
class BuildConfig:
    """Build configuration settings."""

    # Tool versions
    rust_version: str = RUST_VERSION
    rustup_version: str = RUSTUP_VERSION
    cargo_zigbuild_version: str = CARGO_ZIGBUILD_VERSION
    cargo_xwin_version: str = CARGO_XWIN_VERSION
    zig_version: str = ZIG_VERSION
    macos_sdk_version: str = MACOS_SDK_VERSION
    macos_sdk_sha256: str = MACOS_SDK_SHA256

    # Paths (relative to project root)
    builder_dir: Path = field(default_factory=lambda: Path("builder"))
    tools_dir: Path = field(default_factory=lambda: Path("builder/tools"))
    dist_dir: Path = field(default_factory=lambda: Path("dist_beta"))

    # Build settings
    release: bool = True
    clean: bool = False
    allow_release_auto_setup: bool = False

    # Target platforms
    targets: List[str] = field(default_factory=list)
    build_native: bool = True

    # URLs
    @property
    def rustup_init_url(self) -> str:
        # Resolve the checksum first so the URL cannot be constructed for an
        # unsupported host without an accompanying pinned digest.
        self.rustup_init_sha256
        try:
            target = RUSTUP_HOST_TARGETS[(self.host_os, self.host_arch)]
        except KeyError as error:
            raise ValueError(
                "Unsupported rustup bootstrap host: "
                f"{self.host_os}/{self.host_arch}"
            ) from error
        executable = (
            "rustup-init.exe" if self.host_os == "windows" else "rustup-init"
        )
        return (
            "https://static.rust-lang.org/rustup/archive/"
            f"{self.rustup_version}/{target}/{executable}"
        )

    @property
    def rustup_init_sha256(self) -> str:
        key = (self.rustup_version, self.host_os, self.host_arch)
        try:
            return RUSTUP_SHA256[key]
        except KeyError as error:
            raise ValueError(
                "Unsupported rustup bootstrap host/version: "
                f"{self.host_os}/{self.host_arch} for rustup {self.rustup_version}"
            ) from error

    @property
    def macos_sdk_url(self) -> str:
        return f"https://github.com/joseluisq/macosx-sdks/releases/download/{self.macos_sdk_version}/MacOSX{self.macos_sdk_version}.sdk.tar.xz"

    @property
    def zig_url(self) -> str:
        # Resolve the checksum first so unknown OS/architecture pairs fail
        # closed instead of being mistaken for a supported download host.
        self.zig_sha256
        arch = self.host_arch
        if self.host_os == "windows":
            return f"https://ziglang.org/download/{self.zig_version}/zig-windows-{arch}-{self.zig_version}.zip"
        return f"https://ziglang.org/download/{self.zig_version}/zig-{self.host_os}-{arch}-{self.zig_version}.tar.xz"

    @property
    def zig_sha256(self) -> str:
        key = (self.zig_version, self.host_os, self.host_arch)
        try:
            return ZIG_SHA256[key]
        except KeyError as error:
            raise ValueError(
                "Unsupported Zig download host/version: "
                f"{self.host_os}/{self.host_arch} for Zig {self.zig_version}"
            ) from error

    # Host detection
    @property
    def host_arch(self) -> str:
        machine = platform.machine().lower()
        if machine in ("x86_64", "amd64"):
            return "x86_64"
        elif machine in ("aarch64", "arm64"):
            return "aarch64"
        return machine

    @property
    def host_os(self) -> str:
        system = platform.system().lower()
        if system == "darwin":
            return "macos"
        return system

    def __post_init__(self):
        # Convert string paths to Path objects if needed
        if isinstance(self.builder_dir, str):
            self.builder_dir = Path(self.builder_dir)
        if isinstance(self.tools_dir, str):
            self.tools_dir = Path(self.tools_dir)
        if isinstance(self.dist_dir, str):
            self.dist_dir = Path(self.dist_dir)


# Available Rust targets
RUST_TARGETS = {
    "macos-arm64": "aarch64-apple-darwin",
    "macos-x86_64": "x86_64-apple-darwin",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "linux-x86_64": "x86_64-unknown-linux-gnu",
    "windows-x86_64": "x86_64-pc-windows-msvc",
    "windows-arm64": "aarch64-pc-windows-msvc",
    "windows-x86_64-msvc": "x86_64-pc-windows-msvc",
    "windows-arm64-msvc": "aarch64-pc-windows-msvc",
    "windows-x86_64-gnullvm": "x86_64-pc-windows-gnullvm",
    "windows-arm64-gnullvm": "aarch64-pc-windows-gnullvm",
}

# Friendly names for targets
TARGET_NAMES = {
    "aarch64-apple-darwin": "macos-aarch64",
    "x86_64-apple-darwin": "macos-x86_64",
    "aarch64-unknown-linux-gnu": "linux-aarch64",
    "x86_64-unknown-linux-gnu": "linux-x86_64",
    "x86_64-pc-windows-msvc": "windows-x86_64",
    "aarch64-pc-windows-msvc": "windows-aarch64",
    "x86_64-pc-windows-gnullvm": "windows-x86_64-gnullvm",
    "aarch64-pc-windows-gnullvm": "windows-aarch64-gnullvm",
}
