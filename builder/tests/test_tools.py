import hashlib
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, call, patch

from builder.tools import ToolInstaller


class _Config:
    def __init__(self, host_os="linux", host_arch="x86_64"):
        self.host_os = host_os
        self.host_arch = host_arch
        self.tools_dir = Path("builder/tools")
        self.rust_version = "1.96.1"
        self.rustup_version = "1.29.0"
        self.rustup_init_url = "https://example.invalid/rustup-init"
        self.rustup_init_sha256 = hashlib.sha256(b"expected-rustup").hexdigest()
        self.cargo_zigbuild_version = "0.23.0"
        self.cargo_xwin_version = "0.23.0"
        self.zig_version = "0.13.0"
        self.zig_sha256 = hashlib.sha256(b"expected-zig").hexdigest()
        self.macos_sdk_version = "14.0"
        self.macos_sdk_sha256 = hashlib.sha256(b"expected-sdk").hexdigest()
        self.zig_url = "https://example.invalid/zig.tar.xz"
        self.macos_sdk_url = "https://example.invalid/sdk.tar.xz"


class _Response:
    def __init__(self, chunks, content_length):
        self.chunks = iter(chunks)
        self.headers = {"content-length": str(content_length)}

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def read(self, _size):
        return next(self.chunks, b"")


