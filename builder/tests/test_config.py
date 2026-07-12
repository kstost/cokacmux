import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch

from builder.config import (
    CARGO_XWIN_VERSION,
    CARGO_ZIGBUILD_VERSION,
    MACOS_SDK_SHA256,
    MACOS_SDK_VERSION,
    RUST_VERSION,
    RUSTUP_HOST_TARGETS,
    RUSTUP_SHA256,
    RUSTUP_VERSION,
    ZIG_SHA256,
    ZIG_VERSION,
    BuildConfig,
)


class ToolPinConfigurationTests(unittest.TestCase):
    def test_repository_rust_toolchain_matches_builder_pin(self):
        project_root = Path(__file__).resolve().parents[2]
        with (project_root / "rust-toolchain.toml").open("rb") as source:
            toolchain = tomllib.load(source)

        self.assertEqual(toolchain["toolchain"]["channel"], RUST_VERSION)
        self.assertEqual(
            set(toolchain["toolchain"]["components"]),
            {"clippy", "rustfmt"},
        )

    def test_release_tool_versions_are_exact(self):
        config = BuildConfig()

        self.assertEqual(config.rust_version, "1.96.1")
        self.assertEqual(config.rustup_version, "1.29.0")
        self.assertEqual(config.cargo_zigbuild_version, "0.23.0")
        self.assertEqual(config.cargo_xwin_version, "0.23.0")
        self.assertEqual(config.zig_version, "0.13.0")
        self.assertEqual(config.macos_sdk_version, "14.0")
        self.assertFalse(config.allow_release_auto_setup)
        self.assertEqual(
            config.macos_sdk_sha256,
            "5e4d3be6b445f0eacc0333ff2117e93e4433d8c4fe44053a14f735033a98aaa9",
        )

        self.assertEqual(RUST_VERSION, config.rust_version)
        self.assertEqual(RUSTUP_VERSION, config.rustup_version)
        self.assertEqual(CARGO_ZIGBUILD_VERSION, config.cargo_zigbuild_version)
        self.assertEqual(CARGO_XWIN_VERSION, config.cargo_xwin_version)
        self.assertEqual(ZIG_VERSION, config.zig_version)
        self.assertEqual(MACOS_SDK_VERSION, config.macos_sdk_version)
        self.assertEqual(MACOS_SDK_SHA256, config.macos_sdk_sha256)

    def test_rustup_urls_and_hashes_cover_every_supported_host(self):
        expected = {
            ("linux", "aarch64"): (
                "aarch64-unknown-linux-gnu/rustup-init",
                "9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792",
            ),
            ("linux", "x86_64"): (
                "x86_64-unknown-linux-gnu/rustup-init",
                "4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10",
            ),
            ("macos", "aarch64"): (
                "aarch64-apple-darwin/rustup-init",
                "aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1",
            ),
            ("macos", "x86_64"): (
                "x86_64-apple-darwin/rustup-init",
                "33cf85df9142bc6d29cbc62fa5ca1d4c29622cddb55213a4c1a43c457fb9b2d7",
            ),
            ("windows", "aarch64"): (
                "aarch64-pc-windows-msvc/rustup-init.exe",
                "3af309e6c3062aa11df0e932954f69d13b734d8a431e593812f3ecd9ff9e6ef6",
            ),
            ("windows", "x86_64"): (
                "x86_64-pc-windows-msvc/rustup-init.exe",
                "86478e53f769379d7f0ebfa7c9aa97cb76ca92233f79aa2cc0dbee2efaac73c7",
            ),
        }

        self.assertEqual(set(RUSTUP_HOST_TARGETS), set(expected))
        self.assertEqual(
            set(RUSTUP_SHA256),
            {(RUSTUP_VERSION, host_os, arch) for host_os, arch in expected},
        )
        for (host_os, arch), (suffix, checksum) in expected.items():
            system = "Darwin" if host_os == "macos" else host_os
            machine = "arm64" if arch == "aarch64" else arch
            with self.subTest(host_os=host_os, arch=arch), patch(
                "builder.config.platform.system", return_value=system
            ), patch("builder.config.platform.machine", return_value=machine):
                config = BuildConfig()
                self.assertEqual(
                    config.rustup_init_url,
                    "https://static.rust-lang.org/rustup/archive/"
                    f"1.29.0/{suffix}",
                )
                self.assertEqual(config.rustup_init_sha256, checksum)

    def test_unsupported_rustup_host_fails_closed(self):
        with patch(
            "builder.config.platform.system", return_value="FreeBSD"
        ), patch("builder.config.platform.machine", return_value="x86_64"):
            config = BuildConfig()
            with self.assertRaises(ValueError):
                _ = config.rustup_init_url
            with self.assertRaises(ValueError):
                _ = config.rustup_init_sha256

    def test_zig_urls_and_hashes_cover_every_supported_host(self):
        expected = {
            ("linux", "aarch64"): (
                "zig-linux-aarch64-0.13.0.tar.xz",
                "041ac42323837eb5624068acd8b00cd5777dac4cf91179e8dad7a7e90dd0c556",
            ),
            ("linux", "x86_64"): (
                "zig-linux-x86_64-0.13.0.tar.xz",
                "d45312e61ebcc48032b77bc4cf7fd6915c11fa16e4aad116b66c9468211230ea",
            ),
            ("macos", "aarch64"): (
                "zig-macos-aarch64-0.13.0.tar.xz",
                "46fae219656545dfaf4dce12fb4e8685cec5b51d721beee9389ab4194d43394c",
            ),
            ("macos", "x86_64"): (
                "zig-macos-x86_64-0.13.0.tar.xz",
                "8b06ed1091b2269b700b3b07f8e3be3b833000841bae5aa6a09b1a8b4773effd",
            ),
            ("windows", "aarch64"): (
                "zig-windows-aarch64-0.13.0.zip",
                "95ff88427af7ba2b4f312f45d2377ce7a033e5e3c620c8caaa396a9aba20efda",
            ),
            ("windows", "x86_64"): (
                "zig-windows-x86_64-0.13.0.zip",
                "d859994725ef9402381e557c60bb57497215682e355204d754ee3df75ee3c158",
            ),
        }

        self.assertEqual(
            set(ZIG_SHA256),
            {(ZIG_VERSION, host_os, arch) for host_os, arch in expected},
        )
        for (host_os, arch), (filename, checksum) in expected.items():
            system = "Darwin" if host_os == "macos" else host_os
            machine = "arm64" if arch == "aarch64" else arch
            with self.subTest(host_os=host_os, arch=arch), patch(
                "builder.config.platform.system", return_value=system
            ), patch("builder.config.platform.machine", return_value=machine):
                config = BuildConfig()
                self.assertTrue(config.zig_url.endswith(filename))
                self.assertEqual(config.zig_sha256, checksum)

    def test_unsupported_zig_host_fails_closed(self):
        with patch(
            "builder.config.platform.system", return_value="FreeBSD"
        ), patch("builder.config.platform.machine", return_value="x86_64"):
            config = BuildConfig()
            with self.assertRaises(ValueError):
                _ = config.zig_url
            with self.assertRaises(ValueError):
                _ = config.zig_sha256


if __name__ == "__main__":
    unittest.main()
