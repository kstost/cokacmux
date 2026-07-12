import unittest
import os
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from builder.targets import Target, TargetManager


class _HostConfig:
    def __init__(self, host_os: str, host_arch: str = "x86_64"):
        self.host_os = host_os
        self.host_arch = host_arch
        self.rust_version = "1.96.1"


class TargetClassificationTests(unittest.TestCase):
    def test_linux_targets_always_use_zigbuild(self):
        for host_os in ("linux", "macos", "windows"):
            with self.subTest(host_os=host_os):
                target = Target.from_rust_target(
                    "x86_64-unknown-linux-gnu", _HostConfig(host_os)
                )
                self.assertTrue(target.needs_zigbuild)
                self.assertFalse(target.is_native)

    def test_macos_targets_use_zigbuild_only_off_macos(self):
        native = Target.from_rust_target(
            "x86_64-apple-darwin", _HostConfig("macos")
        )
        self.assertFalse(native.needs_zigbuild)
        self.assertTrue(native.is_native)

        for host_os in ("linux", "windows"):
            with self.subTest(host_os=host_os):
                target = Target.from_rust_target(
                    "x86_64-apple-darwin", _HostConfig(host_os)
                )
                self.assertTrue(target.needs_zigbuild)
                self.assertFalse(target.is_native)

    def test_windows_targets_never_use_zigbuild(self):
        for host_os in ("linux", "macos", "windows"):
            with self.subTest(host_os=host_os):
                target = Target.from_rust_target(
                    "x86_64-pc-windows-msvc", _HostConfig(host_os)
                )
                self.assertFalse(target.needs_zigbuild)


class WindowsTargetResolutionTests(unittest.TestCase):
    def manager(self, arch="x86_64"):
        return TargetManager(_HostConfig("windows", arch), MagicMock())

    def test_msvc_detection_only_accepts_a_microsoft_linker_banner(self):
        manager = self.manager()
        with patch("builder.targets.shutil.which", return_value="C:/tools/link.exe"), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                stdout="Microsoft (R) Incremental Linker Version 14.0",
                stderr="",
            ),
        ):
            self.assertTrue(manager._msvc_linker_available())

        with patch("builder.targets.shutil.which", return_value="C:/git/link.exe"), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(stdout="GNU coreutils link", stderr=""),
        ):
            self.assertFalse(manager._msvc_linker_available())

    def test_extensionless_posix_link_command_is_not_considered(self):
        manager = self.manager()
        with patch("builder.targets.shutil.which", return_value=None) as which:
            self.assertFalse(manager._msvc_linker_available())
        which.assert_called_once_with("link.exe")

    def test_automatic_gnullvm_fallback_keeps_installer_artifact_name(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=False)

        target = manager.resolve_targets(["windows-x86_64"])[0]

        self.assertEqual(target.rust_target, "x86_64-pc-windows-gnullvm")
        self.assertEqual(target.friendly_name, "windows-x86_64")

    def test_explicit_gnullvm_target_keeps_variant_suffix(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=False)

        target = manager.resolve_targets(["windows-x86_64-gnullvm"])[0]

        self.assertEqual(target.friendly_name, "windows-x86_64-gnullvm")

    def test_explicit_msvc_target_gets_variant_suffix(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=False)

        target = manager.resolve_targets(["windows-x86_64-msvc"])[0]

        self.assertEqual(target.friendly_name, "windows-x86_64-msvc")

    def test_generic_alias_wins_artifact_name_when_same_triple_was_explicit_first(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=True)

        targets = manager.resolve_targets(
            ["windows-x86_64-msvc", "windows-x86_64"]
        )

        self.assertEqual(len(targets), 1)
        self.assertEqual(targets[0].friendly_name, "windows-x86_64")

    def test_native_arm64_fallback_uses_canonical_distribution_arch(self):
        manager = self.manager("aarch64")
        manager._msvc_linker_available = MagicMock(return_value=False)

        target = manager.resolve_targets(["native"])[0]

        self.assertEqual(target.rust_target, "aarch64-pc-windows-gnullvm")
        self.assertEqual(target.friendly_name, "windows-aarch64")

    def test_explicit_windows_variants_never_probe_path_linker(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=False)

        targets = manager.resolve_targets(
            ["windows-x86_64-msvc", "windows-arm64-gnullvm"]
        )

        self.assertEqual(len(targets), 2)
        manager._msvc_linker_available.assert_not_called()

    def test_noninteractive_generic_windows_target_is_deterministic_msvc(self):
        manager = self.manager()
        manager._msvc_linker_available = MagicMock(return_value=False)

        target = manager.resolve_targets(
            ["windows-x86_64"],
            allow_system_probe=False,
        )[0]

        self.assertEqual(target.rust_target, "x86_64-pc-windows-msvc")
        self.assertEqual(target.friendly_name, "windows-x86_64")
        manager._msvc_linker_available.assert_not_called()


