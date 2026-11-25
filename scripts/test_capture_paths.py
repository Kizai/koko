"""Disposable path and lifecycle tests. These contain no macOS protocol samples."""
import hashlib
import os
from pathlib import Path
import plistlib
import stat
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import capture_session as capture


class CapturePaths(unittest.TestCase):
    def test_output_mode_and_existing_output(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory).resolve()
            parent.chmod(0o755)
            old = os.umask(0)
            try:
                output = parent / "capture"
                self.assertEqual(capture.new_output(output), output)
                self.assertEqual(stat.S_IMODE(output.stat().st_mode), 0o700)
                for path in (output, parent / "alias"):
                    if not path.exists():
                        path.symlink_to(output)
                    with self.assertRaises(capture.CaptureError):
                        capture.new_output(path)
                self.assertEqual(stat.S_IMODE(parent.stat().st_mode), 0o755)
            finally:
                os.umask(old)

    def test_default_output_is_new_and_under_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            with patch.object(capture, "ROOT", root):
                first, second = capture.new_output(None), capture.new_output(None)
                self.assertNotEqual(first, second)
                self.assertEqual(first.parent, root / "target")
                self.assertEqual(stat.S_IMODE(first.stat().st_mode), 0o700)

    def test_reject_local_path_symlinks_before_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            installed = root / "Applications/Koko.app"
            installed.mkdir(parents=True)
            app = root / "target/koko-macos/local/Koko.app"
            with patch.object(capture, "ROOT", root), patch.object(capture, "APP", app):
                path = root
                for part in ("target", "koko-macos", "local", "Koko.app", "Contents", "Helpers", "koko-daemon"):
                    path /= part
                    path.symlink_to(installed)
                    with self.assertRaises(capture.CaptureError):
                        capture.check_build_paths()
                    path.unlink()
                    path.mkdir()

    def test_framework_links_stay_inside_framework(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app = root / "target/koko-macos/local/Koko.app"
            framework = app / "Contents/Frameworks/Sparkle.framework"
            (framework / "Versions/B/Resources").mkdir(parents=True)
            (framework / "Versions/Current").symlink_to("B")
            resource = framework / "Resources"
            resource.symlink_to("Versions/Current/Resources")
            with patch.object(capture, "ROOT", root), patch.object(capture, "APP", app):
                capture.check_build_paths()
                resource.unlink()
                resource.symlink_to("/Applications/Koko.app")
                with self.assertRaises(capture.CaptureError):
                    capture.check_build_paths()

    def test_source_daemon_hash_survives_normal_and_interrupted_wrapping(self):
        # Exercise the same ditto, signing and wrapper installation used by the entry point.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            app = root / "target/koko-macos/local/Koko.app"
            macos = app / "Contents/MacOS"
            helpers = app / "Contents/Helpers"
            macos.mkdir(parents=True)
            helpers.mkdir()
            daemon = helpers / "koko-daemon"
            subprocess.run(["cc", "-x", "c", "-", "-o", str(daemon)], input=b"int main(void){return 0;}", check=True)
            import shutil
            shutil.copyfile(daemon, macos / "Koko")
            (macos / "Koko").chmod(0o700)
            (app / "Contents/Info.plist").write_bytes(plistlib.dumps(dict(
                KokoAdHocSigned=True, CFBundleIdentifier="test.koko", CFBundleExecutable="Koko",
                CFBundlePackageType="APPL", CFBundleShortVersionString="0")))
            for path in (daemon, app):
                subprocess.run(["codesign", "--force", "--sign", "-", str(path)], check=True, capture_output=True)
            expected = hashlib.sha256(daemon.read_bytes()).digest()
            (root / "scripts").mkdir()
            shutil.copyfile(Path(__file__).with_name("capture_bridge.py"), root / "scripts/capture_bridge.py")
            launch = capture.run
            for interrupt in (False, True):
                def command(args, **kwargs):
                    if interrupt and args[:4] == ["codesign", "--force", "--sign", "-"]:
                        raise KeyboardInterrupt
                    return launch(args, **kwargs)
                with patch.object(capture, "ROOT", root), patch.object(capture, "APP", app), patch.object(capture, "run", command):
                    with capture.private_session() as state:
                        self.assertEqual(state.parent, app.parent)
                        self.assertEqual(stat.S_IMODE(state.stat().st_mode), 0o700)
                        env = {key: str(state) for key in ("HOME", "CFFIXED_USER_HOME", "KOKO_CAPTURE_STATE")}
                        if interrupt:
                            with self.assertRaises(KeyboardInterrupt):
                                capture.prepare_capture_app(state, env)
                        else:
                            copy, _ = capture.prepare_capture_app(state, env)
                            self.assertEqual(hashlib.sha256((copy / "Contents/Helpers/koko-daemon-real").read_bytes()).digest(), expected)
                        self.assertEqual(hashlib.sha256(daemon.read_bytes()).digest(), expected)
                    self.assertFalse(state.exists())
                    self.assertEqual(hashlib.sha256(daemon.read_bytes()).digest(), expected)
