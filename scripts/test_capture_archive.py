"""Synthetic file contents for archive mechanics only, never macOS protocol fixtures."""
import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest

from unittest.mock import patch

from capture_session import NAMES, CaptureError, capture_and_package, prepare_manifest


class ArchiveTests(unittest.TestCase):
    def test_only_allowlisted_files_and_valid_relative_digests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            (root / "host.key").touch()
            for name in NAMES:
                (artifacts / name).write_text(name)
            target = root / "test.tar.gz"
            with patch("capture_session.run_session", side_effect=lambda *_: prepare_manifest(artifacts)):
                capture_and_package(root, target, False)
            with tarfile.open(target) as archive:
                self.assertEqual(set(archive.getnames()), set(NAMES) | {"SHA256SUMS"})
                for line in archive.extractfile("SHA256SUMS").read().decode().splitlines():
                    expected, name = line.split("  ")
                    self.assertEqual(hashlib.sha256(archive.extractfile(name).read()).hexdigest(), expected)

    def test_scan_failure_or_unexpected_input_withholds_archive(self):
        def reject(state, _dirty):
            prepare_manifest(state / "artifacts")
            raise ValueError("synthetic scan failure")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            folder = root / "artifacts"
            folder.mkdir()
            for name in NAMES:
                (folder / name).touch()
            key = folder / "host.key"
            key.touch()
            target = root / "capture.tar.gz"
            with patch("capture_session.run_session", side_effect=reject):
                with self.assertRaisesRegex(ValueError, "artifact set"):
                    capture_and_package(root, target, False)
                key.unlink()
                with self.assertRaisesRegex(ValueError, "scan failure"):
                    capture_and_package(root, target, False)
            self.assertFalse(target.exists())

    def test_tar_open_requires_completed_cleanup_and_clean_session(self):
        for late_failure in (True, False):
            with self.subTest(late_failure=late_failure), tempfile.TemporaryDirectory() as directory:
                state = Path(directory)
                destination = state / "result.tar.gz"
                events = []
                def collect(state, source_dirty):
                    folder = state / "artifacts"
                    folder.mkdir()
                    for name in NAMES:
                        (folder / name).touch()
                    prepare_manifest(folder)
                    events.append("scanned")
                    if late_failure:
                        (state / "bridge-error").touch()
                    events.append("writers exited")
                original_open = tarfile.open
                def watch_tar(*args, **kwargs):
                    events.append("tar open")
                    return original_open(*args, **kwargs)
                with patch("capture_session.run_session", side_effect=collect), patch("tarfile.open", side_effect=watch_tar) as tar_open:
                    if late_failure:
                        with self.assertRaises(CaptureError):
                            capture_and_package(state, destination, False)
                        tar_open.assert_not_called()
                        self.assertFalse(destination.exists())
                    else:
                        capture_and_package(state, destination, False)
                        self.assertEqual(events, ["scanned", "writers exited", "tar open"])
                        self.assertTrue(destination.exists())
