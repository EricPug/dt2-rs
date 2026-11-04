//! UART8 panel-display stream decoder: 10-byte tile records into a 64x128 1-bit framebuffer.
//! The stream grammar is reverse-engineered from captures, not from a specification.

pub const FB_WIDTH: usize = 64;
pub const FB_HEIGHT: usize = 128;

pub struct Display {
    pixels: Vec<bool>,
    pending: Vec<u8>,
    needed: usize,
    pub updates: u64,
    pub unknown_bytes: u64,
    palette: [[u8; 3]; 256],
    leds: [u8; 64],
    leds_seen: bool,
}

impl Display {
    pub fn new() -> Self {
        Self {
            pixels: vec![false; FB_WIDTH * FB_HEIGHT],
            pending: Vec::new(),
            needed: 0,
            updates: 0,
            unknown_bytes: 0,
            palette: [[0; 3]; 256],
            leds: [0; 64],
            leds_seen: false,
        }
    }

    pub fn has_led_state(&self) -> bool {
        self.leds_seen
    }

    /// Colour (0x00RRGGBB) of an LED, or `None` when it is unset or shows the dim idle palette entries.
    pub fn led_rgb(&self, id: u8) -> Option<u32> {
        let [r, g, b] = self.palette[usize::from(self.leds[usize::from(id & 63)])];
        // Palette channels are 6-bit; entry 1 (4,4,4) is the unlit glow.
        if r.max(g).max(b) <= 8 {
            return None;
        }
        let scale = |c: u8| u32::from(c.min(63)) * 255 / 63;
        Some(scale(r) << 16 | scale(g) << 8 | scale(b))
    }

    pub fn save_led_state(&self, w: &mut crate::snapshot::Writer) {
        let flat: Vec<u8> = self.palette.iter().flatten().copied().collect();
        w.bytes(&flat);
        w.bytes(&self.leds);
    }

