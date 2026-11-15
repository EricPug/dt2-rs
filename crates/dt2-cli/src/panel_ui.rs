//! Interactive schematic controls for the live diagnostic viewer.
//!
//! The grid is a neutral input map, not a measured or hardware-accurate faceplate.

use std::collections::BTreeSet;

pub const PANEL_WIDTH: usize = 900;
pub const PANEL_HEIGHT: usize = 800;
const GLASS: (f32, f32, f32, f32) = (250.0, 70.0, 400.0, 220.0);
const GRID_X: f32 = 85.0;
const GRID_Y: f32 = 340.0;
const GRID_COLS: f32 = 8.0;
const GRID_DX: f32 = 104.0;
const GRID_DY: f32 = 60.0;
const DEFAULT_COUNTS_PER_DEGREE: f32 = 30.0;
const CLICK_SLOP_PIXELS: f32 = 3.0;
const WHEEL_DEGREES_PER_NOTCH: f32 = 15.0;
static COUNTS_PER_DEGREE_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn set_counts_per_degree(counts: f32) {
    COUNTS_PER_DEGREE_BITS.store(
        counts.max(0.0).to_bits(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn counts_per_degree() -> f32 {
    match f32::from_bits(COUNTS_PER_DEGREE_BITS.load(std::sync::atomic::Ordering::Relaxed)) {
        counts if counts > 0.0 => counts,
        _ => DEFAULT_COUNTS_PER_DEGREE,
    }
}

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
    code: u8,
    encoder: Option<u8>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    shape: Shape,
}

const fn control_position(code: u8) -> (f32, f32) {
    let index = code.saturating_sub(1) as f32;
    (
        GRID_X + (index % GRID_COLS) * GRID_DX,
        GRID_Y + (index / GRID_COLS) * GRID_DY,
    )
}

const fn key(label: &'static str, code: u8) -> Control {
    let (x, y) = control_position(code);
    Control {
        label,
        code,
        encoder: None,
        x,
        y,
        w: 72.0,
        h: 38.0,
        shape: Shape::Rect,
    }
}

const fn knob(label: &'static str, encoder: u8, code: u8) -> Control {
    let (x, y) = control_position(code);
    Control {
        label,
        code,
        encoder: Some(encoder),
        x,
        y,
        w: 42.0,
        h: 42.0,
        shape: Shape::Ellipse,
    }
}

const CONTROLS: &[Control] = &[
    key("TRIG", 1),
    key("SRC", 2),
    key("FLTR", 3),
    key("AMP", 4),
    key("FX", 5),
    key("MOD", 6),
    key("PRST", 7),
    key("SET", 8),
    key("TEMPO", 9),
    key("YES", 10),
    key("UP", 11),
    key("NO", 12),
    key("LEFT", 13),
    key("DOWN", 14),
    key("RIGHT", 15),
    key("TRK", 16),
    key("FUNC", 17),
    key("KB", 18),
    key("REC", 19),
    key("PLAY", 20),
    key("STOP", 21),
    key("PAGE", 22),
    key("PTN", 23),
    key("SONG", 24),
    key("1", 25),
    key("2", 26),
    key("3", 27),
    key("4", 28),
    key("5", 29),
    key("6", 30),
    key("7", 31),
    key("8", 32),
    key("9", 33),
    key("10", 34),
    key("11", 35),
    key("12", 36),
    key("13", 37),
    key("14", 38),
    key("15", 39),
    key("16", 40),
    knob("A", 0, 41),
    knob("B", 1, 42),
    knob("C", 2, 43),
    knob("D", 3, 44),
    knob("E", 4, 45),
    knob("F", 5, 46),
    knob("G", 6, 47),
    knob("H", 7, 48),
    knob("LEVEL", 8, 49),
    key("SAMP", 50),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Press(u8),
    Release(u8),
    Turn(u8, i8),
}

#[derive(Default)]
pub struct PanelUi {
    latched: BTreeSet<u8>,
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
    CONTROLS
        .iter()
        .position(|control| contains(control, position))
}

impl PanelUi {
    pub fn is_lit(&self, code: u8) -> bool {
        self.latched.contains(&code)
            || self
                .left
                .as_ref()
                .is_some_and(|press| press.held_code == Some(code))
    }

    pub fn release_all(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.latched)
            .into_iter()
            .map(Action::Release)
            .collect()
    }

    pub fn handle_mouse(
        &mut self,
        position: Option<(f32, f32)>,
        left: bool,
        right: bool,
        wheel: f32,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        let hovered = position.and_then(control_at);

        if wheel != 0.0 {
            if let Some(encoder) = hovered.and_then(|index| CONTROLS[index].encoder) {
                let counts = (WHEEL_DEGREES_PER_NOTCH * counts_per_degree()).round() as i32;
                push_turns(
                    &mut actions,
                    encoder,
                    if wheel > 0.0 { counts } else { -counts },
                );
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

    pub fn render(
        &self,
        display: Option<(usize, usize, &[u32])>,
        leds: &dyn Fn(u8) -> Option<u32>,
    ) -> Vec<u32> {
        let mut out = vec![0x0018_2028; PANEL_WIDTH * PANEL_HEIGHT];
        draw_text(&mut out, 24, 20, "SYNTHETIC CONTROL MAP", 3, 0x00FF_FFFF);
        draw_text(
            &mut out,
            24,
            286,
            "INPUT GRID - NOT TO SCALE",
            2,
            0x00A0_B0C0,
        );
        for control in CONTROLS {
            draw_control(
                &mut out,
                control,
                self.is_lit(control.code),
                led_for_code(control.code).and_then(leds),
            );
        }
        if let Some((width, height, pixels)) = display {
            let (gx, gy, gw, gh) = GLASS;
            fill_rect(
                &mut out,
                gx as isize - 4,
                gy as isize - 4,
                (gx + gw) as isize + 4,
                (gy + gh) as isize + 4,
                0x0000_0000,
            );
            fill_rect(
                &mut out,
                gx as isize,
                gy as isize,
                (gx + gw) as isize,
                (gy + gh) as isize,
                0x0008_1018,
            );
            let target_w = gw.min(gh * width as f32 / height as f32);
            let target_h = target_w * height as f32 / width as f32;
            let (x0, y0) = (gx + (gw - target_w) * 0.5, gy + (gh - target_h) * 0.5);
            for y in 0..target_h as usize {
                for x in 0..target_w as usize {
                    let source =
                        (y * height / target_h as usize) * width + x * width / target_w as usize;
                    put(
                        &mut out,
                        x0 as isize + x as isize,
                        y0 as isize + y as isize,
                        pixels[source],
                    );
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

fn fill_rect(buffer: &mut [u32], left: isize, top: isize, right: isize, bottom: isize, color: u32) {
    for y in top..bottom {
        for x in left..right {
            put(buffer, x, y, color);
        }
    }
}

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
    let left = (control.x - control.w * 0.5) as isize;
    let top = (control.y - control.h * 0.5) as isize;
    let right = left + control.w as isize;
    let bottom = top + control.h as isize;
    let fill = match (led, lit) {
        (Some(color), _) => color,
        (None, true) => 0x00A0_7A10,
        (None, false) => 0x0040_4040,
    };
    for y in top..bottom {
        for x in left..right {
            let edge = match control.shape {
                Shape::Rect => x == left || x == right - 1 || y == top || y == bottom - 1,
                Shape::Ellipse => {
                    let dx = (x as f32 + 0.5 - control.x) / (control.w * 0.5);
                    let dy = (y as f32 + 0.5 - control.y) / (control.h * 0.5);
                    dx * dx + dy * dy >= 0.78
                }
            };
            let inside = match control.shape {
                Shape::Rect => true,
                Shape::Ellipse => {
                    let dx = (x as f32 + 0.5 - control.x) / (control.w * 0.5);
                    let dy = (y as f32 + 0.5 - control.y) / (control.h * 0.5);
                    dx * dx + dy * dy <= 1.0
                }
            };
            if inside {
                put(buffer, x, y, if edge { 0x00B0_B0B0 } else { fill });
            }
        }
    }
    let scale = if control.label.len() <= 5 { 2 } else { 1 };
    let text_w = (control.label.len() * 4 - 1) as isize * scale;
    draw_text(
        buffer,
        (left + right) / 2 - text_w / 2,
        (top + bottom) / 2 - 5 * scale / 2,
        control.label,
        scale,
        0x00FF_FFFF,
    );
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
                            put(
                                buffer,
                                x + (index as isize * 4 + col) * scale + dx,
                                y + row as isize * scale + dy,
                                color,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(label: &str) -> (f32, f32) {
        let control = CONTROLS
            .iter()
            .find(|control| control.label == label)
            .unwrap();
        (control.x, control.y)
    }

    #[test]
    fn left_click_is_momentary_and_right_click_latches() {
        let mut ui = PanelUi::default();
        let func = at("FUNC");
        assert_eq!(
            ui.handle_mouse(Some(func), false, true, 0.0),
            vec![Action::Press(17)]
        );
        assert_eq!(ui.handle_mouse(Some(func), false, false, 0.0), vec![]);
        assert!(ui.is_lit(17));

        let trig1 = at("1");
        assert_eq!(
            ui.handle_mouse(Some(trig1), true, false, 0.0),
            vec![Action::Press(25)]
        );
        assert_eq!(
            ui.handle_mouse(Some(trig1), false, false, 0.0),
            vec![Action::Release(25)]
        );
        assert!(ui.is_lit(17));
        assert_eq!(
            ui.handle_mouse(Some((2.0, 2.0)), false, true, 0.0),
            vec![Action::Release(17)]
        );
        assert!(!ui.is_lit(17));
    }

    #[test]
    fn knob_drag_turns_wheel_turns_and_click_pushes() {
        let mut ui = PanelUi::default();
        let (x, y) = at("B");
        assert_eq!(ui.handle_mouse(Some((x, y)), true, false, 0.0), vec![]);
        assert_eq!(
            ui.handle_mouse(Some((x, y - 12.0)), true, false, 0.0),
            vec![
                Action::Turn(1, 127),
                Action::Turn(1, 127),
                Action::Turn(1, 106)
            ]
        );
        assert_eq!(
            ui.handle_mouse(Some((x, y - 12.0)), false, false, 0.0),
            vec![]
        );
        assert_eq!(
            ui.handle_mouse(Some((x, y)), false, false, -1.0),
            vec![
                Action::Turn(1, -127),
                Action::Turn(1, -127),
                Action::Turn(1, -127),
                Action::Turn(1, -69)
            ]
        );
        ui.handle_mouse(Some((x, y)), true, false, 0.0);
        assert_eq!(
            ui.handle_mouse(Some((x, y)), false, false, 0.0),
            vec![Action::Press(42), Action::Release(42)]
        );
    }

    #[test]
    fn rendering_fits_display_into_schematic_screen() {
        let ui = PanelUi::default();
        let display = vec![0x00FF_FFFF; 8 * 4];
        let frame = ui.render(Some((8, 4, &display)), &|_| None);
        assert_eq!(frame.len(), PANEL_WIDTH * PANEL_HEIGHT);
        assert_eq!(frame[120 * PANEL_WIDTH + 270], 0x00FF_FFFF);
    }
}
