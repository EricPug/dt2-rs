use std::collections::BTreeMap;
use std::path::PathBuf;

const RGPIO_DATA: u32 = 0x8c00_0002;
const UPLOAD_BYTES: usize = 4 + 0x1000;
const UPLOAD_WRITES: usize = UPLOAD_BYTES * 2;
const SAMPLE_MACHINE: u16 = 2;
const TRACKS: usize = 16;
const FRAMES_PER_DSPI2_BLOCK: usize = 32;
const SAMPLE_RATE: u32 = 48_000;
const SAMPLE_PREFIX_BYTES: usize = 6;

#[derive(Debug, Clone, Copy)]
pub struct Trigger {
    pub block: u64,
    pub instruction: usize,
    pub track: usize,
    pub note: u16,
}

pub struct ModeledDsp {
    output_path: PathBuf,
    upload_active: bool,
    gpio_writes: Vec<u8>,
    pages: BTreeMap<u32, Vec<u8>>,
    sample_length: Option<usize>,
    sample: Vec<i16>,
    previous_notes: [u16; TRACKS],
    playing_position: Option<usize>,
    audio: Vec<i16>,
    triggers: Vec<Trigger>,
    dspi2_blocks: u64,
    upload_calls: u64,
    error: Option<String>,
}

impl ModeledDsp {
    pub fn new(output_path: PathBuf) -> Self {
        Self {
            output_path,
            upload_active: false,
            gpio_writes: Vec::with_capacity(UPLOAD_WRITES),
            pages: BTreeMap::new(),
            sample_length: None,
            sample: Vec::new(),
            previous_notes: [0; TRACKS],
            playing_position: None,
            audio: Vec::new(),
            triggers: Vec::new(),
            dspi2_blocks: 0,
            upload_calls: 0,
            error: None,
        }
    }

    pub fn begin_upload_call(&mut self) {
        self.finish_upload_call();
        self.upload_active = true;
        self.gpio_writes.clear();
    }

    pub fn write_rgpio_data(&mut self, address: u32, value: u16) {
        if self.upload_active && address == RGPIO_DATA {
            self.gpio_writes.push((value >> 8) as u8);
            if self.gpio_writes.len() == UPLOAD_WRITES {
                self.finish_upload_call();
            }
        }
    }

    pub fn process_dspi2_frame(&mut self, words: &[u16], instruction: usize) {
        if self.error.is_some() {
            return;
        }
        let frame_data: Vec<u16> = words.iter().skip(1).step_by(2).copied().collect();
        if frame_data.len() < 0x94 / 2 + TRACKS {
            self.error = Some(format!(
                "DSPI2 frame has {} data halfwords; at least {} are required",
                frame_data.len(),
                0x94 / 2 + TRACKS
            ));
            return;
        }

        for track in 0..TRACKS {
            let note = frame_data[1 + track];
            let machine = frame_data[0x94 / 2 + track];
            if machine == SAMPLE_MACHINE && self.previous_notes[track] == 0 && note != 0 {
                let trigger = Trigger {
                    block: self.dspi2_blocks,
                    instruction,
                    track,
                    note,
                };
                self.triggers.push(trigger);
                if self.sample.is_empty() {
                    self.error = Some(format!(
                        "sample-machine note edge on track {track} before an uploaded sample was complete"
                    ));
                    return;
                }
                self.playing_position = Some(0);
            }
            self.previous_notes[track] = note;
        }

        for _ in 0..FRAMES_PER_DSPI2_BLOCK {
            let sample = self.playing_position.and_then(|position| {
                let value = self.sample.get(position).copied();
                if value.is_some() {
                    self.playing_position = Some(position + 1);
                } else {
                    self.playing_position = None;
                }
                value
            });
            self.audio.push(sample.unwrap_or(0));
        }
        self.dspi2_blocks += 1;
    }

