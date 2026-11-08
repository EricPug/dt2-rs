# usage: python3 render-uart-frames.py [--base <upright-4x.png>] <uart8-tx.bin> [continued-tx.bin ...] <out-prefix>; replays sequential captures into one 64x128 framebuffer and saves sample/final frames and a montage.
import struct, sys, zlib

W, H = 64, 128

args = sys.argv[1:]
base_path = None
if args[:1] == ["--base"]:
    if len(args) < 4:
        raise SystemExit("usage: render-uart-frames.py [--base <upright-4x.png>] <uart8-tx.bin> [continued-tx.bin ...] <out-prefix>")
    base_path, args = args[1], args[2:]
if len(args) < 2:
    raise SystemExit("usage: render-uart-frames.py [--base <upright-4x.png>] <uart8-tx.bin> [continued-tx.bin ...] <out-prefix>")

input_paths = args[:-1]
out_prefix = args[-1]

def png(path, width, height, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * width:(y + 1) * width]) for y in range(height))
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))

def save_frame(path, pixels, updated):
    if not base_path:
        png(path, W, H, pixels)
        return pixels
    scale = 4
    upright = [byte for row in base_rows for byte in row]
    stride = H * scale
    for y in range(H):
        for x in range(W):
            index = y * W + x
            if not updated[index]:
                continue
            x0, y0 = y * scale, (W - 1 - x) * scale
            for row in range(y0, y0 + scale):
                start = row * stride + x0
                upright[start:start + scale] = [pixels[index]] * scale
    png(path, H * scale, W * scale, upright)
    return upright

def load_upright_base(path):
    with open(path, "rb") as source:
        data = source.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: invalid PNG signature")
    offset, width, height, image_data = 8, None, None, bytearray()
    bit_depth = color_type = interlace = None
    while offset < len(data):
        length = struct.unpack_from(">I", data, offset)[0]
        tag = data[offset + 4:offset + 8]
        chunk = data[offset + 8:offset + 8 + length]
        if tag == b"IHDR":
            width, height, bit_depth, color_type = struct.unpack_from(">IIBB", chunk)
            interlace = chunk[12]
        elif tag == b"IDAT":
            image_data.extend(chunk)
        elif tag == b"IEND":
            break
        offset += length + 12
    if (width, height, bit_depth, color_type, interlace) != (H * 4, W * 4, 8, 0, 0):
        raise SystemExit(f"{path}: expected an 8-bit grayscale {H * 4}x{W * 4} upright PNG")
    raw = zlib.decompress(image_data)
    stride = width + 1
    if len(raw) != stride * height:
        raise SystemExit(f"{path}: unexpected PNG data length")
    rows = []
    for y in range(height):
        start = y * stride
        if raw[start] != 0:
            raise SystemExit(f"{path}: unsupported PNG row filter {raw[start]}")
        rows.append(raw[start + 1:start + stride])
    return rows

base_rows = load_upright_base(base_path) if base_path else None
frame = [0] * (W * H)
updated = [False] * (W * H)
states, update_record_counts, current = [], [], 0
previous_input_frame = list(frame)
for input_index, input_path in enumerate(input_paths):
    with open(input_path, "rb") as source:
        data = source.read()
    start_update = len(states)
    input_records = 0
    index = 0
    while index < len(data):
        byte = data[index]
        if byte == 0xB5:
            index += 5
        elif byte in (0xB7, 0x60, 0x70, 0x73, 0xBA, 0xBC):
            index += 2
        elif byte == 0xB9:
            index += 3
        elif byte == 0xB8:
            if current:
                states.append(list(frame))
                update_record_counts.append(current)
                current = 0
            index += 1
        elif 0x10 <= byte <= 0x17:
            x0, y0 = (byte & 7) * 8, data[index + 1]
            for row in range(8):
                for col in range(8):
                    pixel = (y0 + row) * W + x0 + col
                    frame[pixel] = 255 if data[index + 2 + row] >> (7 - col) & 1 else 0
                    updated[pixel] = True
            current += 1
            input_records += 1
            index += 10
        else:
            raise SystemExit(f"unknown byte {byte:#04x} at {input_path}:{index}")
    input_updates = len(states) - start_update
    if input_updates:
        final_input_frame = states[-1]
        output = save_frame(
            f"{out_prefix}-input-{input_index:03d}-final.png", final_input_frame, updated
        )
        if base_path:
            base_pixels = [byte for row in base_rows for byte in row]
            changed_pixels = sum(a != b for a, b in zip(base_pixels, output))
            print(f"{input_path}: {input_updates} updates; {input_records} tile records; changed output pixels={changed_pixels}")
        else:
            changed_pixels = sum(a != b for a, b in zip(previous_input_frame, final_input_frame))
            print(f"{input_path}: {input_updates} updates; {input_records} tile records; final pixels changed={changed_pixels}")
        previous_input_frame = list(final_input_frame)
    elif current:
        print(f"{input_path}: update continues into next input")
print(len(states), "updates and", sum(update_record_counts), "tile records")
if not states:
    if base_path:
        save_frame(f"{out_prefix}-final.png", frame, updated)
        print("no UART updates; wrote the unchanged base frame")
        raise SystemExit(0)
    raise SystemExit("no B8-terminated tile updates found")
keep = sorted({0, len(states) // 4, len(states) // 2, 3 * len(states) // 4, len(states) - 1})
for k in keep:
    save_frame(f"{out_prefix}-{k:03d}.png", states[k], updated)
save_frame(f"{out_prefix}-final.png", states[-1], updated)
cols = 12
rows = (len(states) + cols - 1) // cols
sheet = [64] * ((W + 2) * cols * (H + 2) * rows)
for k, state in enumerate(states):
    ox, oy = (k % cols) * (W + 2) + 1, (k // cols) * (H + 2) + 1
    for y in range(H):
        base = (oy + y) * (W + 2) * cols + ox
        sheet[base:base + W] = state[y * W:(y + 1) * W]
png(f"{out_prefix}-montage.png", (W + 2) * cols, (H + 2) * rows, sheet)
changed = [sum(1 for a, b in zip(s, t) if a != b) for s, t in zip(states, states[1:])]
print("consecutive updates identical:", sum(1 for c in changed if c == 0), "of", len(changed))
print("pixels changed between updates: min", min(changed), "max", max(changed))
