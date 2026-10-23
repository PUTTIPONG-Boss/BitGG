//! tgui - Tiny Game UI. An immediate-mode, 8-bit pixel-art widget kit that
//! draws into a low-res u32 framebuffer which is then integer-scaled to the
//! window (nearest neighbour). No GPU, no fonts, no allocations per frame.

use font8x8::legacy::BASIC_LEGACY;

pub const W: usize = 400;
pub const H: usize = 250;

/// PICO-8 palette.
pub mod pal {
    pub const BLACK: u32 = 0x000000;
    pub const NAVY: u32 = 0x1D2B53;
    pub const PURPLE: u32 = 0x7E2553;
    pub const DGREEN: u32 = 0x008751;
    pub const BROWN: u32 = 0xAB5236;
    pub const DGREY: u32 = 0x5F574F;
    pub const LGREY: u32 = 0xC2C3C7;
    pub const WHITE: u32 = 0xFFF1E8;
    pub const RED: u32 = 0xFF004D;
    pub const ORANGE: u32 = 0xFFA300;
    pub const YELLOW: u32 = 0xFFEC27;
    pub const GREEN: u32 = 0x00E436;
    pub const BLUE: u32 = 0x29ADFF;
    pub const LAVENDER: u32 = 0x83769C;
    pub const PINK: u32 = 0xFF77A8;
    pub const PEACH: u32 = 0xFFCCAA;
}
use pal::*;

#[derive(Default, Clone, Copy)]
pub struct Input {
    pub mx: i32,
    pub my: i32,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub wheel: i32,
    pub rclick: bool,
}

pub struct Ui {
    pub px: Vec<u32>,
    pub inp: Input,
    pub active: u64, // widget being dragged
    pub hot: u64,
    pub tooltip: Option<String>,
    pub frame: u64,
}