    pub fn load_led_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        let flat = r.bytes()?;
        let leds = r.bytes()?;
        if flat.len() != 768 || leds.len() != 64 {
            return Err("snapshot LED state size mismatch".into());
        }
        for (entry, chunk) in self.palette.iter_mut().zip(flat.chunks(3)) {
            entry.copy_from_slice(chunk);
        }
        self.leds.copy_from_slice(leds);
        self.leds_seen = true;
        Ok(())
    }

    pub fn feed(&mut self, byte: u8) {
        if self.needed == 0 {
            self.needed = match byte {
                0xB5 => 5,
                0xB9 => 3,
                0xB7 | 0xBA | 0xBC | 0x60 | 0x70 | 0x73 => 2,
                0xB8 => 1,
                0x10..=0x17 => 10,
                _ => {
                    self.unknown_bytes += 1;
                    return;
                }
            };
            self.pending.clear();
        }
        self.pending.push(byte);
        if self.pending.len() < self.needed {
            return;
        }
        self.needed = 0;
        match self.pending[0] {
            0xB8 => self.updates += 1,
            0xB5 => {
                // `b5 index r g b`
                self.palette[usize::from(self.pending[1])] = [self.pending[2], self.pending[3], self.pending[4]];
                self.leds_seen = true;
            }
            0xB9 => {
                // `b9 led palette_index`
                self.leds[usize::from(self.pending[1] & 63)] = self.pending[2];
                self.leds_seen = true;
            }
            tag @ 0x10..=0x17 => {
                let x0 = usize::from(tag & 7) * 8;
                let y0 = usize::from(self.pending[1]);
                if y0 + 8 <= FB_HEIGHT {
                    for row in 0..8 {
                        let bits = self.pending[2 + row];
                        for col in 0..8 {
                            self.pixels[(y0 + row) * FB_WIDTH + x0 + col] = bits >> (7 - col) & 1 != 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn pixel(&self, x: usize, y: usize) -> bool {
        self.pixels[y * FB_WIDTH + x]
    }

    pub fn save_state(&self, w: &mut crate::snapshot::Writer) {
        let packed: Vec<u8> = self.pixels.iter().map(|&on| u8::from(on)).collect();
        w.bytes(&packed);
        w.bytes(&self.pending);
        w.u32(self.needed as u32);
        w.u64(self.updates);
        w.u64(self.unknown_bytes);
    }

    pub fn load_state(&mut self, r: &mut crate::snapshot::Reader) -> Result<(), String> {
        let packed = r.bytes()?;
        if packed.len() != FB_WIDTH * FB_HEIGHT {
            return Err("snapshot display size mismatch".into());
        }
        self.pixels = packed.iter().map(|&byte| byte != 0).collect();
        self.pending = r.bytes()?.to_vec();
        self.needed = r.u32()? as usize;
        self.updates = r.u64()?;
        self.unknown_bytes = r.u64()?;
        Ok(())
    }

    /// Rotates the framebuffer 90 degrees counter-clockwise (upright reading) and scales by `scale`.
    /// Returns (width, height, 0x00RRGGBB pixels).
    pub fn render_upright(&self, scale: usize) -> (usize, usize, Vec<u32>) {
        let (width, height) = (FB_HEIGHT * scale, FB_WIDTH * scale);
        let mut out = vec![0u32; width * height];
        for y in 0..height {
            for x in 0..width {
                let (ux, uy) = (x / scale, y / scale);
                if self.pixel(FB_WIDTH - 1 - uy, ux) {
                    out[y * width + x] = 0x00FF_FFFF;
                }
            }
        }
        (width, height, out)
    }
}

/// Encodes 0x00RRGGBB pixels as an uncompressed 8-bit grayscale PNG (blue channel used).
pub fn encode_gray_png(width: usize, height: usize, pixels: &[u32]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((width + 1) * height);
    for row in pixels.chunks(width) {
        raw.push(0);
        raw.extend(row.iter().map(|p| (p & 0xFF) as u8));
    }
    let mut zlib = vec![0x78, 0x01];
    let mut blocks = raw.chunks(0xFFFF).peekable();
    while let Some(block) = blocks.next() {
        zlib.push(u8::from(blocks.peek().is_none()));
        zlib.extend((block.len() as u16).to_le_bytes());
        zlib.extend((!(block.len() as u16)).to_le_bytes());
        zlib.extend(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend((width as u32).to_be_bytes());
    header.extend((height as u32).to_be_bytes());
    header.extend([8, 0, 0, 0, 0]);
    for (tag, data) in [(b"IHDR", header), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend((data.len() as u32).to_be_bytes());
        let mut body = tag.to_vec();
        body.extend(data);
        png.extend(&body);
        png.extend(crc32(&body).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(tag: u8, y: u8, rows: [u8; 8]) -> Vec<u8> {
        let mut record = vec![tag, y];
        record.extend(rows);
        record
    }

    #[test]
    fn tile_record_sets_pixels_msb_leftmost() {
        let mut display = Display::new();
        for byte in tile(0x11, 8, [0x80, 0x01, 0, 0, 0, 0, 0, 0xFF]) {
            display.feed(byte);
        }
        assert!(display.pixel(8, 8));
        assert!(!display.pixel(9, 8));
        assert!(display.pixel(15, 9));
        assert!((8..16).all(|x| display.pixel(x, 15)));
    }

    #[test]
    fn palette_commands_and_terminator_are_consumed_by_length() {
        let mut display = Display::new();
        for byte in [0xB5, 1, 2, 3, 4, 0x60, 0x10, 0xBC, 0x01, 0xB8] {
            display.feed(byte);
        }
        assert_eq!((display.updates, display.unknown_bytes), (1, 0));
        // An LED record `b9 idx value` must not be read as a tile tag.
        for byte in [0xB9, 0x10, 0x71] {
            display.feed(byte);
        }
        assert_eq!(display.unknown_bytes, 0);
        display.feed(0xFF);
        assert_eq!(display.unknown_bytes, 1);
    }

    #[test]
    fn led_records_update_palette_and_led_colours() {
        let mut display = Display::new();
        for byte in [0xB5, 0x01, 4, 4, 4, 0xB5, 0x71, 0x3F, 0x3F, 0x3F, 0xB9, 0x14, 0x71, 0xB9, 0x15, 0x01] {
            display.feed(byte);
        }
        assert_eq!(display.led_rgb(0x14), Some(0x00FF_FFFF));
        assert_eq!(display.led_rgb(0x15), None);
        assert_eq!(display.unknown_bytes, 0);
    }

    #[test]
    fn upright_render_rotates_counter_clockwise() {
        let mut display = Display::new();
        // Framebuffer top-left pixel lands at the bottom-left after CCW rotation.
        for byte in tile(0x10, 0, [0x80, 0, 0, 0, 0, 0, 0, 0]) {
            display.feed(byte);
        }
        let (w, h, out) = display.render_upright(1);
        assert_eq!((w, h), (128, 64));
        assert_eq!(out[63 * 128], 0x00FF_FFFF);
        assert_eq!(out.iter().filter(|&&p| p != 0).count(), 1);
    }

    #[test]
    fn snapshot_round_trip_keeps_pixels_and_a_partial_record() {
        let mut display = Display::new();
        for byte in tile(0x12, 16, [0xF0, 0, 0, 0, 0, 0, 0, 1]) {
            display.feed(byte);
        }
        display.feed(0x13);
        display.feed(8);
        let mut writer = crate::snapshot::Writer::new();
        display.save_state(&mut writer);
        let data = writer.into_inner();

        let mut restored = Display::new();
        restored.load_state(&mut crate::snapshot::Reader::new(&data)).unwrap();
        assert!(restored.pixel(16, 16) && restored.pixel(19, 16) && !restored.pixel(20, 16));
        // The half-received record at the snapshot point completes after restore.
        for byte in [0x80, 0, 0, 0, 0, 0, 0, 0] {
            restored.feed(byte);
        }
        assert!(restored.pixel(24, 8));
    }

    #[test]
    fn png_has_signature_and_matching_crc() {
        let png = encode_gray_png(2, 2, &[0, 0xFF, 0xFF, 0]);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }
}
