# usage: python3 render-uart-frames.py <uart8-tx.bin> <out-prefix>; replays tile records (tag&7 = column, y = row, 8 payload bytes, bit 7 left) into a 64x128 framebuffer and saves PNGs at each B8-terminated update plus a montage.
import struct, sys, zlib

W, H = 64, 128

def png(path, width, height, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * width:(y + 1) * width]) for y in range(height))
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))

data = open(sys.argv[1], "rb").read()
out = sys.argv[2]
frame = [0] * (W * H)
states, index, current = [], 0, 0
while index < len(data):
    byte = data[index]
    if byte == 0xB5:
        index += 5
    elif byte in (0xB7, 0x60, 0x70, 0x73, 0xBA):
        index += 2
    elif byte == 0xB8:
        if current:
            states.append(list(frame))
            current = 0
        index += 1
    elif 0x10 <= byte <= 0x17:
        x0, y0 = (byte & 7) * 8, data[index + 1]
        for row in range(8):
            for col in range(8):
                frame[(y0 + row) * W + x0 + col] = 255 if data[index + 2 + row] >> (7 - col) & 1 else 0
        current += 1
        index += 10
    else:
        raise SystemExit(f"unknown byte {byte:#04x} at {index}")
print(len(states), "updates")
keep = sorted({0, len(states) // 4, len(states) // 2, 3 * len(states) // 4, len(states) - 1})
for k in keep:
    png(f"{out}-{k:03d}.png", W, H, states[k])
cols = 12
rows = (len(states) + cols - 1) // cols
sheet = [64] * ((W + 2) * cols * (H + 2) * rows)
for k, state in enumerate(states):
    ox, oy = (k % cols) * (W + 2) + 1, (k // cols) * (H + 2) + 1
    for y in range(H):
        base = (oy + y) * (W + 2) * cols + ox
        sheet[base:base + W] = state[y * W:(y + 1) * W]
png(f"{out}-montage.png", (W + 2) * cols, (H + 2) * rows, sheet)
changed = [sum(1 for a, b in zip(s, t) if a != b) for s, t in zip(states, states[1:])]
print("consecutive updates identical:", sum(1 for c in changed if c == 0), "of", len(changed))
print("pixels changed between updates: min", min(changed), "max", max(changed))
