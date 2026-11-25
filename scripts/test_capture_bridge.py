"""Synthetic negative test inputs for capture tooling, never protocol golden data."""
import tempfile
import unittest
from pathlib import Path

from capture_bridge import Guard


class CandidateTests(unittest.TestCase):
    def test_credentials_are_never_candidates(self):
        with tempfile.TemporaryDirectory() as directory:
            guard = Guard(Path(directory))
            private_key = bytes(range(32))
            (Path(directory) / "host.key").write_bytes(private_key)
            guard.observe(b'Pair code: ABCD1234\n')
            for raw in (b'{"text":"abcd1234"}', b'{"text":"koko://pair/v1/x"}',
                        b'{"privateKey":"x"}', br'{"text":"ABCD\u0031234"}',
                        b'{"text":"' + private_key.hex().encode() + b'"}'):
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    guard.candidate(b'Desktop request: 12 ' + raw + b'\n', b'Desktop request: ')


class BridgeProcessTests(unittest.TestCase):
    # This substitute only exercises Unix pipes and child cleanup. Its data never ships.
    def exercise(self, fail):
        import os
        import select
        import shutil
        import subprocess
        import sys
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            wrapper = state / "koko-daemon"
            shutil.copyfile(Path(__file__).with_name("capture_bridge.py"), wrapper)
            real = state / "koko-daemon-real"
            real.write_text("""#!/usr/bin/env python3
import os, sys
from pathlib import Path
state = Path(os.environ['KOKO_CAPTURE_STATE'])
(state/'child.pid').write_text(str(os.getpid()))
if (state/'tee-failure').exists():
    (state/'current'/'requests.jsonl').mkdir()
print('Pair code: ABCD1234', flush=True)
print('Pair invite: koko://pair/v1/ABCD1234/test', flush=True)
sample = b'Desktop request: 3 { "action" : "displays" }\\n'
if (state/'leak').exists():
    sample = b'Desktop request: 3 {"text":"ABCD1234"}\\n'
sys.stdout.buffer.write(sample)
sys.stdout.buffer.flush()
reply = sys.stdin.buffer.readline()
if reply == b'desktop-reply 3 {"status":"ok"}\\n':
    print('Pair approved', flush=True)
""")
            real.chmod(0o700)
            if fail == "credential":
                (state / "leak").touch()
            elif fail == "tee":
                (state / "tee-failure").touch()
            with subprocess.Popen([sys.executable, wrapper, "serve-pair", "unused", "unused",
                                   "--app-desktop", "test"], env=dict(os.environ, KOKO_CAPTURE_STATE=directory),
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as proc:
                def line():
                    self.assertTrue(select.select([proc.stdout], [], [], 5)[0])
                    return proc.stdout.readline()
                try:
                    if fail is None:
                        self.assertEqual(line(), b"Pair code: ABCD1234\n")
                        self.assertTrue(line().startswith(b'Pair invite: '))
                        request = line()
                        reply = b'desktop-reply 3 {"status":"ok"}\n'
                        proc.stdin.write(reply)
                        proc.stdin.flush()
                        self.assertEqual(line(), b"Pair approved\n")
                    status = proc.wait(timeout=10)
                    if fail:
                        self.assertNotEqual(status, 0)
                        self.assertTrue((state / "bridge-error").exists())
                        if fail == "credential":
                            self.assertFalse((state / "current" / "requests.jsonl").exists())
                    else:
                        self.assertEqual(status, 0)
                        self.assertEqual((state / "current" / "requests.jsonl").read_bytes(), request)
                        self.assertEqual((state / "current" / "replies.jsonl").read_bytes(), reply)
                    pid = int((state / "child.pid").read_text())
                    with self.assertRaises(ProcessLookupError):
                        os.kill(pid, 0)
                finally:
                    if proc.poll() is None:
                        proc.kill()

    def test_forwarded_bytes_and_child_exit(self):
        self.exercise(None)

    def test_candidate_never_reaches_disk(self):
        self.exercise("credential")

    def test_tee_failure_reaps_daemon(self):
        self.exercise("tee")
