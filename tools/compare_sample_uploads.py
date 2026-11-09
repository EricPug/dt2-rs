#!/usr/bin/env python3
"""Compare modeled sample-upload captures against the same native sample file."""

import argparse
import hashlib
import struct
from pathlib import Path

PAGE_BYTES = 0x1000
PACKET_BYTES = 4 + PAGE_BYTES
SENTINEL = 0xFFFFFFFF


def read_packets(directory, pattern):
    paths = sorted(directory.glob(pattern))
    if not paths:
        raise ValueError(f"no packets matching {pattern} in {directory}")
    packets = []
    for path in paths:
        data = path.read_bytes()
        if len(data) != PACKET_BYTES:
            raise ValueError(f"{path}: expected {PACKET_BYTES} bytes, got {len(data)}")
        packets.append((path.name, data))
    return packets


def native_sample(path):
    data = path.read_bytes()
    if len(data) < 0x40:
        raise ValueError(f"{path}: sample header is incomplete")
    length, rate = struct.unpack_from(">II", data, 4)
    if length == 0 or length % 2 or rate == 0 or len(data) < 0x40 + length:
        raise ValueError(f"{path}: invalid sample length or rate")
    return data[: 0x40 + length], length, rate


def header(packet):
    return struct.unpack_from("<6I", packet)


def swap_pairs(data):
    swapped = bytearray(len(data))
    swapped[0::2] = data[1::2]
    swapped[1::2] = data[0::2]
    return bytes(swapped)


def find_upload(packets, source, length, rate):
    pages_needed = (len(source) + PAGE_BYTES - 1) // PAGE_BYTES
    expected_pages = []
    for i in range(pages_needed):
        body = source[i * PAGE_BYTES : (i + 1) * PAGE_BYTES]
        expected_pages.append(swap_pairs(body))

    for start in range(len(packets) - pages_needed - 1):
        begin = header(packets[start][1])
        if begin[0] != SENTINEL or begin[2:] != (0xFFFFFC00, 0xFFFFFC00, 0, 0):
            continue
        page_start = start + 1
        pages = []
        cursor = page_start
        while cursor < len(packets):
            packet = packets[cursor][1]
            tag = header(packet)[0]
            if tag == SENTINEL:
                break
            if tag != len(pages):
                break
            pages.append(packets[cursor])
            cursor += 1
        if len(pages) < pages_needed or cursor >= len(packets):
            continue
        end = header(packets[cursor][1])
        if end != (SENTINEL, begin[1], 0x40, 0x40, rate, length):
            continue
        if any(
            page[1][4 : 4 + len(expected)] != expected
            for page, expected in zip(pages, expected_pages)
        ):
            continue
        return {
            "slot": begin[1],
            "start": packets[start][0],
            "end": packets[cursor][0],
            "pages": pages,
            "meaningful_pages": pages_needed,
        }
    raise ValueError("no complete upload matches the native sample and its metadata")


def compare(source_path, local_dir, reference_dir):
    source, length, rate = native_sample(source_path)
    local = find_upload(read_packets(local_dir, "call-*.bin"), source, length, rate)
    reference = find_upload(read_packets(reference_dir, "unit-*.bin"), source, length, rate)
    pages = local["meaningful_pages"]
    identical = sum(
        local["pages"][i][1] == reference["pages"][i][1] for i in range(pages)
    )
    print(f"sample={source_path} sha256={hashlib.sha256(source_path.read_bytes()).hexdigest()}")
    print(f"rate={rate} pcm_bytes={length} meaningful_bytes={len(source)}")
    for name, result in (("local", local), ("reference", reference)):
        print(
            f"{name}: slot={result['slot']} {result['start']}..{result['end']} "
            f"pages={len(result['pages'])} meaningful_pages={pages}"
        )
    print(f"PASS meaningful bytes match source in both captures; full packets equal={identical}/{pages}")
    return identical, pages


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sample", type=Path)
    parser.add_argument("local_packets", type=Path)
    parser.add_argument("reference_packets", type=Path)
    args = parser.parse_args()
    try:
        compare(args.sample, args.local_packets, args.reference_packets)
    except (OSError, ValueError) as error:
        parser.exit(1, f"FAIL {error}\n")


if __name__ == "__main__":
    main()
