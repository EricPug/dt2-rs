# usage: python3 render-uploads.py <upload-prefix> <out-prefix>; renders the first, middle and last distinct <prefix>-NNN.bin as 128x64 4bpp (both nibble orders) and 64x64 8bpp PNGs and reports distinct frames.
import glob, hashlib, struct, sys, zlib

def png(path, width, height, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * width:(y + 1) * width]) for y in range(height))
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))

def nibbles(data, high_first):
    out = []
    for byte in data:
        pair = (byte >> 4, byte & 15) if high_first else (byte & 15, byte >> 4)
        out.extend(v * 17 for v in pair)
    return out

prefix, out = sys.argv[1], sys.argv[2]
seen = {}
blobs = {}
for path in sorted(glob.glob(prefix + "-*.bin")):
    data = open(path, "rb").read()
    index = path[len(prefix) + 1:-4]
    digest = hashlib.sha256(data).hexdigest()[:12]
    seen.setdefault(digest, []).append(index)
    blobs.setdefault(digest, (index, data))
order = list(blobs.values())
for index, data in {order[0][0]: order[0], order[len(order) // 2][0]: order[len(order) // 2], order[-1][0]: order[-1]}.values():
    name = f"{out}-{index}"
    png(f"{name}-4bpp-high.png", 128, 64, nibbles(data, True))
    png(f"{name}-4bpp-low.png", 128, 64, nibbles(data, False))
    png(f"{name}-8bpp-64x64.png", 64, 64, list(data))
print(f"{sum(len(v) for v in seen.values())} uploads, {len(seen)} distinct buffers")
for digest, indexes in list(seen.items())[:5]:
    print(digest, f"first={indexes[0]} last={indexes[-1]} count={len(indexes)}")
