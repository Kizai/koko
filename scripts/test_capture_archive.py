"""Synthetic file contents for archive mechanics only, never macOS protocol fixtures."""
import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest

from capture_session import NAMES, package


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
            package(artifacts, target, lambda: None)
            with tarfile.open(target) as archive:
                self.assertEqual(set(archive.getnames()), set(NAMES) | {"SHA256SUMS"})
                for line in archive.extractfile("SHA256SUMS").read().decode().splitlines():
                    expected, name = line.split("  ")
                    self.assertEqual(hashlib.sha256(archive.extractfile(name).read()).hexdigest(), expected)

    def test_scan_failure_or_unexpected_input_withholds_archive(self):
        def reject():
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
            with self.assertRaisesRegex(ValueError, "artifact set"):
                package(folder, target, reject)
            key.unlink()
            with self.assertRaisesRegex(ValueError, "scan failure"):
                package(folder, target, reject)
            self.assertFalse(target.exists())
