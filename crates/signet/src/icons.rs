//! Interface icons: the Lucide set (ISC license, `assets/fonts/lucide`),
//! reduced to the icons used and rendered as a font, with the same
//! smoothed rasterizer as the text. Each icon is drawn in a square of
//! 24 units, stroke of 2: a single style, crisp at every size.

use signet_fb::Canvas;
use signet_text::{Font, Typesetter};

static LUCIDE: &[u8] = include_bytes!("../../../assets/fonts/lucide/lucide-signet.ttf");

/// Key of the icon font in the compositor's glyph cache.
const FONT_KEY: usize = 100;

thread_local! {
    static FONT: Font = {
        let f = Font::from_static(LUCIDE).expect("police d'icônes");
        // No auto-hinting: it would distort the outlines at small sizes.
        f.hinting.set(false);
        f
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    TextSize = 0xE587,
    Sun = 0xE178,
    Droplet = 0xE0B4,
    Contents = 0xE61E,
    Highlighter = 0xE0F4,
    Close = 0xE1B2,
    ChevronRight = 0xE06F,
    ChevronLeft = 0xE06E,
    Terminal = 0xE181,
    BookOpen = 0xE05F,
    Usb = 0xE356,
    Dictionary = 0xE544,
    Trash = 0xE18E,
    Library = 0xE100,
    Moon = 0xE11E,
    Power = 0xE140,
    Bookmark = 0xE060,
    Search = 0xE151,
    Hash = 0xE0EF,
    /// Erase (keyboard ⌫ key).
    Backspace = 0xE0AE,
    /// Typeface.
    Type = 0xE198,
    /// Item menu (⋮).
    More = 0xE0B7,
    BookCheck = 0xE546,
}

/// Draws `icon` in a square of `size` pixels centered on (`cx`, `cy`).
pub fn draw(
    c: &mut Canvas,
    ts: &mut Typesetter,
    icon: Icon,
    cx: i32,
    cy: i32,
    size: f32,
    gray: u8,
) {
    FONT.with(|font| {
        let Some(ch) = char::from_u32(icon as u32) else {
            return;
        };
        let Some(gid) = font.glyph_index(ch) else {
            return;
        };
        // The icon square runs from the baseline (0) to the em (1000 units).
        let pen_x = cx - (size / 2.0).round() as i32;
        let pen_y = cy + (size / 2.0).round() as i32;
        if let Some(bmp) = ts.cache.get(FONT_KEY, font, gid, size) {
            signet_text::blit_glyph(c, bmp, pen_x, pen_y, gray);
        }
    });
}

use crate::vector::{self, Path};

/// Width of the battery (terminal included) in the status bar.
pub const BATTERY_W: i32 = 34;

/// Modern battery, `BATTERY_W` × 16 px, top-left corner at (`x`, `y`):
/// rounded outline, terminal, proportional level; when charging, an inverted bolt
/// in the middle (white on the filled part, black on the empty part), legible at every level.
pub fn battery(c: &mut Canvas, x: i32, y: i32, level: Option<u8>, charging: bool, gray: u8) {
    let (x, y) = (x as f32, y as f32);
    let (w, h) = (30.0, 16.0);
    let t = 1.8;
    vector::fill(c, &Path::new().rounded_frame(x, y, w, h, 4.5, t), gray);
    // Terminal.
    vector::fill(
        c,
        &Path::new().rounded_rect(x + w + 1.0, y + h * 0.31, 2.6, h * 0.38, 1.3, false),
        gray,
    );
    // Level: a rounded fill inside the frame, with a margin.
    let pad = t + 1.7;
    let inner = w - 2.0 * pad;
    let frac = level.map(|l| l.min(100) as f32 / 100.0).unwrap_or(1.0);
    let fw = (inner * frac).max(if frac > 0.0 { 2.5 } else { 0.0 });
    if fw > 0.0 {
        vector::fill(
            c,
            &Path::new().rounded_rect(x + pad, y + pad, fw, h - 2.0 * pad, 1.8, false),
            gray,
        );
    }
    if charging {
        // Black bolt in the middle, ringed by a white edge that sets it apart from the fill
        // (as on iOS): legible at every charge level.
        let (bw, bh) = (9.5, 12.5);
        let (bx, by) = (x + (w - bw) / 2.0, y + (h - bh) / 2.0);
        let pts: Vec<(f32, f32)> = [
            (0.60, 0.0),
            (0.06, 0.56),
            (0.44, 0.56),
            (0.34, 1.0),
            (0.94, 0.42),
            (0.56, 0.42),
            (0.74, 0.0),
        ]
        .iter()
        .map(|(u, v)| (bx + u * bw, by + v * bh))
        .collect();
        let bolt = Path::new().polygon(&pts, false);
        vector::fill(c, &bolt, 255);
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            vector::fill(c, &Path::new().stroke(a.0, a.1, b.0, b.1, 2.6), 255);
        }
        vector::fill(c, &bolt, gray);
    }
}

