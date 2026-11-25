"""Capture-tool isolation regressions with throwaway inputs, not macOS golden data."""
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest


class CaptureIsolation(unittest.TestCase):
    def test_only_disposable_plist_uses_independent_preferences_domain(self):
        from capture_session import configure_capture_plist
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "original.plist"
            original = plistlib.dumps({"CFBundleIdentifier": "io.github.kizai.koko",
                                       "CFBundleName": "Koko", "CFBundleExecutable": "Koko"})
            source.write_bytes(original)
            copy = root / "copy.plist"
            shutil.copyfile(source, copy)
            env = {key: str(root) for key in ("HOME", "CFFIXED_USER_HOME", "KOKO_CAPTURE_STATE")}
            plist = configure_capture_plist(copy, env)
            self.assertEqual(plist, plistlib.loads(copy.read_bytes()))
            self.assertEqual(plist["CFBundleIdentifier"], "io.github.kizai.koko.capture")
            for key in ("CFBundleName", "CFBundleDisplayName"):
                self.assertEqual(plist[key], "Koko Protocol Capture")
            self.assertEqual(plist["CFBundleExecutable"], "Koko")
            self.assertEqual(source.read_bytes(), original)

    def test_custom_code_reaches_neither_child_nor_logs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            script = root / "koko-daemon"
            shutil.copyfile(Path(__file__).with_name("capture_bridge.py"), script)
            real = root / "koko-daemon-real"
            real.write_text("""#!/bin/sh
echo ready
if IFS= read -r line; then
    touch "$KOKO_CAPTURE_STATE/received-input"
fi
""")
            real.chmod(0o700)
            token = b"INVALIDTOOL1234"
            with subprocess.Popen([sys.executable, script, "serve-pair", "unused", "unused",
                                   "--app-desktop", "test"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, env=dict(os.environ, KOKO_CAPTURE_STATE=tmp)) as proc:
                self.assertEqual(proc.stdout.readline(), b"ready\n")
                proc.stdin.write(b"custom-code " + token + b"\n")
                proc.stdin.flush()
                self.assertNotEqual(proc.wait(timeout=5), 0)
                self.assertNotIn(token, proc.stdout.read() + proc.stderr.read())
            self.assertTrue((root / "bridge-error").exists())
            self.assertFalse((root / "received-input").exists())
            for file in root.rglob("*"):
                if file.is_file():
                    self.assertNotIn(token, file.read_bytes())
