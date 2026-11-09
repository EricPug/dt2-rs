#!/usr/bin/env python3
"""Replace the pinned kick file with the card's clap on a disposable card copy."""

import hashlib
import struct
import sys
from pathlib import Path

SECTOR = 512
BASE = 0x5D8000
BLOCK_SECTORS = 64
BLOCK_BYTES = BLOCK_SECTORS * SECTOR
INODE_BYTES = 128
INODES_PER_CHUNK = 256
CLAP_HASH = "fc4138ca0334cdb909d9646e2701c25a7e122d97c067eef5bd064726c46a1576"


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: substitute-clap-card.py DISPOSABLE-CARD-COPY")
    path = Path(sys.argv[1])
    if path.samefile("out/run-card.img"):
        raise SystemExit("refusing to modify the original card")
    with path.open("r+b") as card:
        def read_at(offset, size):
            card.seek(offset)
            data = card.read(size)
            if len(data) != size:
                raise ValueError(f"short read at {offset:#x}")
            return data

        def write_at(offset, data):
            card.seek(offset)
            if card.write(data) != len(data):
                raise ValueError(f"short write at {offset:#x}")

        sb = read_at(BASE * SECTOR, SECTOR)
        if sb[:4] != b"ekFS" or len(sb) != SECTOR:
            raise ValueError("not the pinned DT2 ekFS card")
        inode_table, data_off = struct.unpack_from(">II", sb, 0x1C)
        if (inode_table, data_off) != (0x180, 0x16180):
            raise ValueError("unexpected DT2 ekFS geometry")

        def inode_offset(ino):
            return (BASE + inode_table + ino // INODES_PER_CHUNK * BLOCK_SECTORS) * SECTOR + (
                ino % INODES_PER_CHUNK
            ) * INODE_BYTES

        def inode(ino):
            return bytearray(read_at(inode_offset(ino), INODE_BYTES))

        def extent(raw):
            count = struct.unpack_from(">H", raw, 0x1E)[0]
            logical, blocks, physical = struct.unpack_from(">III", raw, 0x20)
            if count != 1 or logical != 0:
                raise ValueError("expected a single contiguous file extent")
            return physical, blocks

        def block_offset(block):
            return (BASE + data_off + block * BLOCK_SECTORS) * SECTOR

        incoming = inode(3)
        if incoming[0] != 1:
            raise ValueError("inode 3 is not the expected incoming directory")
        directory_block = struct.unpack_from(">I", incoming, 0x28)[0]
        directory = read_at(block_offset(directory_block), BLOCK_BYTES)
        entries = {}
        offset = 0
        while offset + 8 <= len(directory):
            ino, record_len, name_len, typ = struct.unpack_from(">IHBB", directory, offset)
            if record_len < 8 or offset + record_len > len(directory) or name_len > record_len - 8:
                raise ValueError("invalid incoming directory record")
            if ino and typ == 0:
                entries[directory[offset + 8 : offset + 8 + name_len]] = ino
            offset += record_len
        if entries.get(b"303 clap real") != 4 or entries.get(b"008 kick verb") != 5:
            raise ValueError("unexpected sample inodes in incoming directory")
        clap, kick = inode(4), inode(5)
        clap_block, clap_blocks = extent(clap)
        kick_block, kick_blocks = extent(kick)
        clap_length = struct.unpack_from(">I", clap, 4)[0]
        kick_length = struct.unpack_from(">I", kick, 4)[0]
        if (clap_block, clap_blocks, kick_block, kick_blocks, clap_length, kick_length) != (
            128, 1, 129, 2, 0x3072, 0x9C50
        ):
            raise ValueError("unexpected sample lengths or extents")
        content = read_at(block_offset(clap_block), clap_length)
        if hashlib.sha256(content).hexdigest() != CLAP_HASH:
            raise ValueError("clap content differs from the verified sample")
        hash_table = block_offset(64)
        clap_hash = clap[12:16]
        if clap_hash != read_at(hash_table + 4 * 4, 4):
            raise ValueError("clap inode and hash table disagree")
        if kick[12:16] != read_at(hash_table + 5 * 4, 4):
            raise ValueError("kick inode and hash table disagree")

        write_at(block_offset(kick_block), content.ljust(kick_blocks * BLOCK_BYTES, b"\0"))
        kick[4:8] = clap[4:8]
        kick[12:16] = clap_hash
        write_at(inode_offset(5), kick)
        write_at(hash_table + 5 * 4, clap_hash)
        card.flush()
        if read_at(block_offset(kick_block), clap_length) != content or inode(5) != kick:
            raise ValueError("replacement verification failed")
    print(f"replaced /incoming/008 kick verb on {path} with verified clap bytes")


if __name__ == "__main__":
    main()
