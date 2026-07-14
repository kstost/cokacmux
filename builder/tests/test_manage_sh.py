import os
import subprocess
import tempfile
import unittest
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parents[2]


class ManageShTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.install_dir = self.root / "install"
        self.fake_bin = self.root / "fake-bin"
        self.home.mkdir()
        self.install_dir.mkdir()
        self.fake_bin.mkdir()

        curl = self.fake_bin / "curl"
        curl.write_text(
            """#!/bin/sh
url=''
out=''
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; out="$1" ;;
        http*) url="$1" ;;
    esac
    shift
done
case "$url" in
    *cokacdir*) cp "$FAKE_COKACDIR_PAYLOAD" "$out" ;;
    *) cp "$FAKE_COKACMUX_PAYLOAD" "$out" ;;
esac
""",
            encoding="utf-8",
        )
        curl.chmod(0o755)

    def payload(self, name, marker, success=True, product="cokacmux"):
        path = self.root / name
        exit_code = 0 if success else 1
        path.write_text(
            f"#!/bin/sh\n# {marker}\n"
            f"if [ \"${{1:-}}\" = --version ]; then echo '{product} 1.0'; exit {exit_code}; fi\n"
            f"exit {exit_code}\n",
            encoding="utf-8",
        )
        path.chmod(0o755)
        return path

    def run_installer(self, app_payload, helper_payload, extra_env=None):
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(self.home),
                "SHELL": "/bin/false",
                "PATH": f"{self.fake_bin}:/usr/bin:/bin",
                "COKACMUX_INSTALL_DIR": str(self.install_dir),
                "COKACMUX_BASE_URL": "https://example.invalid/cokacmux",
                "COKACDIR_BASE_URL": "https://example.invalid/cokacdir",
                "FAKE_COKACMUX_PAYLOAD": str(app_payload),
                "FAKE_COKACDIR_PAYLOAD": str(helper_payload),
            }
        )
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["bash", str(PROJECT_ROOT / "manage.sh")],
            capture_output=True,
            text=True,
            env=env,
        )

    def test_valid_downloads_replace_both_programs(self):
        app = self.payload("new-app", "new-app")
        helper = self.payload("new-helper", "new-helper", product="cokacdir")

        result = self.run_installer(app, helper)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("new-app", (self.install_dir / "cokacmux").read_text())
        self.assertIn(
            "new-helper",
            (self.home / ".cokacmux" / "bin" / "cokacdir").read_text(),
        )
        self.assertEqual(list(self.install_dir.glob(".cokacmux.*")), [])

    def test_invalid_helper_download_preserves_existing_install(self):
        app_dest = self.install_dir / "cokacmux"
        helper_dest = self.home / ".cokacmux" / "bin" / "cokacdir"
        helper_dest.parent.mkdir(parents=True)
        app_dest.write_text("old-app", encoding="utf-8")
        helper_dest.write_text("old-helper", encoding="utf-8")
        app = self.payload("new-app", "new-app")
        helper = self.payload(
            "bad-helper", "bad-helper", success=False, product="cokacdir"
        )

        result = self.run_installer(app, helper)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(app_dest.read_text(encoding="utf-8"), "old-app")
        self.assertEqual(helper_dest.read_text(encoding="utf-8"), "old-helper")

    def test_wrong_binary_at_helper_url_is_rejected_before_install(self):
        app_dest = self.install_dir / "cokacmux"
        helper_dest = self.home / ".cokacmux" / "bin" / "cokacdir"
        helper_dest.parent.mkdir(parents=True)
        app_dest.write_text("old-app", encoding="utf-8")
        helper_dest.write_text("old-helper", encoding="utf-8")
        app = self.payload("new-app", "new-app")
        wrong_helper = self.payload("wrong-helper", "wrong-helper", product="cokacmux")

        result = self.run_installer(app, wrong_helper)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(app_dest.read_text(encoding="utf-8"), "old-app")
        self.assertEqual(helper_dest.read_text(encoding="utf-8"), "old-helper")

    def test_directory_destination_is_rejected_without_touching_pair(self):
        app_dest = self.install_dir / "cokacmux"
        helper_dest = self.home / ".cokacmux" / "bin" / "cokacdir"
        app_dest.mkdir()
        (app_dest / "keep.txt").write_text("keep", encoding="utf-8")
        helper_dest.parent.mkdir(parents=True)
        helper_dest.write_text("old-helper", encoding="utf-8")
        app = self.payload("new-app", "new-app")
        helper = self.payload("new-helper", "new-helper", product="cokacdir")

        result = self.run_installer(app, helper)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((app_dest / "keep.txt").read_text(encoding="utf-8"), "keep")
        self.assertEqual(helper_dest.read_text(encoding="utf-8"), "old-helper")

    def test_second_commit_failure_restores_both_previous_programs(self):
        app_dest = self.install_dir / "cokacmux"
        helper_dest = self.home / ".cokacmux" / "bin" / "cokacdir"
        helper_dest.parent.mkdir(parents=True)
        app_dest.write_text("old-app", encoding="utf-8")
        helper_dest.write_text("old-helper", encoding="utf-8")
        app = self.payload("new-app", "new-app")
        helper = self.payload("new-helper", "new-helper", product="cokacdir")
        fail_marker = self.root / "mv-failed-once"
        fake_mv = self.fake_bin / "mv"
        fake_mv.write_text(
            """#!/bin/sh
last=''
for arg in "$@"; do last="$arg"; done
if [ "${FAIL_APP_COMMIT_ONCE:-}" = 1 ] && \
   [ "$last" = "$COKACMUX_INSTALL_DIR/cokacmux" ] && \
   [ ! -e "$FAIL_APP_COMMIT_MARKER" ]; then
    : > "$FAIL_APP_COMMIT_MARKER"
    exit 1
fi
exec /bin/mv "$@"
""",
            encoding="utf-8",
        )
        fake_mv.chmod(0o755)

        result = self.run_installer(
            app,
            helper,
            {
                "FAIL_APP_COMMIT_ONCE": "1",
                "FAIL_APP_COMMIT_MARKER": str(fail_marker),
            },
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(app_dest.read_text(encoding="utf-8"), "old-app")
        self.assertEqual(helper_dest.read_text(encoding="utf-8"), "old-helper")
        self.assertEqual(list(self.install_dir.glob(".cokacmux.*")), [])
        self.assertEqual(list(helper_dest.parent.glob(".cokacdir.*")), [])

    def test_post_install_validation_failure_restores_both_previous_programs(self):
        app_dest = self.install_dir / "cokacmux"
        helper_dest = self.home / ".cokacmux" / "bin" / "cokacdir"
        helper_dest.parent.mkdir(parents=True)
        app_dest.write_text("old-app", encoding="utf-8")
        helper_dest.write_text("old-helper", encoding="utf-8")
        app = self.payload("new-app", "new-app")
        helper = self.root / "location-sensitive-helper"
        helper.write_text(
            """#!/bin/sh
if [ "${1:-}" = --version ]; then
    echo 'cokacdir 1.0'
    case "$0" in */.cokacmux/bin/cokacdir) exit 1 ;; esac
    exit 0
fi
exit 0
""",
            encoding="utf-8",
        )
        helper.chmod(0o755)

        result = self.run_installer(app, helper)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(app_dest.read_text(encoding="utf-8"), "old-app")
        self.assertEqual(helper_dest.read_text(encoding="utf-8"), "old-helper")
        self.assertEqual(list(self.install_dir.glob(".cokacmux.*")), [])
        self.assertEqual(list(helper_dest.parent.glob(".cokacdir.*")), [])


if __name__ == "__main__":
    unittest.main()
