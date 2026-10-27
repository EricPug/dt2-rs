//! The section codec: an aPLib-style LZ77 with interlaced Elias-gamma lengths and offsets.
//!
//! Stream layout (checked against every packed section of a real 1.17 file): tag bits come
//! most-significant first from a byte fetched on demand; a 1 bit is a literal byte, a 0 bit is a
//! match. The format's constants follow the public description in mischa85/elektron-firmware-tool
//! (MIT); this is a separate implementation.

#[derive(Debug, PartialEq, Eq)]
pub enum AplibError {
    /// The stream ended before the end marker.
    Truncated,
    /// A match pointed before the start of the output, or a gamma value ran away.
    BadStream,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DepackTrace {
    pub output: Vec<u8>,
    pub used: usize,
    pub checkpoints: Vec<(usize, usize)>,
}

/// A raw offset field equal to this is the end marker; anything else has it subtracted.
const OFFSET_BIAS: u32 = 767;
/// Gamma value that means "repeat the last offset".
const REUSE_GAMMA: u32 = 2;
/// Offsets above this get one extra byte of length.
const FAR_THRESHOLD: u32 = 3328;

struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
    tag: u32,
}

impl Bits<'_> {
    fn byte(&mut self) -> Result<u8, AplibError> {
        let b = *self.src.get(self.pos).ok_or(AplibError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    fn bit(&mut self) -> Result<u32, AplibError> {
        self.tag <<= 1;
        if self.tag & 0xFF == 0 {
            let by = self.byte()? as u32;
            self.tag = (by << 1) | 1;
            return Ok(by >> 7);
        }
        Ok((self.tag >> 8) & 1)
    }

    fn gamma(&mut self) -> Result<u32, AplibError> {
        let mut v = 1u32;
        loop {
            v = (v << 1) + self.bit()?;
            if self.bit()? == 1 {
                return Ok(v);
            }
            if v > 0x0200_0000 {
                return Err(AplibError::BadStream);
            }
        }
    }
}

/// Unpack a bare stream. Returns the output and the number of input bytes used.
pub fn depack(src: &[u8]) -> Result<(Vec<u8>, usize), AplibError> {
    let trace = depack_with_checkpoints(src, &[])?;
    Ok((trace.output, trace.used))
}

/// Unpack a bare stream and record the input byte cursor at requested output lengths.
/// Checkpoints are output lengths, not zero-based byte offsets.
pub fn depack_with_checkpoints(
    src: &[u8],
    checkpoints: &[usize],
) -> Result<DepackTrace, AplibError> {
    let mut r = Bits {
        src,
        pos: 0,
        tag: 0,
    };
    let mut out = Vec::with_capacity(src.len() * 3);
    let mut traces = Vec::new();
    let mut next_checkpoint = 0;
    while checkpoints.get(next_checkpoint) == Some(&0) {
        traces.push((0, r.pos));
        next_checkpoint += 1;
    }
    let mut last_off = 1u32;
    loop {
        if r.bit()? == 1 {
            out.push(r.byte()?);
            while checkpoints.get(next_checkpoint) == Some(&out.len()) {
                traces.push((out.len(), r.pos));
                next_checkpoint += 1;
            }
            continue;
        }
        let g = r.gamma()?;
        let off = if g == REUSE_GAMMA {
            last_off
        } else {
            let raw = (g << 8).wrapping_add(r.byte()? as u32);
            if raw == OFFSET_BIAS {
                return Ok(DepackTrace {
                    output: out,
                    used: r.pos,
                    checkpoints: traces,
                });
            }
            last_off = raw.wrapping_sub(OFFSET_BIAS);
            last_off
        };
        let short = 2 * r.bit()? + r.bit()?;
        let mut len = if short != 0 { short } else { r.gamma()? + 2 };
        if off > FAR_THRESHOLD {
            len += 1;
        }
        let off = off as usize;
        if off == 0 || off > out.len() {
            return Err(AplibError::BadStream);
        }
        for _ in 0..=len {
            out.push(out[out.len() - off]);
            while checkpoints.get(next_checkpoint) == Some(&out.len()) {
                traces.push((out.len(), r.pos));
                next_checkpoint += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct W {
        out: Vec<u8>,
        tag_at: usize,
        left: u32,
    }

    impl W {
        fn bit(&mut self, b: u32) {
            if self.left == 0 {
                self.tag_at = self.out.len();
                self.out.push(0);
                self.left = 8;
            }
            self.out[self.tag_at] |= (b as u8) << (self.left - 1);
            self.left -= 1;
        }
        fn gamma(&mut self, v: u32) {
            let nb = 32 - v.leading_zeros();
            for i in (0..nb - 1).rev() {
                self.bit((v >> i) & 1);
                self.bit((i == 0) as u32);
            }
        }
        fn lit(&mut self, b: u8) {
            self.bit(1);
            self.out.push(b);
        }
        fn matched(&mut self, off: Option<u32>, lbase: u32) {
            self.bit(0);
            match off {
                None => self.gamma(REUSE_GAMMA),
                Some(o) => {
                    let raw = o + OFFSET_BIAS;
                    self.gamma(raw >> 8);
                    self.out.push(raw as u8);
                }
            }
            assert!(lbase <= 3);
            self.bit(lbase >> 1);
            self.bit(lbase & 1);
        }
        fn end(mut self) -> Vec<u8> {
            self.bit(0);
            self.gamma(0x0100_0002);
            self.out.push(0xFF);
            self.out
        }
    }

    #[test]
    fn literals_then_end() {
        let mut w = W::default();
        w.lit(b'A');
        w.lit(b'B');
        let s = w.end();
        assert_eq!(depack(&s).unwrap(), (b"AB".to_vec(), s.len()));
        assert_eq!(
            depack_with_checkpoints(&s, &[1, 2]).unwrap().checkpoints,
            [(1, 2), (2, 3)]
        );
    }

    #[test]
    fn match_then_reuse_of_last_offset() {
        let mut w = W::default();
        w.lit(b'A');
        w.lit(b'B');
        w.matched(Some(2), 1); // length 2: "AB"
        w.matched(None, 1); // same offset again
        let s = w.end();
        assert_eq!(depack(&s).unwrap().0, b"ABABAB");
    }

    #[test]
    fn truncated_stream_is_an_error() {
        let mut w = W::default();
        w.lit(b'A');
        let mut s = w.out;
        s.truncate(1);
        assert_eq!(depack(&s).err(), Some(AplibError::Truncated));
    }

    #[test]
    fn offset_before_start_is_rejected() {
        let mut w = W::default();
        w.lit(b'A');
        w.matched(Some(5), 1);
        assert_eq!(depack(&w.end()).err(), Some(AplibError::BadStream));
    }
}
