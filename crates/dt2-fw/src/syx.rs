//! SysEx transport of a firmware file.
//!
//! A file is a start message (`F0 00 20 3C 14 00 7F 01 ..`), a run of 128-byte data messages
//! (`F0 00 20 3C 14 00 7E 00 <ctr hi> <ctr lo> <payload> <check> F7`) and an end message
//! (`.. 7F 02 ..`). The 14-bit counter rises by one per data message. The payload is packed
//! seven data bytes to eight: a byte holding the high bits of the next seven (bit 6 first),
//! then those seven with their high bits cleared.

#[derive(Debug, PartialEq, Eq)]
pub enum SyxError {
    NotSysex(usize),
    BadHeader(usize),
    NoData,
    CounterGap {
        index: usize,
        expected: u16,
        found: u16,
    },
}

const HEAD: [u8; 6] = [0xF0, 0x00, 0x20, 0x3C, 0x14, 0x00];
const DATA: u8 = 0x7E;
/// Bytes before the payload: header 7, a zero 1, counter 2.
const PAYLOAD_AT: usize = 10;
/// The byte before F7. Its rule is not worked out, so it is not checked.
const TRAILER: usize = 2;

pub struct Syx {
    /// The packed stream, unpacked: an 8-byte preamble followed by the container.
    pub stream: Vec<u8>,
    pub messages: usize,
    pub first_counter: u16,
}

pub fn unpack8in7(p: &[u8], out: &mut Vec<u8>) {
    for g in p.chunks(8) {
        let hi = g[0];
        for (n, &b) in g[1..].iter().enumerate() {
            out.push((b & 0x7F) | (((hi >> (6 - n)) & 1) << 7));
        }
    }
}

pub fn decode(file: &[u8]) -> Result<Syx, SyxError> {
    let mut stream = Vec::with_capacity(file.len());
    let (mut pos, mut messages, mut expect) = (0, 0, None);
    let mut first = 0;
    while pos < file.len() {
        if file[pos] != 0xF0 {
            return Err(SyxError::NotSysex(pos));
        }
        let end = file[pos..]
            .iter()
            .position(|&b| b == 0xF7)
            .ok_or(SyxError::NotSysex(pos))?
            + pos;
        let m = &file[pos..=end];
        pos = end + 1;
        if m.len() < 7 || m[..6] != HEAD {
            return Err(SyxError::BadHeader(pos));
        }
        if m[6] != DATA {
            continue;
        }
        if m.len() < PAYLOAD_AT + TRAILER {
            return Err(SyxError::BadHeader(pos));
        }
        let ctr = (m[8] as u16) << 7 | m[9] as u16;
        if let Some(e) = expect {
            if ctr != e {
                return Err(SyxError::CounterGap {
                    index: messages,
                    expected: e,
                    found: ctr,
                });
            }
        } else {
            first = ctr;
        }
        expect = Some(ctr + 1);
        unpack8in7(&m[PAYLOAD_AT..m.len() - TRAILER], &mut stream);
        messages += 1;
    }
    if messages == 0 {
        return Err(SyxError::NoData);
    }
    Ok(Syx {
        stream,
        messages,
        first_counter: first,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpack_restores_high_bits() {
        // High-bit byte 0b0100000: the second data byte of seven has bit 7 set.
        let mut out = vec![];
        unpack8in7(&[0b0100000, 1, 2, 3, 4, 5, 6, 7], &mut out);
        assert_eq!(out, [1, 2 | 0x80, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn partial_group_has_fewer_bytes() {
        let mut out = vec![];
        unpack8in7(&[0b1000000, 0x11, 0x22], &mut out);
        assert_eq!(out, [0x11 | 0x80, 0x22]);
    }

    #[test]
    fn rejects_counter_gap() {
        let msg = |ctr: u16| {
            let mut m = vec![
                0xF0,
                0,
                0x20,
                0x3C,
                0x14,
                0,
                0x7E,
                0,
                (ctr >> 7) as u8,
                (ctr & 0x7F) as u8,
            ];
            m.extend_from_slice(&[0, 1, 2, 0xF7]);
            m
        };
        let file = [msg(5), msg(7)].concat();
        assert_eq!(
            decode(&file).err(),
            Some(SyxError::CounterGap {
                index: 1,
                expected: 6,
                found: 7
            })
        );
    }
}