class ToolEnvironmentTests(unittest.TestCase):
    def installer(self, root, config=None):
        return ToolInstaller(config or _Config(), Path(root), MagicMock())

    def test_system_rust_homes_are_not_redirected_to_empty_local_directories(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            original = {
                "PATH": "/system/bin",
                "CARGO_HOME": "/system/cargo",
                "RUSTUP_HOME": "/system/rustup",
                "RUSTUP_AUTO_INSTALL": "1",
            }
            with patch.dict(os.environ, original, clear=True), patch.object(
                installer, "get_zig_path", return_value=None
            ):
                env = installer.get_env()

            self.assertEqual(env["CARGO_HOME"], "/system/cargo")
            self.assertEqual(env["RUSTUP_HOME"], "/system/rustup")
            self.assertEqual(env["RUSTUP_AUTO_INSTALL"], "0")
            self.assertEqual(env["PATH"], "/system/bin")

    def test_complete_local_rust_install_uses_matching_local_homes(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            cargo_bin = installer.cargo_home / "bin"
            cargo_bin.mkdir(parents=True)
            installer._local_cargo_path().touch()
            installer._local_rustup_path().touch()
            installer._local_cargo_path().chmod(0o755)
            installer._local_rustup_path().chmod(0o755)
            with patch.dict(
                os.environ,
                {"PATH": "/system/bin", "RUSTUP_AUTO_INSTALL": "1"},
                clear=True,
            ), patch.object(
                installer, "get_zig_path", return_value=None
            ):
                env = installer.get_env()

            self.assertEqual(env["CARGO_HOME"], str(installer.cargo_home))
            self.assertEqual(env["RUSTUP_HOME"], str(installer.rustup_home))
            self.assertEqual(env["RUSTUP_AUTO_INSTALL"], "0")
            self.assertEqual(env["PATH"].split(os.pathsep)[0], str(cargo_bin))

    def test_non_executable_partial_local_rust_install_is_ignored(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            installer._local_cargo_path().parent.mkdir(parents=True)
            installer._local_cargo_path().touch(mode=0o600)
            with patch(
                "builder.tools.shutil.which",
                side_effect=lambda name, **_kwargs: f"/system/{name}",
            ):
                self.assertIsNone(installer.get_cargo_path())
                self.assertEqual(installer.get_rustup_path(), Path("/system/rustup"))
                self.assertFalse(installer.is_rust_installed())

    def test_system_cargo_is_selected_beside_rustup_proxy(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            rustup_bin = Path(root) / "system-rustup-bin"
            rustup_bin.mkdir()
            rustup = rustup_bin / installer._exe_name("rustup")
            cargo_proxy = rustup_bin / installer._exe_name("cargo")
            rustup.touch(mode=0o755)
            cargo_proxy.touch(mode=0o755)

            def which(name, **_kwargs):
                if name == "rustup":
                    return str(rustup)
                if name == "cargo":
                    return "/usr/bin/cargo"
                return None

            with patch("builder.tools.shutil.which", side_effect=which):
                self.assertEqual(installer.get_cargo_path(), cargo_proxy)
                self.assertEqual(installer.get_rustup_path(), rustup)

    def test_noninteractive_rustup_requires_matching_local_bootstrap_hash(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            cargo = installer._local_cargo_path()
            rustup = installer._local_rustup_path()
            cargo.parent.mkdir(parents=True)
            cargo.write_bytes(b"expected-rustup")
            rustup.write_bytes(b"expected-rustup")
            cargo.chmod(0o755)
            rustup.chmod(0o755)

            self.assertEqual(installer.get_pinned_rustup_path(), rustup)

            rustup.write_bytes(b"tampered-rustup")
            self.assertIsNone(installer.get_pinned_rustup_path())

    def test_system_rustup_is_never_a_pinned_release_rustup(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch(
                "builder.tools.shutil.which",
                side_effect=lambda name, **_kwargs: f"/system/{name}",
            ) as which:
                self.assertIsNone(installer.get_pinned_rustup_path())
            which.assert_not_called()

    def test_rust_environment_is_official_and_target_aware(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            zig = Path(root) / "zig" / "zig"
            sdk = installer.sdk_dir
            with patch.dict(
                os.environ,
                {
                    "PATH": "/system/bin",
                    "RUSTUP_DIST_SERVER": "https://mirror.invalid",
                    "RUSTUP_UPDATE_ROOT": "https://mirror.invalid/rustup",
                    "RUSTUP_TOOLCHAIN": "nightly",
                    "SDKROOT": "/stale/sdk",
                },
                clear=True,
            ), patch.object(
                installer,
                "get_zig_path",
                return_value=zig,
            ) as get_zig, patch.object(
                installer,
                "is_macos_sdk_installed",
                return_value=True,
            ) as has_sdk:
                rust_only = installer.get_env()
                cross = installer.get_env(
                    include_zig=True,
                    include_macos_sdk=True,
                )

            self.assertEqual(
                rust_only["RUSTUP_DIST_SERVER"],
                "https://static.rust-lang.org",
            )
            self.assertEqual(
                rust_only["RUSTUP_UPDATE_ROOT"],
                "https://static.rust-lang.org/rustup",
            )
            self.assertNotIn("RUSTUP_TOOLCHAIN", rust_only)
            self.assertNotIn("SDKROOT", rust_only)
            self.assertEqual(cross["SDKROOT"], str(sdk))
            self.assertEqual(cross["PATH"].split(os.pathsep)[0], str(zig.parent))
            get_zig.assert_called_once_with()
            has_sdk.assert_called_once_with()

    def test_read_only_check_does_not_install_missing_release_toolchain(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "_rust_toolchain_status", return_value=False
            ), patch.object(
                installer, "install_rust_toolchain", return_value=True
            ) as install:
                self.assertFalse(
                    installer._ensure_default_toolchain(install_if_missing=False)
                )

            install.assert_not_called()

    def test_release_toolchain_install_does_not_change_global_default(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "_rust_toolchain_status", return_value=False
            ), patch.object(
                installer, "install_rust_toolchain", return_value=True
            ) as install:
                self.assertTrue(installer._ensure_default_toolchain())

            install.assert_called_once_with("1.96.1")

    def test_rustup_query_failure_does_not_trigger_toolchain_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "get_rustup_path", return_value=Path("/rustup")
            ), patch.object(installer, "get_env", return_value={}), patch.object(
                installer, "install_rust_toolchain", return_value=True
            ) as install, patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(
                    returncode=1,
                    stdout="",
                    stderr="permission denied",
                ),
            ) as run:
                self.assertFalse(installer._ensure_default_toolchain())

            self.assertEqual(run.call_count, 1)
            install.assert_not_called()

    def test_release_toolchain_identity_requires_exact_rustc_and_cargo(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer,
                "get_pinned_rustup_path",
                return_value=Path("/receipted/rustup"),
            ), patch.object(installer, "get_env", return_value={}), patch(
                "builder.tools.subprocess.run",
                side_effect=[
                    MagicMock(returncode=0, stdout="rustc 1.96.1 (hash)\n", stderr=""),
                    MagicMock(returncode=0, stdout="cargo 1.96.1 (hash)\n", stderr=""),
                ],
            ) as run:
                self.assertTrue(installer.verify_release_toolchain())

            self.assertEqual(
                [logged.args[0] for logged in run.call_args_list],
                [
                    [
                        "/receipted/rustup",
                        "run",
                        "1.96.1",
                        "rustc",
                        "--version",
                    ],
                    [
                        "/receipted/rustup",
                        "run",
                        "1.96.1",
                        "cargo",
                        "--version",
                    ],
                ],
            )

            with patch.object(
                installer,
                "get_pinned_rustup_path",
                return_value=Path("/receipted/rustup"),
            ), patch.object(installer, "get_env", return_value={}), patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(
                    returncode=0,
                    stdout="rustc 1.95.0 (wrong)\n",
                    stderr="",
                ),
            ):
                self.assertFalse(installer.verify_release_toolchain())

    def test_toolchain_install_refuses_ambiguous_inspection_failure(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "_rust_toolchain_status", return_value=None
            ), patch("builder.tools.subprocess.run") as run:
                self.assertFalse(installer.install_rust_toolchain("1.96.1"))

            run.assert_not_called()

    def test_target_install_refuses_ambiguous_inspection_failure(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "get_rustup_path", return_value=Path("/rustup")
            ), patch.object(installer, "get_env", return_value={}), patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(
                    returncode=1,
                    stdout="",
                    stderr="permission denied",
                ),
            ) as run:
                self.assertFalse(
                    installer.install_rust_target(
                        "aarch64-pc-windows-gnullvm",
                        "1.96.1",
                    )
                )

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

    def test_target_install_adds_std_to_native_release_toolchain(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            with patch.object(
                installer, "_rust_target_status", return_value=False
            ), patch.object(
                installer, "get_rustup_path", return_value=Path("/rustup")
            ), patch.object(installer, "get_env", return_value={}), patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ) as run:
                self.assertTrue(
                    installer.install_rust_target(
                        "aarch64-pc-windows-gnullvm",
                        "1.96.1",
                    )
                )

            self.assertEqual(
                run.call_args.args[0],
                [
                    "/rustup",
                    "target",
                    "add",
                    "aarch64-pc-windows-gnullvm",
                    "--toolchain",
                    "1.96.1",
                ],
            )

    def test_local_rust_bootstrap_uses_exact_release_toolchain(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)

            def fake_download(_url, destination, _desc, **_kwargs):
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text("#!/bin/sh\n", encoding="utf-8")
                return True

            with patch.object(
                installer,
                "is_pinned_rust_installed",
                side_effect=[False, True],
            ), patch.object(
                installer,
                "verify_release_toolchain",
                return_value=True,
            ), patch.object(
                installer, "download_file", side_effect=fake_download
            ) as download, patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ) as run:
                self.assertTrue(installer.install_rust())

            download.assert_called_once_with(
                "https://example.invalid/rustup-init",
                installer.tools_dir / "rustup-init",
                "rustup installer",
                expected_sha256=installer.config.rustup_init_sha256,
            )
            command = run.call_args.args[0]
            self.assertEqual(command[-2:], ["--default-toolchain", "1.96.1"])
            self.assertNotIn("stable", command)
            self.assertEqual(run.call_args.kwargs["env"]["RUSTUP_AUTO_INSTALL"], "0")

    def test_rustup_checksum_mismatch_never_executes_installer(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            response = _Response([b"tampered"], content_length=8)
            with patch.object(
                installer, "is_rust_installed", return_value=False
            ), patch(
                "builder.tools.urllib.request.urlopen", return_value=response
            ), patch("builder.tools.subprocess.run") as run:
                self.assertFalse(installer.install_rust())

            run.assert_not_called()
            self.assertFalse((installer.tools_dir / "rustup-init").exists())

    def test_rustup_success_without_receipted_local_pair_is_failure(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)

            def fake_download(_url, destination, _desc, **_kwargs):
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(b"expected-rustup")
                return True

            with patch.object(
                installer,
                "is_pinned_rust_installed",
                return_value=False,
            ), patch.object(
                installer,
                "download_file",
                side_effect=fake_download,
            ), patch(
                "builder.tools.subprocess.run",
                return_value=MagicMock(returncode=0, stdout="", stderr=""),
            ):
                self.assertFalse(installer.install_rust())

    def test_windows_setup_adds_gnullvm_targets_to_native_toolchain(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root, _Config(host_os="windows"))
            with patch.object(
                installer, "install_zig", return_value=True
            ), patch.object(
                installer, "install_rust_target", return_value=True
            ) as install_target, patch.object(
                installer, "install_windows_import_libs", return_value=True
            ), patch.object(installer, "install_rust_toolchain") as install_toolchain:
                self.assertTrue(installer.setup_windows_cross())

            self.assertEqual(
                install_target.call_args_list,
                [
                    call(
                        "aarch64-pc-windows-gnullvm",
                        "1.96.1",
                    ),
                    call(
                        "x86_64-pc-windows-gnullvm",
                        "1.96.1",
                    ),
                ],
            )
            install_toolchain.assert_not_called()


class CargoToolPinTests(unittest.TestCase):
    def installer(self, root):
        return ToolInstaller(_Config(), Path(root), MagicMock())

    def test_wrong_cargo_tool_versions_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            for executable_name, check_method, reported_name in (
                (
                    "cargo-zigbuild",
                    installer.is_cargo_zigbuild_installed,
                    "cargo-zigbuild",
                ),
                ("cargo-xwin", installer.is_cargo_xwin_installed, "cargo-xwin"),
            ):
                executable = installer.cargo_home / "bin" / executable_name
                executable.parent.mkdir(parents=True, exist_ok=True)
                executable.touch(mode=0o755)
                with self.subTest(executable=executable_name), patch.object(
                    installer, "get_env", return_value={}
                ), patch(
                    "builder.tools.subprocess.run",
                    return_value=MagicMock(
                        returncode=0,
                        stdout=f"{reported_name} 0.22.0\n",
                        stderr="",
                    ),
                ):
                    self.assertFalse(check_method())

    def test_cargo_tool_installs_use_exact_versions_and_locked_sources(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            cases = (
                (
                    "cargo-zigbuild",
                    "install_cargo_zigbuild",
                    "is_cargo_zigbuild_installed",
                ),
                ("cargo-xwin", "install_cargo_xwin", "is_cargo_xwin_installed"),
            )
            for package, install_name, check_name in cases:
                with self.subTest(package=package), patch.object(
                    installer, check_name, side_effect=[False, True]
                ), patch.object(
                    installer, "is_rust_installed", return_value=True
                ), patch.object(
                    installer, "_ensure_default_toolchain", return_value=True
                ), patch.object(
                    installer, "get_env", return_value={}
                ), patch.object(
                    installer, "get_rustup_path", return_value=Path("/rustup")
                ), patch(
                    "builder.tools.subprocess.run",
                    return_value=MagicMock(returncode=0, stdout="", stderr=""),
                ) as run:
                    self.assertTrue(getattr(installer, install_name)())

                self.assertEqual(
                    run.call_args.args[0],
                    [
                        "/rustup",
                        "run",
                        "1.96.1",
                        "cargo",
                        "install",
                        package,
                        "--version",
                        "=0.23.0",
                        "--locked",
                    ],
                )
                self.assertNotIn("stable", run.call_args.args[0])


class ToolDownloadTests(unittest.TestCase):
    def installer(self, root):
        return ToolInstaller(_Config(), Path(root), MagicMock())

    def test_incomplete_download_preserves_existing_destination(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            destination = Path(root) / "archive"
            destination.write_bytes(b"known-good")
            response = _Response([b"ab"], content_length=5)
            with patch("builder.tools.urllib.request.urlopen", return_value=response):
                self.assertFalse(
                    installer.download_file(
                        "https://example.invalid/archive", destination, "archive"
                    )
                )

            self.assertEqual(destination.read_bytes(), b"known-good")
            self.assertEqual(list(Path(root).glob(".archive.*.download")), [])

    def test_complete_download_replaces_destination_atomically(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            destination = Path(root) / "archive"
            destination.write_bytes(b"old")
            response = _Response([b"new"], content_length=3)
            with patch("builder.tools.urllib.request.urlopen", return_value=response):
                self.assertTrue(
                    installer.download_file(
                        "https://example.invalid/archive", destination, "archive"
                    )
                )

            self.assertEqual(destination.read_bytes(), b"new")

    def test_download_checksum_mismatch_preserves_existing_archive(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            destination = Path(root) / "archive"
            destination.write_bytes(b"known-good")
            response = _Response([b"tampered"], content_length=8)
            expected = hashlib.sha256(b"expected").hexdigest()
            with patch("builder.tools.urllib.request.urlopen", return_value=response):
                self.assertFalse(
                    installer.download_file(
                        "https://example.invalid/archive",
                        destination,
                        "archive",
                        expected_sha256=expected,
                    )
                )

            self.assertEqual(destination.read_bytes(), b"known-good")
            self.assertEqual(list(Path(root).glob(".archive.*.download")), [])

    def test_failed_cached_archive_copy_preserves_existing_destination(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            destination = Path(root) / "archive"
            destination.write_bytes(b"known-good")

            self.assertFalse(
                installer._copy_file_atomically(Path(root) / "missing", destination)
            )

            self.assertEqual(destination.read_bytes(), b"known-good")
            self.assertEqual(list(Path(root).glob(".archive.*.copy")), [])

    def test_cached_checksum_mismatch_preserves_existing_archive(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            source = Path(root) / "cached"
            destination = Path(root) / "archive"
            source.write_bytes(b"tampered")
            destination.write_bytes(b"known-good")
            expected = hashlib.sha256(b"expected").hexdigest()

            self.assertFalse(
                installer._copy_file_atomically(
                    source,
                    destination,
                    expected_sha256=expected,
                )
            )

            self.assertEqual(destination.read_bytes(), b"known-good")
            self.assertEqual(list(Path(root).glob(".archive.*.copy")), [])

    def test_windows_and_posix_archive_traversal_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            installer = self.installer(root)
            for name in (
                "../outside",
                "..\\outside",
                "/absolute",
                "C:\\absolute\\file",
                "\\\\server\\share\\file",
            ):
                with self.subTest(name=name):
                    self.assertTrue(installer._is_unsafe_archive_name(name))


class ZigArchiveTests(unittest.TestCase):
    def test_interrupted_commit_with_backup_only_restores_prior_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            backup = installer._staged_directory_backup_path(installer.zig_dir)
            backup.mkdir()
            marker = backup / "prior-install"
            marker.write_bytes(b"preserve")

            self.assertTrue(
                installer._recover_staged_directory_backup(installer.zig_dir, "Zig")
            )

            self.assertEqual(
                (installer.zig_dir / "prior-install").read_bytes(), b"preserve"
            )
            self.assertFalse(backup.exists())

    def test_interrupted_commit_with_new_destination_cleans_old_backup(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            installer.zig_dir.mkdir()
            (installer.zig_dir / "new-install").write_bytes(b"new")
            backup = installer._staged_directory_backup_path(installer.zig_dir)
            backup.mkdir()
            (backup / "prior-install").write_bytes(b"old")

            self.assertTrue(
                installer._recover_staged_directory_backup(installer.zig_dir, "Zig")
            )

            self.assertEqual(
                (installer.zig_dir / "new-install").read_bytes(), b"new"
            )
            self.assertFalse(backup.exists())

    def test_existing_checksum_mismatch_preserves_archive_and_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            archive = installer.tools_dir / "zig-linux-x86_64-0.13.0.tar.xz"
            archive.write_bytes(b"tampered")
            installer.zig_dir.mkdir()
            marker = installer.zig_dir / "prior-install"
            marker.write_bytes(b"preserve")

            with patch.object(
                installer, "get_zig_path", return_value=None
            ), patch.object(installer, "extract_tar_xz") as extract:
                self.assertFalse(installer.install_zig())

            extract.assert_not_called()
            self.assertEqual(archive.read_bytes(), b"tampered")
            self.assertEqual(marker.read_bytes(), b"preserve")

    def test_commit_failure_restores_prior_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            archive = installer.tools_dir / "zig-linux-x86_64-0.13.0.tar.xz"
            archive.write_bytes(b"expected-zig")
            installer.zig_dir.mkdir()
            marker = installer.zig_dir / "prior-install"
            marker.write_bytes(b"preserve")

            def fake_extract(_archive, destination):
                extracted = destination / "zig-linux-x86_64-0.13.0"
                extracted.mkdir()
                (extracted / installer._exe_name("zig")).write_bytes(b"new-zig")
                return True

            original_rename = Path.rename

            def fail_staged_commit(source, target):
                if (
                    Path(target) == installer.zig_dir
                    and source.parent != installer.tools_dir
                ):
                    raise OSError("injected commit failure")
                return original_rename(source, target)

            with patch.object(
                installer, "get_zig_path", return_value=None
            ), patch.object(
                installer, "extract_tar_xz", side_effect=fake_extract
            ), patch.object(
                installer, "_zig_version_matches", return_value=True
            ), patch.object(Path, "rename", new=fail_staged_commit):
                self.assertFalse(installer.install_zig())

            self.assertEqual(marker.read_bytes(), b"preserve")
            self.assertEqual(list(installer.tools_dir.glob(".*.backup")), [])


class MacSdkTests(unittest.TestCase):
    def test_partial_sdk_directory_is_not_reported_as_installed(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.sdk_dir.mkdir(parents=True)
            (installer.sdk_dir / "SDKSettings.json").touch()
            self.assertFalse(installer.is_macos_sdk_installed())

    def test_existing_checksum_mismatch_preserves_archive_and_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            archive = installer.tools_dir / "MacOSX14.0.sdk.tar.xz"
            archive.write_bytes(b"tampered")
            installer.sdk_dir.mkdir()
            marker = installer.sdk_dir / "prior-install"
            marker.write_bytes(b"preserve")

            with patch.object(installer, "extract_tar_xz") as extract:
                self.assertFalse(installer.install_macos_sdk())

            extract.assert_not_called()
            self.assertEqual(archive.read_bytes(), b"tampered")
            self.assertEqual(marker.read_bytes(), b"preserve")

    def test_commit_failure_restores_prior_install(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(), Path(root), MagicMock())
            installer.tools_dir.mkdir(parents=True)
            archive = installer.tools_dir / "MacOSX14.0.sdk.tar.xz"
            archive.write_bytes(b"expected-sdk")
            installer.sdk_dir.mkdir()
            marker = installer.sdk_dir / "prior-install"
            marker.write_bytes(b"preserve")

            def fake_extract(_archive, destination):
                extracted = destination / installer.sdk_dir.name
                (extracted / "usr" / "lib").mkdir(parents=True)
                (extracted / "SDKSettings.json").write_text("{}", encoding="utf-8")
                return True

            original_rename = Path.rename

            def fail_staged_commit(source, target):
                if (
                    Path(target) == installer.sdk_dir
                    and source.parent != installer.tools_dir
                ):
                    raise OSError("injected commit failure")
                return original_rename(source, target)

            with patch.object(
                installer, "extract_tar_xz", side_effect=fake_extract
            ), patch.object(Path, "rename", new=fail_staged_commit):
                self.assertFalse(installer.install_macos_sdk())

            self.assertEqual(marker.read_bytes(), b"preserve")
            self.assertEqual(list(installer.tools_dir.glob(".*.backup")), [])

    def test_windows_host_attempts_to_install_sdk_for_macos_cross_builds(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(_Config(host_os="windows"), Path(root), MagicMock())
            installer.download_file = MagicMock(return_value=False)

            self.assertFalse(installer.install_macos_sdk())
            installer.download_file.assert_called_once()


class WindowsImportLibraryTests(unittest.TestCase):
    def test_archive_validation_rejects_partial_files(self):
        with tempfile.TemporaryDirectory() as root:
            installer = ToolInstaller(
                _Config(host_os="windows"), Path(root), MagicMock()
            )
            archive = Path(root) / "library.a"

            archive.write_bytes(b"partial")
            self.assertFalse(installer._is_valid_archive(archive, allow_empty=True))

            archive.write_bytes(b"!<arch>\n")
            self.assertTrue(installer._is_valid_archive(archive, allow_empty=True))
            self.assertFalse(installer._is_valid_archive(archive, allow_empty=False))

            archive.write_bytes(b"!<arch>\nmember")
            self.assertTrue(installer._is_valid_archive(archive, allow_empty=False))


if __name__ == "__main__":
    unittest.main()
