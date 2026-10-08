//! Tray icon pixels: a coloured disc, optionally with a dark badge carrying a
//! count in a 3×5 bitmap font. Pure function of (size, colour, badge text).

use herdr_status_core::Counts;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const AMBER: Rgb = Rgb(0xff, 0x9f, 0x0a);
pub const BLUE: Rgb = Rgb(0x3b, 0x9d, 0xff);
pub const GREEN: Rgb = Rgb(0x34, 0xc7, 0x59);
pub const GREY: Rgb = Rgb(0x5c, 0x62, 0x70);
const BADGE_BG: Rgb = Rgb(0x1b, 0x1f, 0x27);
const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);

/// Amber if anything needs the user, else blue while working, else green when
/// something finished, else grey.
pub fn colour_for(counts: &Counts) -> Rgb {
    if counts.blocked > 0 {
        AMBER
    } else if counts.working > 0 {
        BLUE
    } else if counts.done > 0 {
        GREEN
    } else {
        GREY
    }
}

pub fn badge_text(blocked: usize) -> Option<String> {
    match blocked {
        0 => None,
        1..=9 => Some(blocked.to_string()),
        _ => Some("9+".to_string()),
    }
}

/// RGBA buffer of `size × size` pixels.
pub fn render(size: u32, colour: Rgb, badge: Option<&str>) -> Vec<u8> {
    let mut buf = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    fill_disc(&mut buf, size, s * 0.5, s * 0.5, s * 0.42, colour);
    if let Some(text) = badge {
        let cx = s * 0.70;
        let cy = s * 0.30;
        fill_disc(&mut buf, size, cx, cy, s * 0.27, BADGE_BG);
        let scale = (size / 16).max(1);
        let glyph_w = 3 * scale;
        let glyph_h = 5 * scale;
        let gap = scale;
        let total_w = text.len() as u32 * glyph_w + (text.len().saturating_sub(1) as u32) * gap;
        let mut x = (cx - total_w as f32 / 2.0).round() as i32;
        let y = (cy - glyph_h as f32 / 2.0).round() as i32;
        for ch in text.chars() {
            draw_glyph(&mut buf, size, x, y, scale, ch);
            x += (glyph_w + gap) as i32;
        }
    }
    buf
}

/// Anti-aliased filled circle.
fn fill_disc(buf: &mut [u8], size: u32, cx: f32, cy: f32, r: f32, colour: Rgb) {
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (r - d + 0.5).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(buf, size, x, y, colour, coverage);
            }
        }
    }
}

fn blend(buf: &mut [u8], size: u32, x: u32, y: u32, colour: Rgb, alpha: f32) {
    let i = ((y * size + x) * 4) as usize;
    let a = alpha;
    let dst_a = buf[i + 3] as f32 / 255.0;
    let out_a = a + dst_a * (1.0 - a);
    for (k, c) in [colour.0, colour.1, colour.2].iter().enumerate() {
        let src = *c as f32;
        let dst = buf[i + k] as f32;
        let out = if out_a > 0.0 {
            (src * a + dst * dst_a * (1.0 - a)) / out_a
        } else {
            0.0
        };
        buf[i + k] = out.round() as u8;
    }
    buf[i + 3] = (out_a * 255.0).round() as u8;
}

/// 3×5 glyphs, one bit per pixel, rows top to bottom.
fn glyph(ch: char) -> [u8; 5] {
    match ch {
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        _ => [0; 5],
    }
}

fn draw_glyph(buf: &mut [u8], size: u32, x0: i32, y0: i32, scale: u32, ch: char) {
    let rows = glyph(ch);
    for (row, bits) in rows.iter().enumerate() {
        for col in 0..3 {
            if bits & (0b100 >> col) == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let x = x0 + (col * scale + dx) as i32;
                    let y = y0 + (row as u32 * scale + dy) as i32;
                    if x >= 0 && y >= 0 && (x as u32) < size && (y as u32) < size {
                        blend(buf, size, x as u32, y as u32, WHITE, 1.0);
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn count_white(buf: &[u8], size: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> usize {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = px(buf, size, x, y);
                if p[0] > 240 && p[1] > 240 && p[2] > 240 && p[3] > 200 {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn buffer_is_rgba_of_the_requested_size() {
        let buf = render(32, AMBER, None);
        assert_eq!(buf.len(), 32 * 32 * 4);
    }

    #[test]
    fn disc_is_coloured_in_the_middle_and_transparent_in_the_corners() {
        let buf = render(32, BLUE, None);
        assert_eq!(px(&buf, 32, 16, 16), [0x3b, 0x9d, 0xff, 255]);
        assert_eq!(px(&buf, 32, 0, 0)[3], 0);
        assert_eq!(px(&buf, 32, 31, 31)[3], 0);
    }

    #[test]
    fn badge_draws_white_digits_in_the_top_right_quadrant() {
        let plain = render(32, AMBER, None);
        let one = render(32, AMBER, Some("1"));
        assert_eq!(count_white(&plain, 32, 16, 0, 32, 16), 0);
        assert!(count_white(&one, 32, 16, 0, 32, 16) > 4);
        let nine_plus = render(32, AMBER, Some("9+"));
        assert!(count_white(&nine_plus, 32, 16, 0, 32, 16) > count_white(&one, 32, 16, 0, 32, 16));
    }

    #[test]
    fn badge_text_for_counts() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1).as_deref(), Some("1"));
        assert_eq!(badge_text(9).as_deref(), Some("9"));
        assert_eq!(badge_text(10).as_deref(), Some("9+"));
    }

    #[test]
    fn colour_follows_priority() {
        use herdr_status_core::Counts;
        let c = |blocked, working, done| Counts {
            blocked,
            working,
            done,
            idle: 0,
            unknown: 0,
        };
        assert_eq!(colour_for(&c(1, 5, 5)), AMBER);
        assert_eq!(colour_for(&c(0, 1, 5)), BLUE);
        assert_eq!(colour_for(&c(0, 0, 1)), GREEN);
        assert_eq!(colour_for(&c(0, 0, 0)), GREY);
    }
}
