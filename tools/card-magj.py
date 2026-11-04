#!/usr/bin/env python3
"""Stamp the minimal "MaGj" table header the OS validates before it mounts the ekFS.

usage: card-magj.py <card.img>

The mount path (0x4015a400 -> 0x4002cd6a -> 0x4002ccd0) reads 0x8000 bytes at sector 0x458000
and requires: "MaGj" at +0, u16 0x2b at +8, u16 3 at +0xa, u32 length at +0xc, and a CRC-32 stored
little-endian at +4 so the OS routine (init 0xFFFFFFFF, no final xor) over [8, length) then the
stored word leaves 0xDEBB20E3. Entries (count at +0x10) are looked up by id; count 0 means none.
This is an emulation accommodation that satisfies the firmware's own validator, not a copy of
real card content.
"""
import struct
import sys
import zlib

SECTOR = 0x458000

body = struct.pack(">HHII", 0x2B, 3, 0x14, 0)
header = b"MaGj" + struct.pack("<I", zlib.crc32(body)) + body
with open(sys.argv[1], "r+b") as card:
    card.seek(SECTOR * 512)
    card.write(header)
