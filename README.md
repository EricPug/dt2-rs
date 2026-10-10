# dt2-rs

A Digitakt II emulator in Rust. Not affiliated with or endorsed by Elektron. See
[CLEANROOM.md](CLEANROOM.md) for how it is built.

Status: early. The firmware loader parses OS 1.17; the ColdFire V4e core runs a
small instruction subset and is not yet executing firmware. See
[research/HANDOFF.md](research/HANDOFF.md) for current decisions and next steps,
and [research/](research/) for initial 1.17 observations and scoped prior-research leads.

```sh
cargo test
```