    pub fn finish(&mut self) -> Result<String, String> {
        self.finish_upload_call();
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        if self.sample.is_empty() {
            return Err(format!(
                "no complete sample image was received over RGPIO ({} indexed page(s))",
                self.pages.len()
            ));
        }
        if self.triggers.is_empty() {
            return Err("no sample-machine note edge was observed in DSPI2 frames".to_owned());
        }
        if !self.audio.iter().any(|sample| *sample != 0) {
            return Err("modeled DSP WAV would contain only zero samples".to_owned());
        }

        write_wav(&self.output_path, &self.audio)?;
        let first = self.triggers[0];
        Ok(format!(
            "MODELED_DSP wrote {} mono PCM frame(s) at {} Hz to {}; sample={} frame(s), {} RGPIO upload call(s), {} DSPI2 block(s); first sample-note edge: block {}, instruction {}, track {}, note={:#06x}",
            self.audio.len(),
            SAMPLE_RATE,
            self.output_path.display(),
            self.sample.len(),
            self.upload_calls,
            self.dspi2_blocks,
            first.block,
            first.instruction,
            first.track,
            first.note
        ))
    }

    fn finish_upload_call(&mut self) {
        if !self.upload_active {
            return;
        }
        self.upload_active = false;
        if self.gpio_writes.len() != UPLOAD_WRITES {
            self.error.get_or_insert_with(|| {
                format!(
                    "RGPIO upload call had {} data-register writes; expected {UPLOAD_WRITES}",
                    self.gpio_writes.len()
                )
            });
            return;
        }

        let mut bytes = Vec::with_capacity(UPLOAD_BYTES);
        for pair in self.gpio_writes.chunks_exact(2) {
            if pair[0] != pair[1] {
                self.error.get_or_insert_with(|| {
                    format!(
                        "RGPIO upload byte strobe disagreed: {:#04x} then {:#04x}",
                        pair[0], pair[1]
                    )
                });
                return;
            }
            bytes.push(pair[0]);
        }

        let prefix: [u8; 4] = bytes[..4].try_into().expect("fixed upload prefix length");
        let body = bytes[4..].to_vec();
        let index = u32::from_le_bytes(prefix);
        self.upload_calls += 1;
        let call = self.upload_calls;
        if index == u32::MAX {
            let slot = u32::from_le_bytes(body[..4].try_into().expect("4-byte slot"));
            let length = u32::from_le_bytes(body[16..20].try_into().expect("4-byte sample length"));
            if length != 0 {
                self.sample_length = Some(length as usize);
                self.rebuild_sample();
            }
            println!(
                "MODELED_DSP RGPIO call={call} prefix={index:08x} header slot={slot} sample_length={:#x}",
                length
            );
        } else if index < 64 {
            println!(
                "MODELED_DSP RGPIO call={call} prefix={index:08x} page={index} bytes={}",
                body.len()
            );
            self.pages.insert(index, body);
            self.rebuild_sample();
        } else {
            self.error
                .get_or_insert_with(|| format!("unsupported RGPIO upload page index {index}"));
        }
    }

    fn rebuild_sample(&mut self) {
        let Some(length) = self.sample_length else {
            return;
        };
        let page_count = length.div_ceil(0x1000);
        let mut image = Vec::with_capacity(page_count * 0x1000);
        for index in 0..page_count {
            let Some(page) = self.pages.get(&(index as u32)) else {
                return;
            };
            image.extend_from_slice(page);
        }
        image.truncate(length);
        if image.len() < SAMPLE_PREFIX_BYTES || (image.len() - SAMPLE_PREFIX_BYTES) % 2 != 0 {
            self.error = Some(format!(
                "uploaded sample length {length:#x} cannot contain the {SAMPLE_PREFIX_BYTES}-byte sample prefix and 16-bit PCM"
            ));
            return;
        }
        for pair in image[SAMPLE_PREFIX_BYTES..].chunks_exact_mut(2) {
            pair.swap(0, 1);
        }
        self.sample = image[SAMPLE_PREFIX_BYTES..]
            .chunks_exact(2)
            .map(|pair| i16::from_be_bytes([pair[0], pair[1]]))
            .collect();
    }
}

