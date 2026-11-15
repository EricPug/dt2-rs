# Third-party notices

## aPLib-style format constants

The offset bias, reuse-gamma value, and far-offset threshold used by the
independent section-codec implementation in
[`crates/dt2-fw/src/aplib.rs`](crates/dt2-fw/src/aplib.rs) were taken from the
public format description in
[`mischa85/elektron-firmware-tool`](https://github.com/mischa85/elektron-firmware-tool).
They match the constants in the upstream
[`aplib.h`](https://github.com/mischa85/elektron-firmware-tool/blob/main/aplib.h).
That project's
[`PROVENANCE.md`](https://github.com/mischa85/elektron-firmware-tool/blob/main/PROVENANCE.md)
describes its codec as independently implemented from a format description.
Its repository `LICENSE` is MIT. This repository does not include its codec
source code; the Rust implementation remains the project's own implementation.

Copyright (c) 2026 Marcel Bierling

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Excluded historical integration

The optional Unicorn/QEMU integration and its patch are excluded from this
release candidate. The patch was identified in the development documentation
as GPL-2.0 material; it is not included here and is not relicensed.

The local sample WAVs used by UI smoke tests are also excluded. Their
redistribution rights have not been established.
