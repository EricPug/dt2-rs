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
Its [`LICENSE`](https://github.com/mischa85/elektron-firmware-tool/blob/main/LICENSE)
is MIT. This repository does not include its codec source code; the Rust
implementation remains the project's own implementation.

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

## Historical Unicorn/QEMU integration

Earlier commits contain an optional Unicorn/QEMU integration and patch. The
integration documentation identifies the patch as a derivative of
Unicorn/QEMU under GPL-2.0, not the project's MIT license. The integration and
patch have been removed from the current tree, but remain in the Git history.
See `tools/unicorn/README.md` in earlier commits for its original notice and
[LICENSES/GPL-2.0.txt](LICENSES/GPL-2.0.txt) for the license text.

## Panel layout measurements

The interactive panel's control coordinates were derived from Elekfake's
reference measurements. No Elekfake panel image is bundled.

## Local sample WAVs

The local sample WAVs used by UI smoke tests are excluded. Their
redistribution rights have not been established.
