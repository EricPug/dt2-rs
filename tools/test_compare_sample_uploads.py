#!/usr/bin/env python3
import struct
import tempfile
import unittest
from pathlib import Path

import compare_sample_uploads as comparison


class CompareSampleUploadsTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.local = self.root / "local"
        self.reference = self.root / "reference"
        self.local.mkdir()
        self.reference.mkdir()
        self.sample = self.root / "sample"
        header = bytearray(64)
        struct.pack_into(">II", header, 4, 5000, 46875)
        self.data = bytes(header) + bytes(i % 251 for i in range(5000))
        self.sample.write_bytes(self.data + bytes(16))
        self.write_capture(self.local, "call", 300, extra_pages=1)
        self.write_capture(self.reference, "unit", 7)

    def write_capture(self, directory, prefix, slot, extra_pages=0):
        def packet(tag, body=b""):
            return struct.pack("<I", tag) + body.ljust(4096, b"\0")

        begin = packet(0xFFFFFFFF, struct.pack("<5I", slot, 0xFFFFFC00, 0xFFFFFC00, 0, 0))
        end = packet(0xFFFFFFFF, struct.pack("<5I", slot, 64, 64, 46875, 5000))
        pages = [
            packet(i, comparison.swap_pairs(self.data[i * 4096 : (i + 1) * 4096]))
            for i in range(2)
        ]
        pages.extend(packet(i + 2) for i in range(extra_pages))
        for i, data in enumerate([begin, *pages, end], 1):
            (directory / f"{prefix}-{i:06}.bin").write_bytes(data)

    def test_equal_source_different_slots_and_extra_pages(self):
        self.assertEqual(comparison.compare(self.sample, self.local, self.reference), (2, 2))

    def test_different_source_fails(self):
        modified = bytearray(self.sample.read_bytes())
        modified[100] ^= 1
        self.sample.write_bytes(modified)
        with self.assertRaisesRegex(ValueError, "no complete upload"):
            comparison.compare(self.sample, self.local, self.reference)

    def test_truncated_packet_fails(self):
        (self.reference / "unit-000002.bin").write_bytes(b"short")
        with self.assertRaisesRegex(ValueError, "expected 4100 bytes"):
            comparison.compare(self.sample, self.local, self.reference)

    def test_wrong_header_length_fails(self):
        path = self.reference / "unit-000004.bin"
        data = bytearray(path.read_bytes())
        struct.pack_into("<I", data, 20, 4000)
        path.write_bytes(data)
        with self.assertRaisesRegex(ValueError, "no complete upload"):
            comparison.compare(self.sample, self.local, self.reference)

    def test_missing_page_fails(self):
        (self.reference / "unit-000002.bin").unlink()
        with self.assertRaisesRegex(ValueError, "no complete upload"):
            comparison.compare(self.sample, self.local, self.reference)

    def test_misordered_pages_fail(self):
        first = self.reference / "unit-000002.bin"
        second = self.reference / "unit-000003.bin"
        first_data, second_data = first.read_bytes(), second.read_bytes()
        first.write_bytes(second_data)
        second.write_bytes(first_data)
        with self.assertRaisesRegex(ValueError, "no complete upload"):
            comparison.compare(self.sample, self.local, self.reference)

    def test_padding_difference_is_reported_not_rejected(self):
        path = self.reference / "unit-000003.bin"
        data = bytearray(path.read_bytes())
        data[-1] = 1
        path.write_bytes(data)
        self.assertEqual(comparison.compare(self.sample, self.local, self.reference), (1, 2))


if __name__ == "__main__":
    unittest.main()
