#!/usr/bin/env python3
"""Verify captured upload packets and modeled WAV against the original WAV."""

import hashlib
import re
import struct
import sys
import wave
from pathlib import Path


def fail(message):
    raise SystemExit(message)


def main():
    if len(sys.argv) != 4:
        fail("usage: verify-modeled-dsp.py OUTDIR ORIGINAL-WAV EXPECTED-LENGTH")
    out, source_path = map(Path, sys.argv[1:3])
    expected_length = int(sys.argv[3], 0)
    log = (out / "run.log").read_text(encoding="utf-8", errors="replace")
    uploads = re.findall(
        r"^MODELED_DSP RGPIO call=(\d+) prefix=([0-9a-f]{8}) (header|page=(\d+))",
        log,
        re.MULTILINE,
    )
    expected_uploads = [(1, "ffffffff", "header", "")] + [
        (call, f"{call - 2:08x}", f"page={call - 2}", str(call - 2))
        for call in range(2, 12)
    ] + [(12, "ffffffff", "header", "")]
    if [(int(call), prefix, kind, page) for call, prefix, kind, page in uploads] != expected_uploads:
        fail(f"unexpected RGPIO upload order: {uploads}")
    packets = [(out / "packets" / f"call-{i:02}.bin").read_bytes() for i in range(1, 13)]
    if any(len(packet) != 0x1004 for packet in packets):
        fail("unexpected RGPIO packet size")
    if struct.unpack_from("<I", packets[0])[0] != 0xFFFFFFFF:
        fail("missing first sentinel")
    if [struct.unpack_from("<I", packet)[0] for packet in packets[1:11]] != list(range(10)):
        fail("unexpected packet page prefixes")
    if struct.unpack_from("<I", packets[-1])[0] != 0xFFFFFFFF:
        fail("missing final sentinel")
    slot, offset1, offset2, rate, length = struct.unpack_from("<IIIII", packets[-1], 4)
    if (slot, offset1, offset2, rate, length) != (300, 64, 64, 46875, expected_length):
        fail(f"unexpected final sentinel metadata: {slot}, {offset1}, {offset2}, {rate}, {length}")
    raw = b"".join(packet[4:] for packet in packets[1:11])
    image = bytearray(raw)
    for i in range(0, len(image), 2):
        image[i], image[i + 1] = image[i + 1], image[i]
    with wave.open(str(source_path), "rb") as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getcomptype()) != (
            1, 2, 46875, "NONE"
        ):
            fail(f"unexpected source WAV format: {source_path}")
        source = wav.readframes(wav.getnframes())
    samples = struct.unpack(f"<{len(source) // 2}h", source)
    converted = [round(sample / 32768 * 32767) for sample in samples]
    if len(source) != length:
        fail(f"source PCM length {len(source):#x} differs from header {length:#x}")
    if struct.unpack_from(">II", image, 4) != (length, rate):
        fail("uploaded sample-file header disagrees with final sentinel")
    uploaded = struct.unpack(f">{len(converted)}h", image[offset1 : offset1 + length])
    if list(uploaded) != converted:
        fail("uploaded PCM differs from converted source WAV")
    match = re.search(
        r"first sample-note edge: block (\d+), instruction (\d+), track (\d+), note=(0x[0-9a-f]+)",
        log,
    )
    if not match:
        fail("modeled DSP log has no sample-note edge")
    block, instruction, track, note = match.groups()
    if int(track) != 2 or int(note, 16) != 0x2D00:
        fail(f"unexpected sample trigger: track={track}, note={note}")
    with wave.open(str(out / "sample.wav"), "rb") as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getcomptype()) != (
            1, 2, 48000, "NONE"
        ):
            fail("modeled WAV is not mono 16-bit PCM at 48 kHz")
        pcm = wav.readframes(wav.getnframes())
    actual = struct.unpack(f"<{len(pcm) // 2}h", pcm)
    start = int(block) * 32
    end = start + len(converted)
    if end > len(actual) or list(actual[start:end]) != converted:
        fail("modeled WAV does not contain the complete converted source PCM")
    if any(actual[:start]) or any(actual[end:]):
        fail("modeled WAV contains unexpected audio outside the sample")
    digest = hashlib.sha256((out / "sample.wav").read_bytes()).hexdigest()
    print(f"PASS {source_path}: {len(converted)} exact PCM frames at {start}; "
          f"trigger instruction={instruction}; WAV SHA-256={digest}")


if __name__ == "__main__":
    main()
