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