/// "Margins" thumbnail: a page and its lines of text, indented according to
/// the level (0 narrow, 2 wide). 28 px square centered on (`cx`, `cy`).
pub fn margins(c: &mut Canvas, cx: i32, cy: i32, level: usize, gray: u8) {
    let (pw, ph) = (22.0, 28.0);
    let (x, y) = (cx as f32 - pw / 2.0, cy as f32 - ph / 2.0);
    let t = 1.8;
    let mut p = Path::new().rounded_frame(x, y, pw, ph, 3.5, t);
    let inset = [3.6, 5.2, 6.8][level.min(2)];
    for (i, len) in [1.0, 1.0, 1.0, 0.6].iter().enumerate() {
        let ly = y + 7.5 + i as f32 * 4.6;
        let x0 = x + inset + 0.9;
        let x1 = x0 + (pw - 2.0 * inset - 1.8) * len;
        p = p.stroke(x0, ly, x1, ly, 1.8);
    }
    vector::fill(c, &p, gray);
}

/// "Line spacing" thumbnail: three lines, more or less spaced (0 to 2).
pub fn leading(c: &mut Canvas, cx: i32, cy: i32, level: usize, gray: u8) {
    let gap = [4.6, 6.6, 8.6][level.min(2)];
    let (x0, x1) = (cx as f32 - 11.0, cx as f32 + 11.0);
    let mut p = Path::new();
    for i in -1..=1 {
        let y = cy as f32 + i as f32 * gap;
        p = p.stroke(x0, y, x1, y, 2.0);
    }
    vector::fill(c, &p, gray);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All icons exist in the reduced font, and each draws
    /// ink centered in its square.
    #[test]
    fn every_icon_draws_centered() {
        let mut ts = crate::typesetter();
        let all = [
            Icon::TextSize,
            Icon::Sun,
            Icon::Droplet,
            Icon::Contents,
            Icon::Highlighter,
            Icon::Close,
            Icon::ChevronRight,
            Icon::ChevronLeft,
            Icon::Terminal,
            Icon::BookOpen,
            Icon::Usb,
            Icon::Dictionary,
            Icon::Trash,
            Icon::Library,
            Icon::Moon,
            Icon::Power,
            Icon::Bookmark,
            Icon::Search,
            Icon::Hash,
            Icon::Backspace,
            Icon::Type,
            Icon::More,
            Icon::BookCheck,
        ];
        for icon in all {
            let mut c = Canvas::new(64, 64);
            c.clear(255);
            draw(&mut c, &mut ts, icon, 32, 32, 48.0, 0);
            let (mut n, mut sx, mut sy) = (0f32, 0f32, 0f32);
            for y in 0..64 {
                for x in 0..64 {
                    let a = (255 - c.data[y * 64 + x]) as f32;
                    n += a;
                    sx += a * x as f32;
                    sy += a * y as f32;
                }
            }
            assert!(n > 255.0 * 20.0, "{icon:?} : pas d'encre");
            // Center of the ink near the center of the square (icons are not symmetric).
            let (mx, my) = (sx / n, sy / n);
            assert!(
                (mx - 32.0).abs() < 9.0 && (my - 32.0).abs() < 9.0,
                "{icon:?} : centre ({mx:.1}, {my:.1})"
            );
        }
    }

    /// Sheet of all icons (to judge them by eye): SIGNET_UI_DUMP=directory.
    #[test]
    fn preview_sheet() {
        let Ok(dir) = std::env::var("SIGNET_UI_DUMP") else {
            return;
        };
        let mut ts = crate::typesetter();
        let mut c = Canvas::new(758, 420);
        c.clear(255);
        let all = [
            Icon::TextSize,
            Icon::Sun,
            Icon::Droplet,
            Icon::Contents,
            Icon::Highlighter,
            Icon::Close,
            Icon::ChevronRight,
            Icon::ChevronLeft,
            Icon::Terminal,
            Icon::BookOpen,
            Icon::Usb,
            Icon::Dictionary,
            Icon::Trash,
            Icon::Library,
            Icon::Moon,
            Icon::Power,
            Icon::Bookmark,
            Icon::Search,
            Icon::Hash,
            Icon::Backspace,
            Icon::Type,
            Icon::More,
            Icon::BookCheck,
        ];
        for (i, icon) in all.iter().enumerate() {
            draw(
                &mut c,
                &mut ts,
                *icon,
                40 + (i % 12) as i32 * 60,
                40 + (i / 12) as i32 * 60,
                28.0,
                0,
            );
        }
        for (i, level) in [0u8, 15, 50, 80, 100].iter().enumerate() {
            battery(&mut c, 40 + i as i32 * 70, 170, Some(*level), false, 0);
            battery(&mut c, 40 + i as i32 * 70, 210, Some(*level), true, 0);
        }
        for l in 0..3 {
            margins(&mut c, 60 + l as i32 * 70, 300, l, 0);
            leading(&mut c, 300 + l as i32 * 70, 300, l, 0);
        }
        draw(&mut c, &mut ts, Icon::Usb, 640, 300, 140.0, 0);
        c.save_pgm(&format!("{dir}/icones.pgm")).unwrap();
    }
}
