//! Clickable Digitakt II faceplate for the live viewer.
//!
//! Control positions come from the Elekfake reference measurements (panel photo coordinates; the
//! visible panel is the 1048x856 crop starting at `CROP_LEFT`/`CROP_TOP`). Mouse handling:
//! left press = momentary button, left drag on a knob = turn it, left click on a knob = encoder push,
//! mouse wheel over a knob = turn it, right click = latch/unlatch a button (hold one key while
//! pressing another), right click on empty space or Escape = release every latched key.

use std::collections::BTreeSet;

pub const PANEL_WIDTH: usize = 1048;
pub const PANEL_HEIGHT: usize = 856;
const CROP_LEFT: f32 = 152.0;
const CROP_TOP: f32 = 78.0;
const GLASS: (f32, f32, f32, f32) = (335.0, 217.0, 289.0, 163.0);
/// A full knob turn is about 350 window pixels of drag, so one pixel is one degree. 30 counts is all the
/// firmware takes per UI tick (see `PanelInput`), so a fast drag saturates it.
const DEFAULT_COUNTS_PER_DEGREE: f32 = 30.0;
const CLICK_SLOP_PIXELS: f32 = 3.0;
const WHEEL_DEGREES_PER_NOTCH: f32 = 15.0;
static COUNTS_PER_DEGREE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Raw encoder counts sent per pixel (degree) of drag; 0 or less restores the default. One count
/// moves a displayed value by roughly 0.006, so a degree needs several.
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn set_counts_per_degree(counts: f32) {
    COUNTS_PER_DEGREE_BITS.store(counts.max(0.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

fn counts_per_degree() -> f32 {
    match f32::from_bits(COUNTS_PER_DEGREE_BITS.load(std::sync::atomic::Ordering::Relaxed)) {
        counts if counts > 0.0 => counts,
        _ => DEFAULT_COUNTS_PER_DEGREE,
    }
}

/// Splits a signed count into messages that fit the wire's signed byte.
fn push_turns(actions: &mut Vec<Action>, encoder: u8, mut counts: i32) {
    while counts != 0 {
        let step = counts.clamp(-127, 127);
        actions.push(Action::Turn(encoder, step as i8));
        counts -= step;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Rect,
    Ellipse,
}

struct Control {
    label: &'static str,
    /// Firmware key code pressed by this control (encoder push for knobs).
    code: u8,
    /// Encoder index (0-7 = A-H, 8 = LEVEL/DATA) for knobs.
    encoder: Option<u8>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    shape: Shape,
}

const fn key(label: &'static str, code: u8, x: f32, y: f32, w: f32, h: f32) -> Control {
    Control { label, code, encoder: None, x, y, w, h, shape: Shape::Rect }
}

const fn knob(label: &'static str, encoder: u8, code: u8, x: f32, y: f32, d: f32) -> Control {
    Control { label, code, encoder: Some(encoder), x, y, w: d, h: d, shape: Shape::Ellipse }
}

const fn trig(label: &'static str, step: u8, x: f32, y: f32) -> Control {
    key(label, 24 + step, x, y, 84.0, 84.0)
}

/// Positions are control centres in panel-photo coordinates.
const CONTROLS: &[Control] = &[
    knob("LEVEL", 8, 49, 232.0, 358.0, 58.0),
    knob("A", 0, 41, 729.0, 235.0, 60.0),
    knob("B", 1, 42, 858.0, 235.0, 60.0),
    knob("C", 2, 43, 987.0, 235.0, 60.0),
    knob("D", 3, 44, 1119.0, 235.0, 60.0),
    knob("E", 4, 45, 729.0, 358.0, 60.0),
    knob("F", 5, 46, 858.0, 358.0, 60.0),
    knob("G", 6, 47, 987.0, 358.0, 60.0),
    knob("H", 7, 48, 1119.0, 358.0, 60.0),
    key("FUNC", 17, 249.0, 516.0, 84.0, 53.0),
    key("PRST", 7, 365.0, 516.0, 58.0, 58.0),
    key("SET", 8, 443.0, 516.0, 58.0, 58.0),
    key("SAMP", 50, 522.0, 516.0, 58.0, 58.0),
    key("TEMPO", 9, 602.0, 516.0, 58.0, 58.0),
    key("KB", 18, 249.0, 594.0, 54.0, 54.0),
    key("REC", 19, 380.0, 595.0, 86.0, 56.0),
    key("PLAY", 20, 483.0, 595.0, 86.0, 56.0),
    key("STOP", 21, 587.0, 595.0, 86.0, 56.0),
    key("TRK", 16, 249.0, 683.0, 86.0, 54.0),
    key("PTN", 23, 249.0, 766.0, 86.0, 54.0),
    key("SONG", 24, 249.0, 849.0, 86.0, 54.0),
    key("TRIG", 1, 727.0, 465.0, 58.0, 58.0),
    key("SRC", 2, 805.0, 465.0, 58.0, 58.0),
    key("FLTR", 3, 883.0, 465.0, 58.0, 58.0),
    key("AMP", 4, 961.0, 465.0, 58.0, 58.0),
    key("FX", 5, 1040.0, 465.0, 58.0, 58.0),
    key("MOD", 6, 1118.0, 465.0, 58.0, 58.0),
    key("YES", 10, 727.0, 556.0, 58.0, 54.0),
    key("NO", 12, 727.0, 634.0, 58.0, 54.0),
    key("UP", 11, 883.0, 556.0, 58.0, 54.0),
    key("DOWN", 14, 883.0, 634.0, 58.0, 54.0),
    key("LEFT", 13, 804.0, 634.0, 58.0, 54.0),
    key("RIGHT", 15, 962.0, 634.0, 58.0, 54.0),
    key("PAGE", 22, 1102.0, 634.0, 84.0, 54.0),
    trig("1", 1, 380.0, 736.0),
    trig("2", 2, 484.0, 736.0),
    trig("3", 3, 587.0, 736.0),
    trig("4", 4, 690.0, 736.0),
    trig("5", 5, 793.0, 736.0),
    trig("6", 6, 896.0, 736.0),
    trig("7", 7, 1000.0, 736.0),
    trig("8", 8, 1102.0, 736.0),
    trig("9", 9, 380.0, 846.0),
    trig("10", 10, 484.0, 846.0),
    trig("11", 11, 587.0, 846.0),
    trig("12", 12, 690.0, 846.0),
    trig("13", 13, 793.0, 846.0),
    trig("14", 14, 896.0, 846.0),
    trig("15", 15, 1000.0, 846.0),
    trig("16", 16, 1102.0, 846.0),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press(u8),
    Release(u8),
    Turn(u8, i8),
}

#[derive(Default)]
pub struct PanelUi {
    backdrop: Option<Vec<u32>>,
    latched: BTreeSet<u8>,
    /// Control under the left button: (index, press x/y, y at last turn, moved, accumulated drag).
    left: Option<LeftPress>,
    left_was_down: bool,
    right_was_down: bool,
}

struct LeftPress {
    control: usize,
    start: (f32, f32),
    last_y: f32,
    moved: bool,
    remainder: f32,
    held_code: Option<u8>,
}

fn screen_to_photo(position: (f32, f32)) -> (f32, f32) {
    (position.0 + CROP_LEFT, position.1 + CROP_TOP)
}

fn contains(control: &Control, point: (f32, f32)) -> bool {
    let (dx, dy) = (point.0 - control.x, point.1 - control.y);
    match control.shape {
        Shape::Rect => dx.abs() <= control.w * 0.5 && dy.abs() <= control.h * 0.5,
        Shape::Ellipse => {
            let (rx, ry) = (control.w * 0.5, control.h * 0.5);
            (dx / rx).powi(2) + (dy / ry).powi(2) <= 1.0
        }
    }
}

fn control_at(position: (f32, f32)) -> Option<usize> {
    let point = screen_to_photo(position);
    CONTROLS.iter().position(|control| contains(control, point))
}

impl PanelUi {
    /// `backdrop` is the 1048x856 panel crop as 0x00RRGGBB pixels; without it the panel is drawn from the control table.
    pub fn new(backdrop: Option<Vec<u32>>) -> Self {
        Self { backdrop, ..Self::default() }
    }

    pub fn is_lit(&self, code: u8) -> bool {
        self.latched.contains(&code) || self.left.as_ref().is_some_and(|press| press.held_code == Some(code))
    }

    /// Releases every latched key (Escape, or a right click on empty panel).
    pub fn release_all(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.latched).into_iter().map(Action::Release).collect()
    }

    /// One mouse sample. `position` is in window pixels, `None` when the pointer is outside the window.
    pub fn handle_mouse(&mut self, position: Option<(f32, f32)>, left: bool, right: bool, wheel: f32) -> Vec<Action> {
        let mut actions = Vec::new();
        let hovered = position.and_then(control_at);

        if wheel != 0.0 {
            if let Some(encoder) = hovered.and_then(|index| CONTROLS[index].encoder) {
                let counts = (WHEEL_DEGREES_PER_NOTCH * counts_per_degree()).round() as i32;
                push_turns(&mut actions, encoder, if wheel > 0.0 { counts } else { -counts });
            }
        }

        if left && !self.left_was_down {
            if let (Some(index), Some(position)) = (hovered, position) {
                let control = &CONTROLS[index];
                let held_code = control.encoder.is_none().then_some(control.code);
                if let Some(code) = held_code {
                    if !self.latched.contains(&code) {
                        actions.push(Action::Press(code));
                    }
                }
                self.left = Some(LeftPress {
                    control: index,
                    start: position,
                    last_y: position.1,
                    moved: false,
                    remainder: 0.0,
                    held_code,
                });
            }
        } else if left {
            if let (Some(press), Some(position)) = (self.left.as_mut(), position) {
                if let Some(encoder) = CONTROLS[press.control].encoder {
                    let travelled = (position.0 - press.start.0).hypot(position.1 - press.start.1);
                    press.moved |= travelled > CLICK_SLOP_PIXELS;
                    press.remainder += (press.last_y - position.1) * counts_per_degree();
                    let detents = press.remainder.trunc();
                    if detents != 0.0 && press.moved {
                        press.remainder -= detents;
                        push_turns(&mut actions, encoder, detents as i32);
                    }
                    press.last_y = position.1;
                }
            }
        } else if let Some(press) = self.left.take() {
            let control = &CONTROLS[press.control];
            if let Some(code) = press.held_code {
                if !self.latched.contains(&code) {
                    actions.push(Action::Release(code));
                }
            } else if !press.moved && !self.latched.contains(&control.code) {
                actions.push(Action::Press(control.code));
                actions.push(Action::Release(control.code));
            }
        }
        self.left_was_down = left;

        if right && !self.right_was_down {
            match hovered {
                Some(index) => {
                    let code = CONTROLS[index].code;
                    if self.latched.remove(&code) {
                        actions.push(Action::Release(code));
                    } else {
                        self.latched.insert(code);
                        actions.push(Action::Press(code));
                    }
                }
                None => actions.extend(self.release_all()),
            }
        }
        self.right_was_down = right;
        actions
    }

    /// Draws the panel into a `PANEL_WIDTH` x `PANEL_HEIGHT` buffer with the live display in the glass area.
    pub fn render(&self, display: Option<(usize, usize, &[u32])>, leds: &dyn Fn(u8) -> Option<u32>) -> Vec<u32> {
        let mut out = match &self.backdrop {
            Some(backdrop) => backdrop.clone(),
            None => vec![0x0020_2020; PANEL_WIDTH * PANEL_HEIGHT],
        };
        for control in CONTROLS {
            let lit = self.is_lit(control.code);
            let led = led_for_code(control.code).and_then(leds);
            if self.backdrop.is_none() {
                draw_control(&mut out, control, lit, led);
            } else if lit || led.is_some() {
                tint_control(&mut out, control, led.unwrap_or(0x00FF_D23C));
            }
        }
        if let Some((width, height, pixels)) = display {
            let (gx, gy, gw, gh) = GLASS;
            let (gx, gy) = (gx - CROP_LEFT, gy - CROP_TOP);
            let target_w = gw.min(gh * width as f32 / height as f32);
            let target_h = target_w * height as f32 / width as f32;
            let (x0, y0) = (gx + (gw - target_w) * 0.5, gy + (gh - target_h) * 0.5);
            for y in 0..target_h as usize {
                for x in 0..target_w as usize {
                    let source = (y * height / target_h as usize) * width + x * width / target_w as usize;
                    put(&mut out, (x0 as usize + x) as isize, (y0 as usize + y) as isize, pixels[source]);
                }
            }
        }
        out
    }
}

fn put(buffer: &mut [u32], x: isize, y: isize, color: u32) {
    if (0..PANEL_WIDTH as isize).contains(&x) && (0..PANEL_HEIGHT as isize).contains(&y) {
        buffer[y as usize * PANEL_WIDTH + x as usize] = color;
    }
}

fn blend(buffer: &mut [u32], x: isize, y: isize, color: u32, alpha: u32) {
    if (0..PANEL_WIDTH as isize).contains(&x) && (0..PANEL_HEIGHT as isize).contains(&y) {
        let old = buffer[y as usize * PANEL_WIDTH + x as usize];
        let mix = |shift: u32| (((old >> shift & 0xFF) * (255 - alpha) + (color >> shift & 0xFF) * alpha) / 255) << shift;
        buffer[y as usize * PANEL_WIDTH + x as usize] = mix(16) | mix(8) | mix(0);
    }
}

fn bounds(control: &Control) -> (isize, isize, isize, isize) {
    let left = (control.x - CROP_LEFT - control.w * 0.5) as isize;
    let top = (control.y - CROP_TOP - control.h * 0.5) as isize;
    (left, top, left + control.w as isize, top + control.h as isize)
}

fn inside(control: &Control, x: isize, y: isize) -> bool {
    contains(control, (x as f32 + CROP_LEFT + 0.5, y as f32 + CROP_TOP + 0.5))
}

fn tint_control(buffer: &mut [u32], control: &Control, color: u32) {
    let (left, top, right, bottom) = bounds(control);
    for y in top..bottom {
        for x in left..right {
            if inside(control, x, y) {
                blend(buffer, x, y, color, 110);
            }
        }
    }
}

/// LED id (as sent in UART8 `b9 id value` records) for a key code. Partly inferred from
/// key-press captures, so some keys without an entry here have no LED shown yet.
fn led_for_code(code: u8) -> Option<u8> {
    match code {
        1..=6 => Some(18 + code),
        8 => Some(5),
        9 => Some(6),
        19 => Some(9),
        20 => Some(10),
        21 => Some(11),
        23 => Some(3),
        24 => Some(4),
        25..=40 => Some(code),
        _ => None,
    }
}

fn draw_control(buffer: &mut [u32], control: &Control, lit: bool, led: Option<u32>) {
    let (left, top, right, bottom) = bounds(control);
    let fill = match (led, lit) {
        (Some(color), _) => color,
        (None, true) => 0x00A0_7A10,
        (None, false) => 0x0040_4040,
    };
    for y in top..bottom {
        for x in left..right {
            if !inside(control, x, y) {
                continue;
            }
            let edge = !inside(control, x - 1, y) || !inside(control, x + 1, y) || !inside(control, x, y - 1) || !inside(control, x, y + 1);
            put(buffer, x, y, if edge { 0x00B0_B0B0 } else { fill });
        }
    }
    let scale = if control.label.len() <= 5 { 2 } else { 1 };
    let text_w = (control.label.len() * 4 - 1) as isize * scale;
    draw_text(buffer, (left + right) / 2 - text_w / 2, (top + bottom) / 2 - 5 * scale / 2, control.label, scale, 0x00FF_FFFF);
}

fn glyph(c: char) -> [u8; 5] {
    match c {
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 7, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [6, 1, 2, 4, 7],
        '3' => [6, 1, 2, 1, 6],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 6, 1, 6],
        '6' => [3, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 6],
        _ => [0; 5],
    }
}

fn draw_text(buffer: &mut [u32], x: isize, y: isize, text: &str, scale: isize, color: u32) {
    for (index, c) in text.chars().enumerate() {
        for (row, bits) in glyph(c).iter().enumerate() {
            for col in 0..3 {
                if bits >> (2 - col) & 1 == 1 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(buffer, x + (index as isize * 4 + col) * scale + dx, y + row as isize * scale + dy, color);
                        }
                    }
                }
            }
        }
    }
}

