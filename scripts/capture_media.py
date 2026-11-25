"""Inspect a complete remote-video recording; copy original framed audio bytes."""
from collections import Counter
import json
import struct


def inspect_stream(source, audio, feedback, observe=None):
    counts = Counter()
    info = {}
    next_packet = None
    next_frame = None
    audio_format = None
    early_packets = 0
    while True:
        header = source.read(4)
        if not header:
            break
        size = int.from_bytes(header, "big")
        if len(header) != 4 or not 1 <= size <= 8 * 1024 * 1024:
            raise ValueError("invalid media length")
        record = source.read(size)
        if len(record) != size:
            raise ValueError("incomplete media record")
        kind, payload = record[0], record[1:]
        if kind == 5:
            if len(payload) <= 14:
                raise ValueError("incomplete audio format")
            fmt, rate, channels, frames = struct.unpack(">IIHI", payload[:14])
            audio_format = dict(format_id=fmt, sample_rate=rate, channels=channels,
                                frames_per_packet=frames, cookie_bytes=len(payload) - 14)
            if not counts[5]:
                early_packets = counts[6]
        elif kind == 6:
            sequence = int.from_bytes(payload[:4], "big")
            if (len(payload) <= 4
                    or next_packet is not None and sequence != next_packet):
                raise ValueError("invalid audio packet")
            next_packet = (sequence + 1) % (1 << 32)
        elif kind == 10:
            values = json.loads(payload)
            if not isinstance(values, dict) or not feedback:
                raise ValueError("invalid streamInfo")
            if not counts[10]:
                if (values.get("encoder") not in ("hardware", "software")
                        or values.get("capture") != "ScreenCaptureKit"
                        or type(values.get("bitrate")) is not int):
                    raise ValueError("missing encoder metadata")
                info = {key: values[key] for key in ("encoder", "capture", "bitrate")}
        elif kind == 1 or kind in (2, 9):
            if kind in (2, 9) and not counts[1]:
                raise ValueError("video frame before format")
            if kind == 9:
                if len(payload) < 5 or payload[4] not in (0, 1):
                    raise ValueError("invalid video flags")
                sequence = int.from_bytes(payload[:4], "big")
                if next_frame is not None and sequence != next_frame:
                    raise ValueError("invalid video sequence")
                next_frame = (sequence + 1) % (1 << 32)
            units = payload[5:] if kind == 9 else payload
            n = 0
            while units:
                length = int.from_bytes(units[:4], "big")
                if not 0 < length <= len(units) - 4:
                    raise ValueError("invalid video unit")
                n += 1
                units = units[4 + length:]
            if not n or kind == 1 and n != 2:
                raise ValueError("invalid video format or frame")
        elif kind != 4 or len(payload) != 9:
            raise ValueError("unexpected record in H.264 capture")
        if observe is not None:
            observe(kind, header, record)
        counts[kind] += 1
        if kind in (5, 6):
            audio.write(header + record)
    required = {1, 5, 6} | ({9, 10} if feedback else {2})
    if required.difference(counts) or (2 if feedback else 9) in counts:
        raise ValueError("missing or mismatched capture records")
    return {"records": dict(counts), "stream_info": info or None, "codec": "H.264",
            "audio_format": audio_format, "packets_before_first_format": early_packets}
