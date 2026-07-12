import os
import unittest
from unittest.mock import MagicMock, patch

import build


class BuildCliTests(unittest.TestCase):
    def _run_mocked_build(self, argv, environ=None):
        installer = MagicMock()
        with patch.dict(os.environ, environ or {}, clear=True), patch(
            "sys.argv", ["build.py", *argv]
        ), patch("build.print_banner"), patch(
            "build.ToolInstaller", return_value=installer
        ), patch(
            "build.ensure_rust_installed", return_value=True
        ) as ensure_rust, patch(
            "build.run_build", return_value=True
        ) as run_build:
            exit_code = build.main()
        return exit_code, ensure_rust, run_build

    def test_default_release_cli_keeps_auto_setup_enabled(self):
        exit_code, ensure_rust, run_build = self._run_mocked_build([])

        self.assertEqual(exit_code, 0)
        ensure_rust.assert_called_once()
        self.assertTrue(ensure_rust.call_args.args[2])
        self.assertTrue(run_build.call_args.args[0].allow_release_auto_setup)
        self.assertTrue(run_build.call_args.kwargs["auto_setup"])

    def test_ci_debug_cli_keeps_auto_setup_enabled(self):
        exit_code, ensure_rust, run_build = self._run_mocked_build(
            ["--debug"], {"CI": "true"}
        )

        self.assertEqual(exit_code, 0)
        self.assertTrue(ensure_rust.call_args.args[2])
        self.assertTrue(run_build.call_args.kwargs["auto_setup"])

    def test_local_debug_cli_keeps_auto_setup_enabled(self):
        exit_code, ensure_rust, run_build = self._run_mocked_build(["--debug"])

        self.assertEqual(exit_code, 0)
        self.assertTrue(ensure_rust.call_args.args[2])
        self.assertTrue(run_build.call_args.kwargs["auto_setup"])

    def test_no_auto_setup_is_forwarded_to_the_executor(self):
        exit_code, ensure_rust, run_build = self._run_mocked_build(
            ["--no-auto-setup"]
        )

        self.assertEqual(exit_code, 0)
        self.assertFalse(ensure_rust.call_args.args[2])
        self.assertFalse(run_build.call_args.kwargs["auto_setup"])

    def test_explicit_setup_remains_available_in_ci(self):
        installer = MagicMock()
        installer.setup_all.return_value = True
        with patch.dict(os.environ, {"CI": "true"}, clear=True), patch(
            "sys.argv", ["build.py", "--setup"]
        ), patch("build.print_banner"), patch(
            "build.ToolInstaller", return_value=installer
        ):
            exit_code = build.main()

        self.assertEqual(exit_code, 0)
        installer.setup_all.assert_called_once_with()

    def test_debug_and_release_can_be_combined(self):
        parser = build.create_parser()
        args = parser.parse_args(["--debug", "--release"])

        self.assertTrue(args.debug)
        self.assertTrue(args.release)

    def test_setup_actions_can_be_combined(self):
        parser = build.create_parser()
        args = parser.parse_args(["--setup-rust", "--setup-cross"])

        self.assertTrue(args.setup_rust)
        self.assertTrue(args.setup_cross)

    def test_setup_mode_takes_precedence_over_build_targets(self):
        installer = MagicMock()
        installer.setup_all.return_value = True
        with patch("sys.argv", ["build.py", "--setup", "--windows"]), patch(
            "build.print_banner"
        ), patch("build.ToolInstaller", return_value=installer), patch(
            "build.run_build"
        ) as run_build:
            exit_code = build.main()

        self.assertEqual(exit_code, 0)
        installer.setup_all.assert_called_once_with()
        run_build.assert_not_called()

    def test_existing_rust_is_accepted_when_default_toolchain_setup_fails(self):
        installer = MagicMock()
        installer.is_rust_installed.return_value = True
        installer._ensure_default_toolchain.return_value = False

        self.assertTrue(
            build.ensure_rust_installed(installer, MagicMock(), auto_setup=False)
        )
        installer._ensure_default_toolchain.assert_called_once_with()

    def test_existing_system_rust_is_accepted_without_auto_setup(self):
        installer = MagicMock()
        installer.is_rust_installed.return_value = True
        installer.get_pinned_rustup_path.return_value = None
        installer._ensure_default_toolchain.return_value = True

        self.assertTrue(
            build.ensure_rust_installed(installer, MagicMock(), auto_setup=False)
        )
        installer._ensure_default_toolchain.assert_called_once_with()


if __name__ == "__main__":
    unittest.main()
