"""Synthetic negative framing tests only. No macOS golden samples are included."""
import io
import unittest

from capture_media import inspect_stream


class IncompleteCapture(unittest.TestCase):
    def test_incomplete_length_or_body(self):
        for bad in (b'\0', b'\0\0\0\x10\x05', b'\0\0\0\0', b'\xff' * 4):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                inspect_stream(io.BytesIO(bad), io.BytesIO(), False)

    def test_missing_audio(self):
        with self.assertRaisesRegex(ValueError, 'missing'):
            inspect_stream(io.BytesIO(), io.BytesIO(), True)

    def test_audio_needs_payload(self):
        with self.assertRaisesRegex(ValueError, 'audio'):
            inspect_stream(io.BytesIO(b'\0\0\0\x05\x06\0\0\0\0'), io.BytesIO(), False)

    def test_demux_preserves_framing_and_accepts_audio_before_cookie(self):
        # Minimal synthetic records exercise byte copying only; never use as a codec fixture.
        import struct

        def framed(kind, payload):
            return struct.pack(">I", len(payload) + 1) + bytes([kind]) + payload

        aac = framed(6, struct.pack(">I", 1) + b'x')
        fmt = framed(5, struct.pack(">IIHI", 1, 48000, 2, 512) + b'x')
        unit = b'\0\0\0\x01x'
        data = aac + framed(1, unit + unit) + fmt + framed(2, unit)
        audio = io.BytesIO()
        metadata = inspect_stream(io.BytesIO(data), audio, False)
        self.assertEqual(audio.getvalue(), aac + fmt)
        self.assertEqual(metadata['packets_before_first_format'], 1)
        self.assertEqual(metadata['audio_format']['sample_rate'], 48000)
        self.assertIsNone(metadata['stream_info'])