/// Decodes a panel photo and crops it to the visible 1048x856 panel. The photo is not part of this repository.
#[cfg(feature = "window")]
pub fn load_backdrop(path: &str) -> Result<Vec<u32>, String> {
    let image = image::open(path).map_err(|error| format!("{path}: {error}"))?.to_rgb8();
    let (left, top) = (CROP_LEFT as u32, CROP_TOP as u32);
    if image.width() < left + PANEL_WIDTH as u32 || image.height() < top + PANEL_HEIGHT as u32 {
        return Err(format!("{path}: expected at least {}x{}", left + PANEL_WIDTH as u32, top + PANEL_HEIGHT as u32));
    }
    let mut out = Vec::with_capacity(PANEL_WIDTH * PANEL_HEIGHT);
    for y in 0..PANEL_HEIGHT as u32 {
        for x in 0..PANEL_WIDTH as u32 {
            let [r, g, b] = image.get_pixel(left + x, top + y).0;
            out.push(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(label: &str) -> (f32, f32) {
        let control = CONTROLS.iter().find(|control| control.label == label).unwrap();
        (control.x - CROP_LEFT, control.y - CROP_TOP)
    }

    #[test]
    fn left_click_is_momentary_and_right_click_latches() {
        let mut ui = PanelUi::default();
        let func = at("FUNC");
        assert_eq!(ui.handle_mouse(Some(func), false, true, 0.0), vec![Action::Press(17)]);
        assert_eq!(ui.handle_mouse(Some(func), false, false, 0.0), vec![]);
        assert!(ui.is_lit(17));

        let trig1 = at("1");
        assert_eq!(ui.handle_mouse(Some(trig1), true, false, 0.0), vec![Action::Press(25)]);
        assert_eq!(ui.handle_mouse(Some(trig1), false, false, 0.0), vec![Action::Release(25)]);
        assert!(ui.is_lit(17));

        assert_eq!(ui.handle_mouse(Some((2.0, 2.0)), false, true, 0.0), vec![Action::Release(17)]);
        assert!(!ui.is_lit(17));
    }

    #[test]
    fn knob_drag_turns_wheel_turns_and_click_pushes() {
        let mut ui = PanelUi::default();
        let (x, y) = at("B");
        assert_eq!(ui.handle_mouse(Some((x, y)), true, false, 0.0), vec![]);
        assert_eq!(ui.handle_mouse(Some((x, y - 12.0)), true, false, 0.0), vec![Action::Turn(1, 127), Action::Turn(1, 127), Action::Turn(1, 106)]);
        assert_eq!(ui.handle_mouse(Some((x, y - 12.0)), false, false, 0.0), vec![]);

        assert_eq!(ui.handle_mouse(Some((x, y)), false, false, -1.0), vec![Action::Turn(1, -127), Action::Turn(1, -127), Action::Turn(1, -127), Action::Turn(1, -69)]);

        ui.handle_mouse(Some((x, y)), true, false, 0.0);
        assert_eq!(ui.handle_mouse(Some((x, y)), false, false, 0.0), vec![Action::Press(42), Action::Release(42)]);
    }

    #[test]
    fn rendering_fits_display_into_glass() {
        let ui = PanelUi::default();
        let display = vec![0x00FF_FFFF; 8 * 4];
        let frame = ui.render(Some((8, 4, &display)), &|_| None);
        assert_eq!(frame.len(), PANEL_WIDTH * PANEL_HEIGHT);
        let (gx, gy, ..) = GLASS;
        assert_eq!(frame[(gy - CROP_TOP) as usize * PANEL_WIDTH + (gx - CROP_LEFT) as usize + 10 + 40 * PANEL_WIDTH], 0x00FF_FFFF);
    }
}
