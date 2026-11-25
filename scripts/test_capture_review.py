"""Synthetic checks for collector failures. These are not protocol golden data."""
import base64
import io
import json
from pathlib import Path
import tempfile
import unittest

from capture_bridge import Guard
from capture_media import inspect_stream
from capture_session import CaptureError, verify_audio_windows, verify_exchanges, wait_for


class ReviewRegressionTests(unittest.TestCase):
    def test_encoded_image_and_normalized_credential(self):
        with tempfile.TemporaryDirectory() as tmp:
            guard = Guard(Path(tmp))
            guard.observe(b"custom-code Test-1234\n")
            for value in ({"nodes": [{"text": "TEST 1\u2009234"}]},
                          {"status": "frame", "png": base64.b64encode(b"koko://pair/v1/sample").decode()},
                          {"status": "image", "image": base64.b64encode(b"TEST-1234").decode()},
                          {"status": "image", "image": "%%%"}):
                with self.subTest(value=value), self.assertRaises(ValueError):
                    guard.candidate(b"desktop-reply 1 " + json.dumps(value).encode() + b"\n", b"desktop-reply ")

    def test_video_requires_format_and_valid_sequenced_headers(self):
        def frame(kind, body):
            return (len(body) + 1).to_bytes(4, "big") + bytes([kind]) + body
        nal = b'\0\0\0\1x'
        fmt = frame(1, nal * 2)
        for recording, message in ((frame(2, nal), "before format"),
                                   (fmt + frame(9, bytes([0, 0, 0, 0, 2]) + nal), "flags"),
                                   (fmt + frame(9, bytes(5) + nal) +
                                    frame(9, bytes([0, 0, 0, 3, 0]) + nal), "sequence")):
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                inspect_stream(io.BytesIO(recording), io.BytesIO(), True)

    def test_short_audio_substream_and_missing_desktop_exchange(self):
        with self.assertRaises(CaptureError):
            verify_audio_windows([0.1, 0.2], [(0, 1), (2, 3)])
        with self.assertRaises(CaptureError):
            verify_exchanges({b"1": {"action": "displays"}}, {b"1": {"status": "displays"}},
                             [({"action": "screenshot"}, "frame")])

    def test_stale_approval_cannot_satisfy_new_instance(self):
        from unittest.mock import Mock
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            one, two = state / "one", state / "two"
            one.mkdir()
            two.mkdir()
            (one / "approved").touch()
            (state / "current").symlink_to(two)
            with self.assertRaisesRegex(CaptureError, "restarted"):
                wait_for(state, "approved", Mock(), 1, one)

    def test_duplicate_feedback_setting_or_wrong_reply_format_is_incomplete(self):
        for requests, replies, expected in (
                ({1: dict(action="video", video_socket="local"), 2: dict(action="video", video_socket="local")},
                 {1: dict(status="ok"), 2: dict(status="ok")},
                 [(dict(action="video", feedback=False), "ok"), (dict(action="video", feedback=True), "ok")]),
                ({1: dict(action="capture", format="png")}, {1: dict(status="image", format="jpeg", image="AA==")},
                 [(dict(action="capture", format="png"), "image")])):
            with self.subTest(expected=expected), self.assertRaises(CaptureError):
                verify_exchanges(requests, replies, expected)

    def test_shutdown_withdraws_launch_path_and_reaps_private_process(self):
        # Use a disposable sleep executable in a temporary fake bundle. No GUI or Koko.
        from unittest.mock import patch
        from capture_session import test_app
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            executable = state / "Test.app/Contents/MacOS/test"
            executable.parent.mkdir(parents=True)
            executable.write_text("#!/bin/sh\nexec /bin/sleep 60\n")
            executable.chmod(0o700)
            late_attempts = []
            def simulate_launchservices(path):
                if path == executable:
                    late_attempts.append(path.exists())
                return []
            with patch('capture_session.app_pids', side_effect=simulate_launchservices), \
                    patch('capture_session.relaunch_pids', return_value=set()):
                with test_app(executable, state, {}) as process:
                    self.assertIsNone(process.poll())
            self.assertIsNotNone(process.poll())
            self.assertTrue(late_attempts)
            self.assertFalse(any(late_attempts))

    def test_previous_failure_blocks_new_listener_and_final_publication(self):
        from unittest.mock import Mock
        from capture_session import publish
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp)
            second = state / "second"
            second.mkdir()
            (state / "current").symlink_to(second)
            for marker in ("running", "listener.json", "scan-complete"):
                (second / marker).touch()
            (state / "bridge-error").touch()
            app = Mock()
            app.poll.return_value = None
            for name in ("listener.json", "scan-complete"):
                with self.assertRaises(CaptureError):
                    wait_for(state, name, app, 1)
            staged, final = state / "verified", state / "public"
            staged.touch()
            with self.assertRaises(CaptureError):
                publish(state, staged, final)
            self.assertFalse(final.exists())

    def test_unrequested_complete_exchange_is_rejected(self):
        with self.assertRaises(CaptureError):
            verify_exchanges({1: dict(action="audio", video_socket="pipe"), 2: dict(action="text")},
                             {1: dict(status="ok"), 2: dict(status="ok")},
                             [(dict(action="audio"), "ok")])

    def test_restarted_wrappers_keep_separate_trace_files(self):
        import os
        import shutil
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp).resolve()
            wrapper = state / "koko-daemon"
            shutil.copyfile(Path(__file__).with_name("capture_bridge.py"), wrapper)
            real = state / "koko-daemon-real"
            real.write_text("#!/bin/sh\nprintf '%s %s {}\\n' 'Desktop request:' \"$5\"\n")
            real.chmod(0o700)
            logs = []
            for number in ("1", "2"):
                with subprocess.Popen([sys.executable, wrapper, "serve-pair", "unused", "unused",
                                       "--app-desktop", number], env=dict(os.environ, KOKO_CAPTURE_STATE=str(state)),
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE) as proc:
                    self.assertEqual(proc.wait(timeout=5), 0)
                logs.append((state / "current").resolve() / "requests.jsonl")
            self.assertNotEqual(logs[0], logs[1])
            self.assertEqual([path.read_bytes() for path in logs],
                             [b"Desktop request: 1 {}\n", b"Desktop request: 2 {}\n"])
            self.assertFalse((state / "requests.jsonl").exists())
            self.assertFalse((state / "bridge-error").exists())

    def test_real_rejection_survives_real_wrapper_restart(self):
        import os
        import shutil
        import subprocess
        import sys
        from capture_session import publish
        with tempfile.TemporaryDirectory() as tmp:
            state = Path(tmp).resolve()
            wrapper = state / "koko-daemon"
            shutil.copyfile(Path(__file__).with_name("capture_bridge.py"), wrapper)
            real = state / "koko-daemon-real"
            real.write_text("""#!/bin/sh
if [ "$5" = rejected ]; then
    echo 'Desktop request: 1 {"text":"koko://pair/v1/invalid-test-input"}'
else
    echo 'Listening on 127.0.0.1:12345'
    echo 'Device ID: 123456789'
fi
""")
            real.chmod(0o700)
            for mode in ("rejected", "restarted"):
                with subprocess.Popen([sys.executable, wrapper, "serve-pair", "unused", "unused",
                                       "--app-desktop", mode], env=dict(os.environ, KOKO_CAPTURE_STATE=str(state)),
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE) as proc:
                    status = proc.wait(timeout=5)
                if mode == "rejected":
                    self.assertNotEqual(status, 0)
                    self.assertFalse((state / "current/requests.jsonl").exists())
                else:
                    self.assertEqual(status, 0)
                    self.assertTrue((state / "current/listener.json").exists())
            staged, final = state / "staged", state / "published"
            staged.touch()
            with self.assertRaises(CaptureError):
                publish(state, staged, final)
            self.assertFalse(final.exists())
