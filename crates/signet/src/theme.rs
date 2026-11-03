//! Visual constants, derived from the "Paper direction" mockup
//! (758 × 1024 px, 212 dpi). Everything is in screen pixels.

pub const SCREEN_W: u32 = 758;
pub const SCREEN_H: u32 = 1024;

pub const BLACK: u8 = 0x00;
pub const INK: u8 = 0x20;
pub const GRAY_DARK: u8 = 0x60;
pub const GRAY: u8 = 0x90;
pub const GRAY_LIGHT: u8 = 0xC8;
pub const GRAY_FAINT: u8 = 0xE8;
pub const WHITE: u8 = 0xFF;

/// Side margin common to all screens.
pub const MARGIN: i32 = 40;
/// Height of the status bar (title + time + battery).
pub const STATUS_H: i32 = 64;

// Font sizes (px)
pub const PX_STATUS: f32 = 22.0;
pub const PX_SMALL: f32 = 22.0;
pub const PX_BODY: f32 = 26.0;
pub const PX_TITLE: f32 = 36.0;
pub const PX_SECTION: f32 = 30.0;
pub const PX_BUTTON: f32 = 24.0;

/// Reading fonts offered (the first is also the interface's).
pub const FAMILIES: [&str; 2] = ["Literata", "Source Sans"];

/// Text sizes offered in the quick settings (A, A, A).
pub const READER_SIZES: [f32; 3] = [26.0, 30.0, 36.0];
/// Side reading margins offered.
pub const READER_MARGINS: [i32; 3] = [40, 64, 92];
/// Line spacings offered.
pub const READER_LEADINGS: [f32; 3] = [1.3, 1.45, 1.65];
/// Ink: (anti-aliasing gamma, emboldening as a fraction of a pixel).
/// Thin = raw rendering; Crisp = darker edges, a quarter pixel more;
/// Bold = even fuller, for small sizes or tired eyes.
pub const INKS: [(f32, f32); 3] = [(1.0, 0.0), (1.4, 0.25), (1.8, 0.5)];
