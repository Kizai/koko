#!/usr/bin/env python3
"""Line recorder installed solely in a disposable copy of the ad hoc app."""
import base64
import fcntl
import json
import os
import re
import signal
import subprocess
import sys
import threading
import uuid
from pathlib import Path


class Guard:
    markers = re.compile(rb'koko://pair/|(?:pair(?:ing)?|verification)[ _-]?(?:code|invite)|custom[ _-]?code|'
                         rb'private[ _-]?key|secret[ _-]?key|-----BEGIN .*PRIVATE', re.I)

    def __init__(self, state):
        self.state = state
        self.codes = set()
        self.keys = set()
        self.lock = threading.Lock()

    def observe(self, line):
        # Recognize codes solely to suppress their accidental appearance in an artifact.
        for prefix in (b"Pair code: ", b"custom-code "):
            if line.startswith(prefix):
                value = re.sub(r"[\s-]", "", line[len(prefix):].decode()).lower().encode()
                if value:
                    with self.lock:
                        self.codes.add(value)

    def refresh(self):
        for name in ("controller.key", "host.key", "home/.koko/controller.key"):
            path = self.state / name
            if path.is_file():
                raw = path.read_bytes()
                if len(raw) != 32:
                    raise ValueError("invalid disposable identity")
                self.keys.update((raw, raw.hex().encode(), raw.hex().upper().encode(),
                                  base64.b64encode(raw).rstrip(b'='),
                                  base64.urlsafe_b64encode(raw).rstrip(b'='),
                                  base64.b32encode(raw).rstrip(b'='),
                                  base64.b32encode(raw).rstrip(b'=').lower()))

    def check(self, data):
        with self.lock:
            folded = re.sub(rb"[\s-]", b"", data.lower())
            if (self.markers.search(data) or any(key in data for key in self.keys)
                    or any(code in folded for code in self.codes)):
                raise ValueError("unsafe capture")

    def candidate(self, line, prefix):
        if not line.endswith(b'\n'):
            raise ValueError("incomplete desktop line")
        number, payload = line[len(prefix):-1].split(b' ', 1)
        if not re.fullmatch(rb'[0-9]+', number):
            raise ValueError("invalid desktop number")
        obj = json.loads(payload.decode('utf-8'))
        if not isinstance(obj, dict):
            raise ValueError("invalid desktop data")
        with self.lock:
            self.refresh()
        self.check(line)
        def check_json(value):
            if isinstance(value, str):
                self.check(re.sub(r"[\s-]", "", value).encode())
            elif isinstance(value, list):
                for item in value:
                    check_json(item)
            elif isinstance(value, dict):
                for key, item in value.items():
                    self.check(key.encode())
                    if key in ("png", "image"):
                        if not isinstance(item, str):
                            raise ValueError("invalid encoded image")
                        self.check(base64.b64decode(item, validate=True))
                    else:
                        check_json(item)
        check_json(obj)


def run_bridge(real, args, state):
    state = state.resolve()
    host_key = state / "host.key"
    if args[:1] == ["identity"]:
        os.execv(real, [real, "identity", str(host_key)])
    if args[:1] != ["serve-pair"]:
        os.execv(real, [real, *args])
    if len(args) != 5 or args[3] != "--app-desktop":
        raise ValueError("unexpected host invocation")
    args[1:3] = [str(host_key), "127.0.0.1:0"]
    instance = state / ("bridge-" + uuid.uuid4().hex)
    instance.mkdir()
    owner = (instance / "owner.lock").open("wb")
    fcntl.flock(owner, fcntl.LOCK_EX)
    link = instance / "pending-link"
    link.symlink_to(instance)
    link.replace(state / "current")
    daemon = subprocess.Popen([real, *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    (instance / "running").write_text(str(daemon.pid))
    guard = Guard(state)
    failure = threading.Event()
    stopped = threading.Event()

    def abort(*_):
        failure.set()
        (state / "bridge-error").touch()
        (instance / "failed").touch()
        daemon.kill()

    def terminate(*_):
        # Normal permission relaunch and cleanup may terminate this wrapper.
        daemon.terminate()

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGINT, terminate)

    def pump(source, target, prefix, filename, host):
        address = None
        try:
            for line in source:
                guard.observe(line)
                if line.startswith(prefix):
                    if (state / "current").resolve() != instance:
                        raise ValueError("host changed")
                    guard.candidate(line, prefix)
                    subprocess.run(["/usr/bin/tee", "-a", str(instance / filename)],
                                   input=line, stdout=target, stderr=subprocess.DEVNULL,
                                   timeout=30, check=True)
                else:
                    target.write(line)
                    target.flush()
                if host and line.startswith(b"Listening on "):
                    address = line[len(b"Listening on "):].strip().decode("ascii")
                    if not re.fullmatch(r'127\.0\.0\.1:[0-9]+', address):
                        raise ValueError("unexpected listener")
                if host and address and re.fullmatch(rb'Device ID: [0-9]{9}\n', line):
                    tmp = instance / "listener.tmp"
                    tmp.write_text(json.dumps({"address": address, "device_id": line[11:-1].decode()}))
                    tmp.replace(instance / "listener.json")
                if host and line == b"Pair approved\n":
                    (instance / "approved").touch()
            if not host:
                daemon.stdin.close()
                daemon.terminate()
        except BrokenPipeError:
            if daemon.poll() is None and not (state / "stop-bridge").exists():
                abort()
        except Exception:
            abort()

    def final_check():
        try:
            while not stopped.wait(0.1):
                if (state / "stop-bridge").exists():
                    daemon.terminate()
                    return
                if (instance / "scan-request").exists():
                    for path in (state / "artifacts").iterdir():
                        tail = b''
                        with path.open('rb') as stream:
                            while True:
                                block = stream.read(1024 * 1024)
                                if not block:
                                    break
                                guard.check(tail + block)
                                tail = block[-256:]
                    (instance / "scan-complete").touch()
                    while not stopped.wait(0.1):
                        if (state / "stop-bridge").exists():
                            daemon.terminate()
                            return
        except Exception:
            abort()

    threading.Thread(target=final_check, daemon=True).start()
    threading.Thread(target=pump, args=(os.fdopen(os.dup(0), "rb"), daemon.stdin, b"desktop-reply ",
                                        "replies.jsonl", False), daemon=True).start()
    output = threading.Thread(target=pump, args=(daemon.stdout, sys.stdout.buffer, b"Desktop request: ",
                                                 "requests.jsonl", True))
    output.start()
    try:
        result = daemon.wait()
        output.join()
        return 1 if failure.is_set() else result
    finally:
        stopped.set()
        if daemon.poll() is None:
            daemon.kill()
            daemon.wait()
        (instance / "running").unlink(missing_ok=True)
        owner.close()


if __name__ == "__main__":
    try:
        state = Path(os.environ["KOKO_CAPTURE_STATE"])
        os.umask(0o077)
        status = run_bridge(str(Path(__file__).with_name("koko-daemon-real")), sys.argv[1:], state)
    except Exception:
        status = 1
    sys.exit(status)