#[derive(Clone, Copy)]
pub struct R {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub const fn r(x: i32, y: i32, w: i32, h: i32) -> R {
    R { x, y, w, h }
}

impl R {
    pub fn has(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

pub fn id(s: &str, n: usize) -> u64 {
    // FNV-1a
    let mut h: u64 = 0xcbf29ce484222325 ^ (n as u64).wrapping_mul(0x9E3779B97F4A7C15);
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Ui {
    pub fn new() -> Ui {
        Ui { px: vec![0; W * H], inp: Input::default(), active: 0, hot: 0, tooltip: None, frame: 0 }
    }

    pub fn begin(&mut self) {
        self.hot = 0;
        self.tooltip = None;
        self.frame += 1;
    }

    pub fn end(&mut self) {
        if !self.inp.down {
            self.active = 0;
        }
        self.inp.pressed = false;
        self.inp.released = false;
        self.inp.wheel = 0;
        self.inp.rclick = false;
    }

    // ------------------------------------------------------------ drawing

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, c: u32) {
        if x >= 0 && y >= 0 && (x as usize) < W && (y as usize) < H {
            self.px[y as usize * W + x as usize] = c;
        }
    }

    pub fn fill(&mut self, q: R, c: u32) {
        let x0 = q.x.max(0) as usize;
        let y0 = q.y.max(0) as usize;
        let x1 = ((q.x + q.w).max(0) as usize).min(W);
        let y1 = ((q.y + q.h).max(0) as usize).min(H);
        for y in y0..y1 {
            self.px[y * W + x0..y * W + x1.max(x0)].fill(c);
        }
    }

    pub fn clear(&mut self, c: u32) {
        self.px.fill(c);
    }

    pub fn hline(&mut self, x: i32, y: i32, w: i32, c: u32) {
        self.fill(r(x, y, w, 1), c);
    }
    pub fn vline(&mut self, x: i32, y: i32, h: i32, c: u32) {
        self.fill(r(x, y, 1, h), c);
    }

    /// Chunky 8-bit box: 1px outline with notched corners.
    pub fn boxed(&mut self, q: R, bg: u32, border: u32) {
        self.fill(r(q.x + 1, q.y + 1, q.w - 2, q.h - 2), bg);
        self.hline(q.x + 1, q.y, q.w - 2, border);
        self.hline(q.x + 1, q.y + q.h - 1, q.w - 2, border);
        self.vline(q.x, q.y + 1, q.h - 2, border);
        self.vline(q.x + q.w - 1, q.y + 1, q.h - 2, border);
    }

    /// Bevelled raised panel (light top-left, dark bottom-right).
    pub fn bevel(&mut self, q: R, bg: u32, hi: u32, lo: u32) {
        self.fill(q, bg);
        self.hline(q.x, q.y, q.w, hi);
        self.vline(q.x, q.y, q.h, hi);
        self.hline(q.x, q.y + q.h - 1, q.w, lo);
        self.vline(q.x + q.w - 1, q.y, q.h, lo);
    }

    pub fn glyph(&mut self, x: i32, y: i32, ch: char, c: u32) {
        let i = ch as usize;
        let g = if i < 128 { BASIC_LEGACY[i] } else { BASIC_LEGACY[b'?' as usize] };
        for (row, bits) in g.iter().enumerate() {
            for col in 0..8 {
                if bits & (1 << col) != 0 {
                    self.put(x + col, y + row as i32, c);
                }
            }
        }
    }

    /// 8x8 text. Returns width in pixels.
    pub fn text(&mut self, x: i32, y: i32, s: &str, c: u32) -> i32 {
        let mut cx = x;
        for ch in s.chars() {
            self.glyph(cx, y, ch.to_ascii_uppercase(), c);
            cx += 8;
        }
        cx - x
    }

    /// Tiny 3x5 pixel text for version numbers: digits, '.', 'v', '-'.
    /// Returns width in pixels.
    pub fn text_tiny(&mut self, x: i32, y: i32, s: &str, c: u32) -> i32 {
        // each row: 3 bits, bit 2 = leftmost pixel
        fn glyph(ch: char) -> Option<(i32, [u8; 5])> {
            Some(match ch {
                '0' => (3, [0b111, 0b101, 0b101, 0b101, 0b111]),
                '1' => (3, [0b010, 0b110, 0b010, 0b010, 0b111]),
                '2' => (3, [0b111, 0b001, 0b111, 0b100, 0b111]),
                '3' => (3, [0b111, 0b001, 0b111, 0b001, 0b111]),
                '4' => (3, [0b101, 0b101, 0b111, 0b001, 0b001]),
                '5' => (3, [0b111, 0b100, 0b111, 0b001, 0b111]),
                '6' => (3, [0b111, 0b100, 0b111, 0b101, 0b111]),
                '7' => (3, [0b111, 0b001, 0b001, 0b010, 0b010]),
                '8' => (3, [0b111, 0b101, 0b111, 0b101, 0b111]),
                '9' => (3, [0b111, 0b101, 0b111, 0b001, 0b111]),
                'v' | 'V' => (3, [0b000, 0b000, 0b101, 0b101, 0b010]),
                '-' => (3, [0b000, 0b000, 0b111, 0b000, 0b000]),
                '.' => (1, [0b000, 0b000, 0b000, 0b000, 0b100]),
                _ => return None,
            })
        }
        let mut cx = x;
        for ch in s.chars() {
            let Some((w, rows)) = glyph(ch) else { continue };
            for (ry, bits) in rows.iter().enumerate() {
                for col in 0..w {
                    if bits & (0b100 >> col) != 0 {
                        self.put(cx + col, y + ry as i32, c);
                    }
                }
            }
            cx += w + 1;
        }
        cx - x
    }

    /// Text with a 1px drop shadow — the classic arcade look.
    pub fn text_s(&mut self, x: i32, y: i32, s: &str, c: u32) -> i32 {
        self.text(x + 1, y + 1, s, BLACK);
        self.text(x, y, s, c)
    }

    pub fn text_c(&mut self, q: R, s: &str, c: u32) {
        let w = s.chars().count() as i32 * 8;
        self.text(q.x + (q.w - w) / 2, q.y + (q.h - 8) / 2, s, c);
    }

    /// Text clipped to `max` characters (adds '~').
    pub fn text_fit(&mut self, x: i32, y: i32, s: &str, max: usize, c: u32) {
        if s.chars().count() <= max {
            self.text(x, y, s, c);
        } else {
            let t: String = s.chars().take(max.saturating_sub(1)).collect();
            let w = self.text(x, y, &t, c);
            self.glyph(x + w, y, '~', c);
        }
    }

    // ------------------------------------------------------------ widgets

    fn hover(&self, q: R) -> bool {
        q.has(self.inp.mx, self.inp.my)
    }

    pub fn button(&mut self, q: R, label: &str, accent: u32) -> bool {
        let hov = self.hover(q);
        let pressed = hov && self.inp.down;
        let (bg, fg) = if pressed { (accent, BLACK) } else if hov { (NAVY, accent) } else { (BLACK, LGREY) };
        let off = if pressed { 1 } else { 0 };
        if !pressed {
            self.fill(r(q.x + 1, q.y + q.h, q.w - 1, 1), DGREY); // drop shadow
            self.fill(r(q.x + q.w, q.y + 1, 1, q.h - 1), DGREY);
        }
        self.boxed(r(q.x + off, q.y + off, q.w, q.h), bg, if hov { accent } else { DGREY });
        self.text_c(r(q.x + off, q.y + off, q.w, q.h), label, fg);
        hov && self.inp.released
    }

    /// Toggle: lit LED style.
    pub fn toggle(&mut self, q: R, label: &str, on: &mut bool, accent: u32) -> bool {
        let hov = self.hover(q);
        let bg = if *on { accent } else { BLACK };
        let fg = if *on { BLACK } else if hov { accent } else { LGREY };
        self.boxed(q, bg, if hov || *on { accent } else { DGREY });
        self.text_c(q, label, fg);
        if hov && self.inp.released {
            *on = !*on;
            return true;
        }
        false
    }

    /// Small "< value >" spinner. Returns -1 / +1 when an arrow is clicked.
    pub fn spinner(&mut self, q: R, label: &str, accent: u32) -> i32 {
        let mut d = 0;
        if self.button(r(q.x, q.y, 12, q.h), "<", accent) {
            d = -1;
        }
        if self.button(r(q.x + q.w - 12, q.y, 12, q.h), ">", accent) {
            d = 1;
        }
        let inner = r(q.x + 14, q.y, q.w - 28, q.h);
        self.boxed(inner, BLACK, DGREY);
        let max = ((inner.w - 4) / 8).max(1) as usize;
        let s: String = label.chars().take(max).collect();
        self.text_c(inner, &s, WHITE);
        if self.hover(inner) && self.inp.wheel != 0 {
            d = -self.inp.wheel.signum();
        }
        d
    }

    /// Vertical fader with block-segment fill. Value 0..1.
    pub fn vslider(&mut self, wid: u64, q: R, v: &mut f32, accent: u32) -> bool {
        let hov = self.hover(q);
        if hov && self.inp.pressed {
            self.active = wid;
        }
        let mut changed = false;
        if self.active == wid && self.inp.down {
            let t = 1.0 - (self.inp.my - q.y - 2) as f32 / (q.h - 4) as f32;
            let nv = t.clamp(0.0, 1.0);
            changed = nv != *v;
            *v = nv;
        }
        if hov && self.inp.wheel != 0 {
            *v = (*v + self.inp.wheel as f32 * 0.03).clamp(0.0, 1.0);
            changed = true;
        }
        self.boxed(q, BLACK, if hov || self.active == wid { accent } else { DGREY });
        let inner_h = q.h - 4;
        let segs = inner_h / 3;
        let lit = (*v * segs as f32).round() as i32;
        for s in 0..segs {
            let y = q.y + q.h - 3 - (s + 1) * 3 + 1;
            let c = if s < lit { accent } else { NAVY };
            self.fill(r(q.x + 2, y, q.w - 4, 2), c);
        }
        changed
    }

    /// Horizontal slider, value 0..1.
    pub fn hslider(&mut self, wid: u64, q: R, v: &mut f32, accent: u32) -> bool {
        let hov = self.hover(q);
        if hov && self.inp.pressed {
            self.active = wid;
        }
        let mut changed = false;
        if self.active == wid && self.inp.down {
            let t = (self.inp.mx - q.x - 2) as f32 / (q.w - 4) as f32;
            let nv = t.clamp(0.0, 1.0);
            changed = nv != *v;
            *v = nv;
        }
        if hov && self.inp.wheel != 0 {
            *v = (*v + self.inp.wheel as f32 * 0.02).clamp(0.0, 1.0);
            changed = true;
        }
        self.boxed(q, BLACK, if hov || self.active == wid { accent } else { DGREY });
        let segs = (q.w - 4) / 3;
        let lit = (*v * segs as f32).round() as i32;
        for s in 0..segs {
            let c = if s < lit { accent } else { NAVY };
            self.fill(r(q.x + 2 + s * 3, q.y + 2, 2, q.h - 4), c);
        }
        changed
    }

    /// Vertical LED meter (0..1 linear amplitude, drawn in dB-ish scale).
    pub fn meter(&mut self, q: R, amp: f32) {
        self.fill(q, BLACK);
        let db = 20.0 * amp.max(1e-5).log10();
        let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        let segs = q.h / 2;
        let lit = (t * segs as f32) as i32;
        for s in 0..segs {
            let y = q.y + q.h - (s + 1) * 2;
            let frac = s as f32 / segs as f32;
            let on = if frac > 0.9 { RED } else if frac > 0.7 { YELLOW } else { GREEN };
            let off = if frac > 0.9 { 0x3a0011 } else if frac > 0.7 { 0x3a3500 } else { 0x00351a };
            self.fill(r(q.x, y, q.w, 1), if s < lit { on } else { off });
        }
    }

    pub fn tip(&mut self, q: R, s: &str) {
        if self.hover(q) {
            self.tooltip = Some(s.to_string());
        }
    }

    pub fn clicked_in(&self, q: R) -> bool {
        self.hover(q) && self.inp.released
    }

    pub fn wheel_in(&self, q: R) -> i32 {
        if self.hover(q) {
            self.inp.wheel
        } else {
            0
        }
    }

    /// Blit the low-res buffer into a window-sized buffer with integer scale,
    /// centred with a border. Returns (scale, off_x, off_y) for mouse mapping.
    pub fn blit(&self, dst: &mut [u32], dw: usize, dh: usize) -> (usize, usize, usize) {
        let s = (dw / W).min(dh / H).max(1);
        let ox = dw.saturating_sub(W * s) / 2;
        let oy = dh.saturating_sub(H * s) / 2;
        dst.fill(0x0b0f1f);
        for y in 0..H {
            let src = &self.px[y * W..y * W + W];
            let ty0 = oy + y * s;
            if ty0 >= dh {
                break;
            }
            // build first scaled row, then copy it s-1 times
            let row_start = ty0 * dw + ox;
            {
                let row = &mut dst[row_start..(row_start + (W * s).min(dw - ox))];
                for (x, &c) in src.iter().enumerate() {
                    let a = x * s;
                    if a >= row.len() {
                        break;
                    }
                    let b = (a + s).min(row.len());
                    row[a..b].fill(c);
                }
            }
            for k in 1..s {
                let ty = ty0 + k;
                if ty >= dh {
                    break;
                }
                let len = (W * s).min(dw - ox);
                dst.copy_within(row_start..row_start + len, ty * dw + ox);
            }
        }
        (s, ox, oy)
    }
}
