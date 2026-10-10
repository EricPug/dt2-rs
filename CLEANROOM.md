# Clean-room rules

dt2-rs is an emulator of the Elektron Digitakt II, initially targeting OS 1.17, written from scratch in Rust.

- Checked-in files must never name, link to, or otherwise identify one specific external
  emulator project, its repository, contributors, or associated files. Keep any necessary
  context in local notes excluded from Git.
- Emulator code, tests, and generated tables are implemented independently from public manuals;
  do not copy, translate, or adapt material from another emulator.
- Hardware facts (addresses, register behaviour, handshakes) may be taken from notes and
  papers, but are re-stated in our own words and checked by our own tests.
- Primary sources are public manuals: the MCF5441x Reference Manual, the ColdFire Family
  Programmer's Reference Manual, and Analog Devices' SHARC+ manuals. Cite the manual and page
  next to any behaviour that is not obvious.
- Firmware and anything derived from it (`*.syx`, section dumps, snapshots) is Elektron's
  copyright. It is never committed. Users supply their own `.syx`.

## Third-party format sources

The aPLib-style section codec's constants (offset bias, reuse code, far-offset threshold) come
from the public description in mischa85/elektron-firmware-tool (MIT). The code here is our own
implementation, and each constant was checked against a real firmware file: every packed
section ends exactly at its stored length and matches its byte sum.
