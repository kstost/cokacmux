import os
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from builder.executor import BuildExecutor, BuildResult, run_build
from builder.targets import Target


class _Config:
    def __init__(
        self,
        host_os="linux",
        host_arch="x86_64",
        clean=False,
        release=True,
        allow_release_auto_setup=False,
    ):
        self.host_os = host_os
        self.host_arch = host_arch
        self.rust_version = "1.96.1"
        self.release = release
        self.clean = clean
        self.allow_release_auto_setup = allow_release_auto_setup
        self.dist_dir = Path("dist")


def _executor(root, config=None):
    installer = MagicMock()
    installer.cargo_home = Path(root) / "builder" / "tools" / "cargo"
    installer.get_rustup_path.return_value = Path("/verified/rustup")
    installer.get_pinned_rustup_path.return_value = Path("/verified/rustup")
    installer.verify_release_toolchain.return_value = True
    installer.get_env.return_value = {"PRESERVE": "yes"}
    return BuildExecutor(
        config or _Config(),
        Path(root),
        installer,
        MagicMock(),
        MagicMock(),
    )


class ExecutorSafetyTests(unittest.TestCase):
    def test_every_build_variant_runs_cargo_through_verified_pinned_rustup(self):
        cases = [
            (
                "release-native",
                _Config(host_os="macos"),
                Target(
                    "x86_64-apple-darwin",
                    "macos-x86_64",
                    "macos",
                    "x86_64",
                    is_native=True,
                ),
                [
                    "/verified/rustup",
                    "run",
                    "1.96.1",
                    "cargo",
                    "build",
                    "--locked",
                    "--offline",
                    "--release",
                ],
                True,
            ),
            (
                "debug-zigbuild",
                _Config(release=False),
                Target.from_rust_target(
                    "x86_64-unknown-linux-gnu",
                    _Config(release=False),
                ),
                [
                    "/verified/rustup",
                    "run",
                    "1.96.1",
                    "cargo",
                    "zigbuild",
                    "--locked",
                    "--target",
                    "x86_64-unknown-linux-gnu.2.17",
                ],
                False,
            ),
            (
                "debug-xwin",
                _Config(release=False),
                Target.from_rust_target(
                    "x86_64-pc-windows-msvc",
                    _Config(release=False),
                ),
                [
                    "/verified/rustup",
                    "run",
                    "1.96.1",
                    "cargo",
                    "xwin",
                    "build",
                    "--locked",
                    "--target",
                    "x86_64-pc-windows-msvc",
                ],
                False,
            ),
            (
                "debug-opposite-arch-windows-gnullvm",
                _Config(host_os="windows", release=False),
                Target.from_rust_target(
                    "aarch64-pc-windows-gnullvm",
                    _Config(host_os="windows", release=False),
                ),
                [
                    "/verified/rustup",
                    "run",
                    "1.96.1",
                    "cargo",
                    "build",
                    "--locked",
                    "--target",
                    "aarch64-pc-windows-gnullvm",
                ],
                False,
            ),
        ]

        for name, config, target, expected, expect_offline in cases:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as root, patch.dict(
                os.environ, {}, clear=True
            ):
                executor = _executor(root, config)
                with patch.object(
                    executor,
                    "_find_binary",
                    return_value=Path(root) / "cokacmux",
                ), patch(
                    "builder.executor.subprocess.run",
                    return_value=MagicMock(returncode=0, stdout="", stderr=""),
                ) as run:
                    result = executor.build_target(target)

                self.assertTrue(result.success)
                self.assertEqual(run.call_args.args[0], expected)
                command_env = run.call_args.kwargs["env"]
                self.assertEqual(command_env["PRESERVE"], "yes")
                if expect_offline:
                    self.assertEqual(command_env["CARGO_NET_OFFLINE"], "true")
                else:
                    self.assertNotIn("CARGO_NET_OFFLINE", command_env)
                if "gnullvm" in name:
                    self.assertNotIn("CARGO_TARGET_DIR", command_env)
                    self.assertNotIn("HOST_RUSTFLAGS", command_env)
                    self.assertNotIn("CC", command_env)
                    self.assertNotIn("AR", command_env)
                self.assertEqual(
                    executor.tool_installer.get_env.return_value,
                    {"PRESERVE": "yes"},
                )

    def test_debug_no_auto_setup_build_is_locked_and_offline(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {}, clear=True
        ):
            executor = _executor(root, _Config(host_os="macos", release=False))
            target = Target(
                "x86_64-apple-darwin",
                "macos-x86_64",
                "macos",
                "x86_64",
                is_native=True,
            )
            with patch.object(
                executor,
                "_find_binary",
                return_value=Path(root) / "cokacmux",
            ), patch(
                "builder.executor.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ) as run:
                result = executor.build_target(target, auto_setup=False)

        self.assertTrue(result.success)
        self.assertEqual(
            run.call_args.args[0],
            [
                "/verified/rustup",
                "run",
                "1.96.1",
                "cargo",
                "build",
                "--locked",
                "--offline",
            ],
        )
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_NET_OFFLINE"], "true")

    def test_ci_debug_direct_build_defaults_to_locked_offline(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {"CI": "true"}, clear=True
        ):
            executor = _executor(root, _Config(host_os="macos", release=False))
            target = Target(
                "x86_64-apple-darwin",
                "macos-x86_64",
                "macos",
                "x86_64",
                is_native=True,
            )
            with patch.object(
                executor,
                "_find_binary",
                return_value=Path(root) / "cokacmux",
            ), patch(
                "builder.executor.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ) as run:
                result = executor.build_target(target)

        self.assertTrue(result.success)
        self.assertIn("--offline", run.call_args.args[0])
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_NET_OFFLINE"], "true")

    def test_release_clean_uses_verified_rustup_and_offline_lockfile_policy(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            with patch(
                "builder.executor.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ) as run:
                self.assertTrue(executor.clean())

        self.assertEqual(
            run.call_args.args[0],
            [
                "/verified/rustup",
                "run",
                "1.96.1",
                "cargo",
                "clean",
                "--locked",
                "--offline",
            ],
        )
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_NET_OFFLINE"], "true")

    def test_missing_verified_rustup_never_runs_cargo(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            executor.tool_installer.get_rustup_path.return_value = None
            executor.tool_installer.get_pinned_rustup_path.return_value = None
            target = Target(
                "x86_64-unknown-linux-gnu",
                "linux-x86_64",
                "linux",
                "x86_64",
                is_native=True,
            )
            with patch("builder.executor.subprocess.run") as run:
                self.assertFalse(executor.clean())
                result = executor.build_target(target)

        self.assertFalse(result.success)
        run.assert_not_called()

    def test_clang_wrappers_use_unique_private_directories_and_quote_clang(self):
        with tempfile.TemporaryDirectory() as root, patch(
            "builder.executor.shutil.which", return_value="/Program Files/LLVM/clang"
        ), patch("builder.executor.tempfile.tempdir", root):
            executor = _executor(root)
            first = executor._create_clang_wrapper()
            second = executor._create_clang_wrapper()
            self.addCleanup(shutil.rmtree, first, True)
            self.addCleanup(shutil.rmtree, second, True)

            self.assertNotEqual(first, second)
            script = (Path(first) / "clang").read_text(encoding="utf-8")
            self.assertIn("exec '/Program Files/LLVM/clang'", script)

    @unittest.skipIf(os.name == "nt", "requires Unix symlinks")
    def test_xwin_cache_cleanup_is_limited_to_dangling_or_owned_links(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            env = {"HOME": root}
            cache_dir = Path(root) / ".cache" / "cargo-xwin"
            cache_dir.mkdir(parents=True)
            link = cache_dir / "clang-cl"

            link.symlink_to(Path(root) / "missing-clang")
            prepared = executor._prepare_xwin_clang_cl_cache(env)
            self.assertEqual(prepared, link)
            self.assertFalse(link.is_symlink())
            self.assertEqual(env["XWIN_CACHE_DIR"], str(cache_dir))

            real_clang = Path(root) / "real-clang"
            real_clang.write_text("clang", encoding="utf-8")
            link.symlink_to(real_clang)
            executor._prepare_xwin_clang_cl_cache(env)
            self.assertTrue(link.is_symlink(), "a live cargo-xwin link was removed")

            wrapper = Path(root) / "wrapper" / "clang"
            wrapper.parent.mkdir()
            wrapper.write_text("wrapper", encoding="utf-8")
            link.unlink()
            link.symlink_to(wrapper)
            executor._remove_owned_xwin_clang_cl_link(link, wrapper)
            self.assertFalse(link.is_symlink())

            link.symlink_to(real_clang)
            executor._remove_owned_xwin_clang_cl_link(link, wrapper)
            self.assertTrue(link.is_symlink(), "an unrelated live link was removed")

    def test_copy_to_dist_uses_canonical_target_name(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="windows"))
            binary = Path(root) / "built.exe"
            binary.write_bytes(b"binary")
            target = Target.from_rust_target(
                "x86_64-pc-windows-gnullvm", executor.config
            )
            target.friendly_name = "windows-x86_64"

            copied = executor.copy_to_dist(
                [BuildResult(target=target, success=True, binary_path=binary)]
            )

            destination = Path(root) / "dist" / "cokacmux-windows-x86_64.exe"
            self.assertEqual(copied[0][0], destination)
            self.assertEqual(destination.read_bytes(), b"binary")
            self.assertEqual(list(destination.parent.glob(".*.tmp")), [])

    def test_failed_distribution_commit_restores_every_previous_binary(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            executor.dist_dir.mkdir()
            old_x86 = executor.dist_dir / "cokacmux-linux-x86_64"
            old_arm = executor.dist_dir / "cokacmux-linux-aarch64"
            old_x86.write_bytes(b"old-x86")
            old_arm.write_bytes(b"old-arm")
            new_x86 = Path(root) / "new-x86"
            new_arm = Path(root) / "new-arm"
            new_x86.write_bytes(b"new-x86")
            new_arm.write_bytes(b"new-arm")
            x86 = Target.from_rust_target("x86_64-unknown-linux-gnu", executor.config)
            arm = Target.from_rust_target("aarch64-unknown-linux-gnu", executor.config)
            results = [
                BuildResult(x86, True, new_x86),
                BuildResult(arm, True, new_arm),
            ]
            real_replace = os.replace

            def fail_second_install(source, destination):
                if (
                    Path(destination) == old_arm
                    and str(source).endswith(".tmp")
                ):
                    raise OSError("simulated publish failure")
                return real_replace(source, destination)

            with patch("builder.executor.os.replace", side_effect=fail_second_install):
                self.assertEqual(executor.copy_to_dist(results), [])

            self.assertEqual(old_x86.read_bytes(), b"old-x86")
            self.assertEqual(old_arm.read_bytes(), b"old-arm")
            self.assertEqual(list(executor.dist_dir.glob(".*.backup")), [])
            self.assertEqual(list(executor.dist_dir.glob(".*.tmp")), [])

    def test_release_build_all_forces_read_only_target_check(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            executor.target_manager.ensure_targets.return_value = False
            target = Target.from_rust_target(
                "x86_64-apple-darwin", executor.config
            )

            self.assertEqual(executor.build_all([target], auto_setup=True), [])
            executor.target_manager.ensure_targets.assert_called_once_with(
                [target], install_missing=False
            )
            executor.tool_installer.install_zig.assert_not_called()

    def test_ci_debug_build_all_forces_read_only_target_check(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {"CI": "true"}, clear=True
        ):
            executor = _executor(root, _Config(host_os="macos", release=False))
            executor.target_manager.ensure_targets.return_value = False
            target = Target.from_rust_target(
                "x86_64-apple-darwin", executor.config
            )

            self.assertEqual(executor.build_all([target], auto_setup=True), [])
            executor.target_manager.ensure_targets.assert_called_once_with(
                [target], install_missing=False
            )

    def test_local_debug_build_all_keeps_auto_setup_enabled(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {}, clear=True
        ):
            executor = _executor(root, _Config(release=False))
            executor.target_manager.ensure_targets.return_value = False
            target = Target.from_rust_target(
                "x86_64-unknown-linux-gnu", executor.config
            )

            self.assertEqual(executor.build_all([target], auto_setup=True), [])
            executor.target_manager.ensure_targets.assert_called_once_with(
                [target], install_missing=True
            )

    def test_legacy_release_build_keeps_auto_setup_enabled(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {}, clear=True
        ):
            executor = _executor(
                root,
                _Config(allow_release_auto_setup=True),
            )
            executor.target_manager.ensure_targets.return_value = False
            target = Target.from_rust_target(
                "x86_64-unknown-linux-gnu", executor.config
            )

            self.assertEqual(executor.build_all([target], auto_setup=True), [])
            executor.target_manager.ensure_targets.assert_called_once_with(
                [target], install_missing=True
            )

    def test_build_all_threads_effective_offline_policy_to_direct_build(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            executor.target_manager.ensure_targets.return_value = True
            target = Target(
                "x86_64-apple-darwin",
                "macos-x86_64",
                "macos",
                "x86_64",
                is_native=True,
            )
            expected = BuildResult(target, True, Path(root) / "cokacmux")
            with patch.object(
                executor,
                "build_target",
                return_value=expected,
            ) as build_target:
                self.assertEqual(
                    executor.build_all([target], auto_setup=True),
                    [expected],
                )

        build_target.assert_called_once_with(target, auto_setup=False)

    def test_build_all_threads_local_debug_auto_setup_to_direct_build(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {}, clear=True
        ):
            executor = _executor(root, _Config(release=False))
            executor.target_manager.ensure_targets.return_value = True
            target = Target.from_rust_target(
                "x86_64-unknown-linux-gnu",
                executor.config,
            )
            expected = BuildResult(target, True, Path(root) / "cokacmux")
            with patch.object(
                executor,
                "build_target",
                return_value=expected,
            ) as build_target:
                self.assertEqual(
                    executor.build_all([target], auto_setup=True),
                    [expected],
                )

        build_target.assert_called_once_with(target, auto_setup=True)

    def test_release_zigbuild_is_blocked_without_pinned_tool_receipt(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            executor.target_manager.ensure_targets.return_value = True
            target = Target.from_rust_target(
                "x86_64-unknown-linux-gnu",
                executor.config,
            )

            with patch.object(executor, "build_target") as build_target:
                self.assertEqual(executor.build_all([target], auto_setup=True), [])

            build_target.assert_not_called()
            executor.target_manager.ensure_targets.assert_not_called()
            executor.tool_installer.get_pinned_rustup_path.assert_not_called()
            self.assertIn(
                "pinned tool receipt",
                executor.logger.error.call_args.args[0],
            )

    def test_no_auto_gnullvm_is_blocked_without_pinned_tool_receipt(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(
            os.environ, {}, clear=True
        ):
            executor = _executor(root, _Config(host_os="windows", release=False))
            executor.target_manager.ensure_targets.return_value = True
            target = Target.from_rust_target(
                "aarch64-pc-windows-gnullvm",
                executor.config,
            )

            with patch.object(executor, "build_target") as build_target:
                self.assertEqual(executor.build_all([target], auto_setup=False), [])

            build_target.assert_not_called()
            executor.target_manager.ensure_targets.assert_not_called()
            self.assertIn(
                "pinned tool receipt",
                executor.logger.error.call_args.args[0],
            )
            executor.tool_installer.install_windows_import_libs.assert_not_called()

    def test_direct_cross_build_fails_closed_without_pinned_receipt(self):
        cases = [
            (
                _Config(),
                Target.from_rust_target(
                    "x86_64-unknown-linux-gnu",
                    _Config(),
                ),
                "pinned tool receipt",
            ),
            (
                _Config(host_os="windows"),
                Target.from_rust_target(
                    "aarch64-pc-windows-gnullvm",
                    _Config(host_os="windows"),
                ),
                "pinned tool receipt",
            ),
            (
                _Config(),
                Target.from_rust_target(
                    "aarch64-pc-windows-gnullvm",
                    _Config(),
                ),
                "pinned tool receipt",
            ),
            (
                _Config(),
                Target.from_rust_target(
                    "x86_64-pc-windows-msvc",
                    _Config(),
                ),
                "pinned cache receipt",
            ),
        ]

        for config, target, receipt_text in cases:
            with self.subTest(target=target.rust_target), tempfile.TemporaryDirectory() as root:
                executor = _executor(root, config)
                with patch("builder.executor.subprocess.run") as run:
                    result = executor.build_target(target)

                self.assertFalse(result.success)
                self.assertIn(receipt_text, result.error_message)
                run.assert_not_called()

    def test_release_xwin_is_blocked_without_pinned_sdk_receipt(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            executor.target_manager.ensure_targets.return_value = True
            target = Target.from_rust_target(
                "x86_64-pc-windows-msvc", executor.config
            )
            executor.tool_installer.is_cargo_xwin_installed.return_value = True
            executor.tool_installer.is_clang_installed.return_value = True
            executor.tool_installer.is_lld_installed.return_value = True
            executor.tool_installer.is_llvm_lib_installed.return_value = True
            executor.tool_installer.is_clang_cl_installed.return_value = True

            with patch.object(executor, "build_target") as build_target:
                self.assertEqual(executor.build_all([target], auto_setup=True), [])

            build_target.assert_not_called()
            executor.target_manager.ensure_targets.assert_not_called()
            executor.logger.error.assert_called_once()
            self.assertIn(
                "pinned cache receipt",
                executor.logger.error.call_args.args[0],
            )

    def test_successful_command_without_expected_binary_is_a_failed_build(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            target = Target(
                "x86_64-apple-darwin",
                "macos-x86_64",
                "macos",
                "x86_64",
                is_native=True,
            )
            with patch(
                "builder.executor.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ):
                result = executor.build_target(target)

            self.assertFalse(result.success)
            self.assertIn("binary was not found", result.error_message)

    def test_forged_target_flags_cannot_bypass_cross_receipt_gate(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root)
            forged = Target(
                "aarch64-pc-windows-gnullvm",
                "windows-aarch64-gnullvm",
                "windows",
                "aarch64",
                needs_gnullvm=False,
                is_native=True,
            )
            with patch("builder.executor.subprocess.run") as run:
                result = executor.build_target(forged)

        self.assertFalse(result.success)
        self.assertIn("pinned tool receipt", result.error_message)
        run.assert_not_called()

    def test_unsupported_or_unsafe_direct_target_never_executes(self):
        cases = [
            Target(
                "x86_64-unknown-freebsd",
                "freebsd-x86_64",
                "unknown",
                "x86_64",
            ),
            Target(
                "x86_64-apple-darwin",
                "../../outside",
                "macos",
                "x86_64",
            ),
        ]
        for target in cases:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as root:
                executor = _executor(root, _Config(host_os="macos"))
                with patch("builder.executor.subprocess.run") as run:
                    result = executor.build_target(target)
                self.assertFalse(result.success)
                run.assert_not_called()

    def test_noninteractive_compiler_override_fails_before_rustup(self):
        for key in ("RUSTC_WRAPPER", "RUSTC_BOOTSTRAP", "CARGO_TARGET_DIR"):
            with self.subTest(key=key), tempfile.TemporaryDirectory() as root, patch.dict(
                os.environ,
                {key: "/tmp/unreceipted-override"},
                clear=True,
            ):
                executor = _executor(root, _Config(host_os="macos"))
                target = Target.from_rust_target(
                    "x86_64-apple-darwin",
                    executor.config,
                )
                with patch("builder.executor.subprocess.run") as run:
                    result = executor.build_target(target)

                self.assertFalse(result.success)
                errors = " ".join(
                    str(arg)
                    for logged_call in executor.logger.error.call_args_list
                    for arg in logged_call.args
                )
                self.assertIn(key, errors)
                executor.tool_installer.get_pinned_rustup_path.assert_not_called()
                run.assert_not_called()

    def test_noninteractive_cargo_config_fails_before_rustup(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            cargo_config = Path(root) / ".cargo" / "config.toml"
            cargo_config.parent.mkdir()
            cargo_config.write_text(
                '[build]\nrustc-wrapper = "/tmp/unreceipted-wrapper"\n',
                encoding="utf-8",
            )
            target = Target.from_rust_target(
                "x86_64-apple-darwin",
                executor.config,
            )
            with patch("builder.executor.subprocess.run") as run:
                result = executor.build_target(target)

        self.assertFalse(result.success)
        errors = " ".join(
            str(arg)
            for logged_call in executor.logger.error.call_args_list
            for arg in logged_call.args
        )
        self.assertIn(str(cargo_config), errors)
        executor.tool_installer.get_pinned_rustup_path.assert_not_called()
        run.assert_not_called()

    def test_target_aware_environment_only_enables_required_cross_inputs(self):
        cases = [
            (
                _Config(host_os="macos", release=False),
                "x86_64-apple-darwin",
                {"include_zig": False, "include_macos_sdk": False},
            ),
            (
                _Config(release=False),
                "x86_64-unknown-linux-gnu",
                {"include_zig": True, "include_macos_sdk": False},
            ),
            (
                _Config(release=False),
                "x86_64-apple-darwin",
                {"include_zig": True, "include_macos_sdk": True},
            ),
        ]
        for config, triple, expected_kwargs in cases:
            with self.subTest(triple=triple), tempfile.TemporaryDirectory() as root, patch.dict(
                os.environ,
                {},
                clear=True,
            ):
                executor = _executor(root, config)
                target = Target.from_rust_target(triple, config)
                with patch.object(
                    executor,
                    "_find_binary",
                    return_value=Path(root) / "cokacmux",
                ), patch(
                    "builder.executor.subprocess.run",
                    return_value=MagicMock(returncode=0, stdout="", stderr=""),
                ):
                    self.assertTrue(executor.build_target(target))
                executor.tool_installer.get_env.assert_called_once_with(
                    **expected_kwargs
                )

    def test_rust_pin_mismatch_blocks_direct_clean_and_build(self):
        with tempfile.TemporaryDirectory() as root:
            config = _Config(host_os="macos")
            config.rust_version = "stable"
            executor = _executor(root, config)
            target = Target.from_rust_target("x86_64-apple-darwin", config)
            with patch("builder.executor.subprocess.run") as run:
                self.assertFalse(executor.clean())
                result = executor.build_target(target)

        self.assertFalse(result.success)
        run.assert_not_called()

    def test_toolchain_identity_failure_blocks_cargo(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            executor.tool_installer.verify_release_toolchain.return_value = False
            target = Target.from_rust_target(
                "x86_64-apple-darwin",
                executor.config,
            )
            with patch("builder.executor.subprocess.run") as run:
                result = executor.build_target(target)

        self.assertFalse(result.success)
        run.assert_not_called()

    def test_direct_build_all_binds_pinned_rustup_before_target_query(self):
        with tempfile.TemporaryDirectory() as root:
            executor = _executor(root, _Config(host_os="macos"))
            target = Target.from_rust_target(
                "x86_64-apple-darwin",
                executor.config,
            )
            executor.target_manager.ensure_targets.return_value = False

            self.assertEqual(executor.build_all([target]), [])

        executor.tool_installer.get_pinned_rustup_path.assert_called_once_with()
        executor.tool_installer.get_env.assert_called_once_with()
        self.assertEqual(
            executor.target_manager.rustup_path,
            Path("/verified/rustup"),
        )
        executor.target_manager.ensure_targets.assert_called_once()


class RunBuildSetupTests(unittest.TestCase):
    def test_release_receipt_preflight_precedes_clean_and_all_tool_probes(self):
        config = _Config(clean=True)
        logger = MagicMock()
        installer = MagicMock()
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()

        with patch("builder.executor.ToolInstaller", return_value=installer), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch("builder.executor.BuildExecutor", return_value=executor):
            self.assertFalse(
                run_build(
                    config,
                    Path("/project"),
                    ["x86_64-unknown-linux-gnu"],
                    logger,
                )
            )

        manager.resolve_targets.assert_called_once_with(
            ["x86_64-unknown-linux-gnu"],
            allow_system_probe=False,
        )
        executor.clean.assert_not_called()
        executor.build_all.assert_not_called()
        installer.get_env.assert_not_called()
        installer.get_pinned_rustup_path.assert_not_called()
        installer.is_zig_installed.assert_not_called()
        installer.is_cargo_zigbuild_installed.assert_not_called()
        manager.ensure_targets.assert_not_called()

    def test_run_build_pin_mismatch_precedes_installer_construction(self):
        config = _Config(host_os="macos")
        config.rust_version = "stable"
        logger = MagicMock()
        with patch("builder.executor.ToolInstaller") as installer:
            self.assertFalse(run_build(config, Path("/project"), ["native"], logger))
        installer.assert_not_called()

    def test_release_rejects_missing_cross_tools_without_installing(self):
        config = _Config()
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        installer.is_zig_installed.return_value = False
        installer.is_cargo_zigbuild_installed.return_value = False
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()

        with patch("builder.executor.ToolInstaller", return_value=installer), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch("builder.executor.BuildExecutor", return_value=executor):
            self.assertFalse(
                run_build(
                    config,
                    Path("/project"),
                    ["x86_64-unknown-linux-gnu"],
                    logger,
                    auto_setup=True,
                )
            )

        installer.install_zig.assert_not_called()
        installer.install_cargo_zigbuild.assert_not_called()
        executor.build_all.assert_not_called()

    def test_ci_debug_rejects_missing_cross_tools_without_installing(self):
        config = _Config(release=False)
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        installer.is_zig_installed.return_value = False
        installer.is_cargo_zigbuild_installed.return_value = False
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()

        with patch.dict(os.environ, {"CI": "true"}, clear=True), patch(
            "builder.executor.ToolInstaller", return_value=installer
        ), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch(
            "builder.executor.BuildExecutor", return_value=executor
        ):
            self.assertFalse(
                run_build(
                    config,
                    Path("/project"),
                    ["x86_64-unknown-linux-gnu"],
                    logger,
                    auto_setup=True,
                )
            )

        installer.install_zig.assert_not_called()
        installer.install_cargo_zigbuild.assert_not_called()
        executor.build_all.assert_not_called()

    def test_local_debug_can_install_missing_cross_tools(self):
        config = _Config(release=False)
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        installer.is_zig_installed.return_value = False
        installer.is_cargo_zigbuild_installed.return_value = False
        installer.install_zig.return_value = True
        installer.install_cargo_zigbuild.return_value = True
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-unknown-linux-gnu", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()
        executor.build_all.return_value = [
            BuildResult(target, True, Path("/project/target/cokacmux"))
        ]
        executor.copy_to_dist.return_value = [
            (Path("/project/dist/cokacmux-linux-x86_64"), "1.0MB")
        ]

        with patch.dict(os.environ, {}, clear=True), patch(
            "builder.executor.ToolInstaller", return_value=installer
        ), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch(
            "builder.executor.BuildExecutor", return_value=executor
        ):
            self.assertTrue(
                run_build(
                    config,
                    Path("/project"),
                    ["x86_64-unknown-linux-gnu"],
                    logger,
                    auto_setup=True,
                )
            )

        installer.install_zig.assert_called_once_with()
        installer.install_cargo_zigbuild.assert_called_once_with()
        executor.build_all.assert_called_once_with([target], auto_setup=True)

    def test_failed_requested_clean_aborts_after_resolving_and_preflighting_targets(self):
        config = _Config(host_os="macos", clean=True)
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        installer.get_pinned_rustup_path.return_value = Path("/verified/rustup")
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-apple-darwin", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()
        executor.clean.return_value = False

        with patch("builder.executor.ToolInstaller", return_value=installer), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch("builder.executor.BuildExecutor", return_value=executor):
            self.assertFalse(run_build(config, Path("/project"), ["native"], logger))

        executor.clean.assert_called_once_with(auto_setup=False)
        manager.resolve_targets.assert_called_once_with(
            ["native"],
            allow_system_probe=False,
        )
        executor.build_all.assert_not_called()

    def test_partial_multi_target_build_does_not_publish_successful_subset(self):
        config = _Config(host_os="macos", release=False)
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        manager = MagicMock()
        targets = [
            Target.from_rust_target("x86_64-apple-darwin", config),
            Target.from_rust_target("aarch64-apple-darwin", config),
        ]
        manager.resolve_targets.return_value = targets
        executor = MagicMock()
        executor.build_all.return_value = [
            BuildResult(targets[0], True, Path("/first")),
            BuildResult(targets[1], False, error_message="failed"),
        ]

        with patch.dict(os.environ, {}, clear=True), patch(
            "builder.executor.ToolInstaller", return_value=installer
        ), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch("builder.executor.BuildExecutor", return_value=executor):
            self.assertFalse(run_build(config, Path("/project"), ["all"], logger))

        executor.build_all.assert_called_once_with(targets, auto_setup=True)
        executor.copy_to_dist.assert_not_called()

    def test_distribution_copy_failure_is_not_reported_as_build_complete(self):
        config = _Config(host_os="macos")
        logger = MagicMock()
        installer = MagicMock()
        installer.get_env.return_value = {}
        manager = MagicMock()
        target = Target.from_rust_target("x86_64-apple-darwin", config)
        manager.resolve_targets.return_value = [target]
        executor = MagicMock()
        executor.build_all.return_value = [
            BuildResult(target, True, Path("/first"))
        ]
        executor.copy_to_dist.return_value = []

        with patch("builder.executor.ToolInstaller", return_value=installer), patch(
            "builder.executor.TargetManager", return_value=manager
        ), patch("builder.executor.BuildExecutor", return_value=executor):
            self.assertFalse(run_build(config, Path("/project"), ["native"], logger))

        logger.results.assert_not_called()
        logger.error.assert_called_with(
            "Builds succeeded, but the distribution was not published"
        )


if __name__ == "__main__":
    unittest.main()