fn write_wav(path: &PathBuf, samples: &[i16]) -> Result<(), String> {
    use std::io::Write;

    let data_len = u32::try_from(samples.len().saturating_mul(2))
        .map_err(|_| "modeled DSP WAV exceeds RIFF's 4 GiB limit".to_owned())?;
    let riff_len = 36u32
        .checked_add(data_len)
        .ok_or_else(|| "modeled DSP WAV exceeds RIFF's 4 GiB limit".to_owned())?;
    let mut wav = Vec::with_capacity(riff_len as usize + 8);
    wav.write_all(b"RIFF")
        .and_then(|_| wav.write_all(&riff_len.to_le_bytes()))
        .and_then(|_| wav.write_all(b"WAVEfmt "))
        .and_then(|_| wav.write_all(&16u32.to_le_bytes()))
        .and_then(|_| wav.write_all(&1u16.to_le_bytes()))
        .and_then(|_| wav.write_all(&1u16.to_le_bytes()))
        .and_then(|_| wav.write_all(&SAMPLE_RATE.to_le_bytes()))
        .and_then(|_| wav.write_all(&(SAMPLE_RATE * 2).to_le_bytes()))
        .and_then(|_| wav.write_all(&2u16.to_le_bytes()))
        .and_then(|_| wav.write_all(&16u16.to_le_bytes()))
        .and_then(|_| wav.write_all(b"data"))
        .and_then(|_| wav.write_all(&data_len.to_le_bytes()))
        .map_err(|error| format!("cannot build modeled DSP WAV header: {error}"))?;
    for sample in samples {
        wav.write_all(&sample.to_le_bytes())
            .map_err(|error| format!("cannot encode modeled DSP WAV: {error}"))?;
    }
    std::fs::write(path, wav)
        .map_err(|error| format!("cannot write modeled DSP WAV {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{ModeledDsp, RGPIO_DATA, UPLOAD_WRITES};
    use std::path::PathBuf;

    fn send_call(dsp: &mut ModeledDsp, prefix: u32, body: &[u8]) {
        let mut packet = prefix.to_le_bytes().to_vec();
        packet.extend_from_slice(body);
        assert_eq!(packet.len(), 4 + 0x1000);
        dsp.begin_upload_call();
        for byte in packet {
            let value = u16::from(byte) << 8;
            dsp.write_rgpio_data(RGPIO_DATA, value);
            dsp.write_rgpio_data(RGPIO_DATA, value);
        }
        assert_eq!(dsp.gpio_writes.len(), UPLOAD_WRITES);
    }

    #[test]
    fn uploaded_pages_are_reassembled_and_played_from_a_sample_note_edge() {
        let mut dsp = ModeledDsp::new(PathBuf::from("unused.wav"));
        let mut page = vec![0; 0x1000];
        page[..10].copy_from_slice(&[0x00, 0x01, 0xfc, 0xff, 0xfc, 0xff, 0x34, 0x12, 0xfe, 0xff]);
        send_call(&mut dsp, 0, &page);
        dsp.finish_upload_call();

        let mut header = vec![0; 0x1000];
        header[..4].copy_from_slice(&300u32.to_le_bytes());
        header[16..20].copy_from_slice(&10u32.to_le_bytes());
        send_call(&mut dsp, u32::MAX, &header);
        dsp.finish_upload_call();
        assert_eq!(dsp.sample, [0x1234, -2]);

        let mut words = vec![0; 2748];
        for pair in words.chunks_exact_mut(2) {
            pair[0] = 0x8001;
        }
        words[2 * (1 + 2) + 1] = 0x2d00;
        words[2 * (74 + 2) + 1] = 2;
        dsp.process_dspi2_frame(&words, 1234);

        assert_eq!(dsp.triggers.len(), 1);
        assert_eq!(dsp.triggers[0].track, 2);
        assert_eq!(dsp.triggers[0].instruction, 1234);
        assert_eq!(&dsp.audio[..4], &[0x1234, -2, 0, 0]);
    }
}
