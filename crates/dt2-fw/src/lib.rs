//! Elektron firmware file handling: SysEx transport, ELE3 container, aPLib sections.
//!
//! The layouts here were read off a user-supplied Digitakt II OS 1.17 file, not taken from
//! any other project.

pub mod aplib;
pub mod container;
pub mod syx;
