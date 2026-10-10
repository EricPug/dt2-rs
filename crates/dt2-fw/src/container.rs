//! The ELE3 container inside a firmware stream.
//!
//! After an 8-byte preamble comes `ELE3`, a version string, a section count at +0x1C and a table
//! at +0x20 of 16-byte entries `{id, offset, size, dest}` (big-endian u32, offsets from `ELE3`).
//! A packed section starts with a big-endian u32 length of the stream and a u32 sum of its bytes.

use crate::aplib;

const BASE: usize = 8;

#[derive(Debug, PartialEq, Eq)]
pub enum ContainerError {
    NoMagic,
    Truncated,
    Section { id: u32, why: String },
}

#[derive(Debug, Clone)]
pub struct Section {
    pub id: u32,
    pub dest: u32,
    pub packed: bool,
    /// Section bytes as stored (including the length word when packed).
    pub stored: std::ops::Range<usize>,
}

pub struct Container<'a> {
    pub stream: &'a [u8],
    pub version: String,
    pub sections: Vec<Section>,
}

fn be32(b: &[u8], at: usize) -> Result<u32, ContainerError> {
    b.get(at..at + 4)
        .map(|x| u32::from_be_bytes(x.try_into().unwrap()))
        .ok_or(ContainerError::Truncated)
}

pub fn parse(stream: &[u8]) -> Result<Container<'_>, ContainerError> {
    if stream.get(BASE..BASE + 4) != Some(b"ELE3") {
        return Err(ContainerError::NoMagic);
    }
    let text = String::from_utf8_lossy(&stream[BASE + 4..BASE + 0x18]).replace('\0', " ");
    let version = text.split_whitespace().last().unwrap_or("").to_string();
    let count = be32(stream, BASE + 0x1C)? as usize;
    let mut sections = Vec::new();
    for i in 0..count {
        let e = BASE + 0x20 + 16 * i;
        let (id, off, size, dest) = (
            be32(stream, e)?,
            be32(stream, e + 4)?,
            be32(stream, e + 8)?,
            be32(stream, e + 12)?,
        );
        let start = BASE + off as usize;
        let range = start..start + size as usize;
        let body = stream.get(range.clone()).ok_or(ContainerError::Truncated)?;
        // Packed iff a length word and byte sum describe the section, bar a little padding.
        let packed = body.len() >= 8 && {
            let l = be32(body, 0)? as usize;
            l + 8 <= body.len()
                && body.len() - (l + 8) < 16
                && body[8..8 + l]
                    .iter()
                    .fold(0u32, |a, &b| a.wrapping_add(b as u32))
                    == be32(body, 4)?
        };
        sections.push(Section {
            id,
            dest,
            packed,
            stored: range,
        });
    }
    Ok(Container {
        stream,
        version,
        sections,
    })
}

impl Container<'_> {
    /// The section's contents, unpacked if it is packed.
    pub fn unpack(&self, s: &Section) -> Result<Vec<u8>, ContainerError> {
        let body = &self.stream[s.stored.clone()];
        if !s.packed {
            return Ok(body.to_vec());
        }
        let l = be32(body, 0)? as usize;
        let (out, used) = aplib::depack(&body[8..8 + l]).map_err(|e| ContainerError::Section {
            id: s.id,
            why: format!("{e:?}"),
        })?;
        if used != l {
            return Err(ContainerError::Section {
                id: s.id,
                why: format!("stream used {used} of {l} bytes"),
            });
        }
        Ok(out)
    }
}