class TargetInstallationTests(unittest.TestCase):
    def test_direct_manager_forces_read_only_official_rustup_environment(self):
        with patch.dict(
            os.environ,
            {
                "RUSTUP_AUTO_INSTALL": "1",
                "RUSTUP_DIST_SERVER": "https://mirror.invalid",
                "RUSTUP_UPDATE_ROOT": "https://mirror.invalid/rustup",
                "RUSTUP_TOOLCHAIN": "nightly",
            },
            clear=True,
        ):
            manager = TargetManager(_HostConfig("linux"), MagicMock())

        self.assertEqual(manager.env["RUSTUP_AUTO_INSTALL"], "0")
        self.assertEqual(
            manager.env["RUSTUP_DIST_SERVER"],
            "https://static.rust-lang.org",
        )
        self.assertEqual(
            manager.env["RUSTUP_UPDATE_ROOT"],
            "https://static.rust-lang.org/rustup",
        )
        self.assertNotIn("RUSTUP_TOOLCHAIN", manager.env)

    def test_explicit_rustup_path_bypasses_path_resolution(self):
        manager = TargetManager(
            _HostConfig("linux"),
            MagicMock(),
            rustup_path=Path("/receipted/rustup"),
        )
        with patch("builder.targets.shutil.which") as which:
            self.assertEqual(manager._rustup_command(), "/receipted/rustup")
        which.assert_not_called()

    def test_target_list_queries_exact_release_toolchain(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock(), env={})
        with patch.object(
            manager, "_rustup_command", return_value="/rustup"
        ), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                returncode=0,
                stdout="x86_64-unknown-linux-gnu\n",
                stderr="",
            ),
        ) as run:
            self.assertEqual(
                manager.get_installed_targets(),
                {"x86_64-unknown-linux-gnu"},
            )

        self.assertEqual(
            run.call_args.args[0],
            [
                "/rustup",
                "target",
                "list",
                "--installed",
                "--toolchain",
                "1.96.1",
            ],
        )

    def test_target_add_uses_exact_release_toolchain(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock(), env={})
        manager._installed_targets = set()
        with patch.object(
            manager, "_rustup_command", return_value="/rustup"
        ), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(returncode=0, stdout="", stderr=""),
        ) as run:
            self.assertTrue(manager.add_target("aarch64-unknown-linux-gnu"))

        self.assertEqual(
            run.call_args.args[0],
            [
                "/rustup",
                "target",
                "add",
                "aarch64-unknown-linux-gnu",
                "--toolchain",
                "1.96.1",
            ],
        )

    def test_opposite_arch_windows_gnullvm_is_target_on_native_toolchain(self):
        manager = TargetManager(_HostConfig("windows"), MagicMock())
        manager._installed_targets = set()
        manager.add_toolchain = MagicMock(return_value=True)
        target = Target.from_rust_target(
            "aarch64-pc-windows-gnullvm", manager.config
        )

        with patch.object(
            manager, "_rustup_command", return_value="C:/verified/rustup.exe"
        ), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(returncode=0, stdout="", stderr=""),
        ) as run:
            self.assertTrue(manager.ensure_targets([target]))

        self.assertEqual(
            run.call_args.args[0],
            [
                "C:/verified/rustup.exe",
                "target",
                "add",
                "aarch64-pc-windows-gnullvm",
                "--toolchain",
                "1.96.1",
            ],
        )
        manager.add_toolchain.assert_not_called()

    def test_no_auto_setup_does_not_invoke_rustup_target_add(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock())
        manager._installed_targets = set()
        manager.add_target = MagicMock(return_value=True)
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", manager.config)

        self.assertFalse(manager.ensure_targets([target], install_missing=False))
        manager.add_target.assert_not_called()

    def test_target_query_failure_is_unknown_and_never_adds_in_auto_setup(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock(), env={})
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", manager.config)
        with patch.object(
            manager, "_rustup_command", return_value="/rustup"
        ), patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="query failed",
            ),
        ) as run:
            self.assertFalse(manager.ensure_targets([target], install_missing=True))

        self.assertEqual(run.call_count, 1)
        self.assertEqual(
            run.call_args.args[0],
            [
                "/rustup",
                "target",
                "list",
                "--installed",
                "--toolchain",
                "1.96.1",
            ],
        )

    def test_target_query_failure_is_unknown_and_read_only_check_fails(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock(), env={})
        manager.add_target = MagicMock(return_value=True)
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", manager.config)
        with patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="query failed",
            ),
        ):
            self.assertFalse(manager.ensure_targets([target], install_missing=False))

        manager.add_target.assert_not_called()

    def test_add_target_does_not_mutate_after_unknown_inspection(self):
        manager = TargetManager(_HostConfig("linux"), MagicMock(), env={})
        with patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="query failed",
            ),
        ) as run:
            self.assertFalse(manager.add_target("aarch64-unknown-linux-gnu"))

        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][1:4], ["target", "list", "--installed"])

    def test_add_toolchain_does_not_install_after_unknown_inspection(self):
        manager = TargetManager(_HostConfig("windows"), MagicMock(), env={})
        with patch(
            "builder.targets.subprocess.run",
            return_value=SimpleNamespace(
                returncode=1,
                stdout="",
                stderr="query failed",
            ),
        ) as run:
            self.assertFalse(manager.add_toolchain("1.96.1"))

        self.assertEqual(run.call_count, 1)
        self.assertEqual(
            run.call_args.args[0][1:],
            ["toolchain", "list"],
        )


if __name__ == "__main__":
    unittest.main()
