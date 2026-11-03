//! `signet-text` — typography.
//!
//! Full pipeline, in pure Rust:
//!
//! 1. **Font** (`Font`): a TTF loaded in memory, read both by
//!    `rustybuzz` (shaping) and `ab_glyph` (outlines → pixels).
//! 2. **Shaping** (`Font::shape`): turns a Unicode string into a sequence
//!    of positioned glyphs, applying the font's kerning and ligatures
//!    (« fi », « ffl »…). This is what separates "printed" text
//!    from "typed" text.
//! 3. **Rasterization** (`GlyphCache`): each glyph is converted only once
//!    into a small 8-bit coverage image, then cached. On a
//!    1 GHz Cortex-A9, the cache is what makes a page display
//!    almost instantly.
//! 4. **Layout** (`Typesetter`): breaks a paragraph into justified lines
//!    for a given width, then draws it on a `Canvas`.

pub mod hyphen;
pub use hyphen::Hyphenator;

use ab_glyph::{Font as _, FontRef, ScaleFont as _};
use signet_fb::Canvas;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Regular,
    Italic,
    Bold,
    BoldItalic,
}

/// A TrueType/OpenType font loaded in memory for the whole duration of the
/// program (the data is deliberately "leaked": an e-reader
/// loads its fonts once at startup and never unloads them).
pub struct Font {
    face: rustybuzz::Face<'static>,
    ab: FontRef<'static>,
    /// Outlines and hinting (TrueType interpreter / skrifa auto-hinter).
    sk: skrifa::FontRef<'static>,
    /// One hinting instance per size (key: px × 64), created on demand.
    hinters: std::cell::RefCell<HashMap<u32, Option<skrifa::outline::HintingInstance>>>,
    /// Hinting enabled (can be turned off for comparison).
    pub hinting: std::cell::Cell<bool>,
    units_per_em: f32,
    /// Conversion factor "size in px (em)" → ab_glyph's `PxScale`.
    /// ab_glyph defines its scale by the ascent − descent height, not by
    /// the em: without this conversion, glyphs come out too small
    /// compared to the advances computed by rustybuzz.
    ab_scale_factor: f32,
}

/// A shaped glyph, positioned relative to the start of the string, in px.
#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub id: u16,
    /// Pen point position (glyph origin) relative to the start of the run.
    pub x: f32,
    pub y: f32,
    /// Index of the first byte of the source character in the string.
    pub cluster: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ShapedRun {
    pub glyphs: Vec<ShapedGlyph>,
    /// Total width (advance) in px.
    pub advance: f32,
}

impl Font {
    pub fn from_bytes(bytes: Vec<u8>) -> Option<Font> {
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        Self::from_static(data)
    }

    pub fn from_static(data: &'static [u8]) -> Option<Font> {
        let face = rustybuzz::Face::from_slice(data, 0)?;
        let ab = FontRef::try_from_slice(data).ok()?;
        let sk = skrifa::FontRef::new(data).ok()?;
        let units_per_em = face.units_per_em() as f32;
        let ab_scale_factor = ab.height_unscaled() / units_per_em;
        Some(Font {
            face,
            ab,
            sk,
            hinters: Default::default(),
            hinting: std::cell::Cell::new(true),
            units_per_em,
            ab_scale_factor,
        })
    }

    /// Glyph for a character (icon font: one private-use-area character
    /// per icon).
    pub fn glyph_index(&self, c: char) -> Option<u16> {
        use skrifa::MetadataProvider;
        self.sk.charmap().map(c).map(|g| g.to_u32() as u16)
    }

    /// ab_glyph scale corresponding to an em of `px` pixels.
    fn ab_scale(&self, px: f32) -> ab_glyph::PxScale {
        ab_glyph::PxScale::from(px * self.ab_scale_factor)
    }

    /// Height above the baseline for a size of `px`.
    pub fn ascent(&self, px: f32) -> f32 {
        self.ab.as_scaled(self.ab_scale(px)).ascent()
    }
    /// Depth below the baseline (positive value).
    pub fn descent(&self, px: f32) -> f32 {
        -self.ab.as_scaled(self.ab_scale(px)).descent()
    }
    pub fn line_gap(&self, px: f32) -> f32 {
        self.ab.as_scaled(self.ab_scale(px)).line_gap()
    }

    /// Shapes `text` at size `px` (kerning + ligatures enabled by default).
    pub fn shape(&self, text: &str, px: f32) -> ShapedRun {
        self.shape_with(text, px, &[])
    }

    /// Shaping with OpenType features (« sups », « c2sc »…).
    pub fn shape_with(&self, text: &str, px: f32, features: &[rustybuzz::Feature]) -> ShapedRun {
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        let out = rustybuzz::shape(&self.face, features, buf);
        let scale = px / self.units_per_em;
        let infos = out.glyph_infos();
        let positions = out.glyph_positions();
        let mut glyphs = Vec::with_capacity(infos.len());
        let (mut pen_x, mut pen_y) = (0.0f32, 0.0f32);
        for (info, pos) in infos.iter().zip(positions) {
            glyphs.push(ShapedGlyph {
                id: info.glyph_id as u16,
                x: pen_x + pos.x_offset as f32 * scale,
                y: pen_y - pos.y_offset as f32 * scale,
                cluster: info.cluster,
            });
            pen_x += pos.x_advance as f32 * scale;
            pen_y -= pos.y_advance as f32 * scale;
        }
        ShapedRun {
            glyphs,
            advance: pen_x,
        }
    }

    /// Width of a string, without drawing it.
    pub fn measure(&self, text: &str, px: f32) -> f32 {
        self.shape(text, px).advance
    }
}

/// Coverage image of a rasterized glyph (0 = transparent, 255 = solid).
#[derive(Clone, Debug)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Offset of the image's top-left corner relative to the pen point.
    pub left: i32,
    pub top: i32,
    pub data: Vec<u8>,
}

/// Cache of rasterized glyphs, keyed by (font, glyph, size in 1/64 px).
#[derive(Default)]
pub struct GlyphCache {
    map: HashMap<(usize, u16, u32), Option<GlyphBitmap>>,
    pub hits: u64,
    pub misses: u64,
}

impl GlyphCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&mut self, font_key: usize, font: &Font, id: u16, px: f32) -> Option<&GlyphBitmap> {
        let key = (font_key, id, (px * 64.0) as u32);
        if !self.map.contains_key(&key) {
            self.misses += 1;
            let bmp = rasterize(font, id, px);
            self.map.insert(key, bmp);
        } else {
            self.hits += 1;
        }
        self.map.get(&key).and_then(|b| b.as_ref())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Pen that accumulates the outline in pixels (y downward) and its bounding box.
#[derive(Default)]
struct PathPen {
    cmds: Vec<PathCmd>,
    min: (f32, f32),
    max: (f32, f32),
    start: (f32, f32),
    last: (f32, f32),
    any: bool,
}

enum PathCmd {
    Line((f32, f32), (f32, f32)),
    Quad((f32, f32), (f32, f32), (f32, f32)),
    Cubic((f32, f32), (f32, f32), (f32, f32), (f32, f32)),
}

impl PathPen {
    fn see(&mut self, p: (f32, f32)) {
        if !self.any {
            self.min = p;
            self.max = p;
            self.any = true;
        } else {
            self.min = (self.min.0.min(p.0), self.min.1.min(p.1));
            self.max = (self.max.0.max(p.0), self.max.1.max(p.1));
        }
    }
}

impl skrifa::outline::OutlinePen for PathPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.start = (x, -y);
        self.last = self.start;
        self.see(self.start);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = (x, -y);
        self.cmds.push(PathCmd::Line(self.last, p));
        self.see(p);
        self.last = p;
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let c = (cx0, -cy0);
        let p = (x, -y);
        self.cmds.push(PathCmd::Quad(self.last, c, p));
        self.see(c);
        self.see(p);
        self.last = p;
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let c0 = (cx0, -cy0);
        let c1 = (cx1, -cy1);
        let p = (x, -y);
        self.cmds.push(PathCmd::Cubic(self.last, c0, c1, p));
        self.see(c0);
        self.see(c1);
        self.see(p);
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.cmds.push(PathCmd::Line(self.last, self.start));
            self.last = self.start;
        }
    }
}

impl Font {
    /// Hinting instance for this size: FreeType's "light" hinting
    /// (vertical alignment only, advances unchanged), TrueType interpreter
    /// if the font has instructions (Literata does), auto-hinter otherwise.
    fn hinter(&self, px: f32) -> std::cell::RefMut<'_, Option<skrifa::outline::HintingInstance>> {
        use skrifa::outline::{Engine, HintingInstance, HintingOptions, SmoothMode, Target};
        use skrifa::MetadataProvider;
        let key = (px * 64.0) as u32;
        let mut map = self.hinters.borrow_mut();
        map.entry(key).or_insert_with(|| {
            let opts = HintingOptions {
                engine: Engine::AutoFallback,
                target: Target::Smooth {
                    mode: SmoothMode::Light,
                    symmetric_rendering: false,
                    preserve_linear_metrics: true,
                },
            };
            let inst = HintingInstance::new(
                &self.sk.outline_glyphs(),
                skrifa::instance::Size::new(px),
                skrifa::instance::LocationRef::default(),
                opts,
            )
            .ok();
            inst
        });
        std::cell::RefMut::map(map, |m| m.get_mut(&key).unwrap())
    }
}

fn rasterize(font: &Font, id: u16, px: f32) -> Option<GlyphBitmap> {
    use skrifa::outline::DrawSettings;
    use skrifa::MetadataProvider;
    let outlines = font.sk.outline_glyphs();
    let glyph = outlines.get(skrifa::GlyphId::new(id as u32))?;
    let mut pen = PathPen::default();
    let mut drawn = false;
    if font.hinting.get() {
        if let Some(inst) = font.hinter(px).as_ref() {
            drawn = glyph
                .draw(DrawSettings::hinted(inst, false), &mut pen)
                .is_ok();
        }
    }
    if !drawn {
        pen = PathPen::default();
        glyph
            .draw(
                DrawSettings::unhinted(
                    skrifa::instance::Size::new(px),
                    skrifa::instance::LocationRef::default(),
                ),
                &mut pen,
            )
            .ok()?;
    }
    use skrifa::outline::OutlinePen;
    pen.close();
    if !pen.any || pen.cmds.is_empty() {
        return None;
    }
    let left = pen.min.0.floor();
    let top = pen.min.1.floor();
    let width = (pen.max.0.ceil() - left).max(0.0) as u32;
    let height = (pen.max.1.ceil() - top).max(0.0) as u32;
    if width == 0 || height == 0 {
        return None;
    }
    let pt = |p: (f32, f32)| ab_glyph_rasterizer::point(p.0 - left, p.1 - top);
    let mut r = ab_glyph_rasterizer::Rasterizer::new(width as usize, height as usize);
    for c in &pen.cmds {
        match *c {
            PathCmd::Line(a, b) => r.draw_line(pt(a), pt(b)),
            PathCmd::Quad(a, c, b) => r.draw_quad(pt(a), pt(c), pt(b)),
            PathCmd::Cubic(a, c0, c1, b) => r.draw_cubic(pt(a), pt(c0), pt(c1), pt(b)),
        }
    }
    let mut data = vec![0u8; (width * height) as usize];
    r.for_each_pixel_2d(|x, y, c| {
        if x < width && y < height {
            data[(y * width + x) as usize] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    });
    Some(GlyphBitmap {
        width,
        height,
        left: left as i32,
        top: top as i32,
        data,
    })
}

/// Draws a glyph (coverage) on the canvas, in gray `gray` (0 = black).
pub fn blit_glyph(canvas: &mut Canvas, bmp: &GlyphBitmap, x: i32, y: i32, gray: u8) {
    blit_glyph_mode(canvas, bmp, x, y, gray, false)
}

/// Variant: `binary = true` thresholds the coverage (no anti-aliasing).
/// Purely black-and-white content allows the fast e-ink waveforms
/// (DU ~375 ms, A2 ~220 ms instead of ~620 ms in 16 grays).
pub fn blit_glyph_mode(
    canvas: &mut Canvas,
    bmp: &GlyphBitmap,
    x: i32,
    y: i32,
    gray: u8,
    binary: bool,
) {
    blit_glyph_ink(canvas, bmp, x, y, gray, binary, &Ink::default())
}

/// Ink rendering: contrast curve on the coverage (gamma < 1 =
/// darker edges, crisper strokes on e-ink) and horizontal
/// emboldening by a fraction of a pixel.
#[derive(Clone)]
pub struct Ink {
    /// Coverage → coverage table, 256 entries.
    pub lut: [u8; 256],
    /// Share of the left pixel's coverage that is added (0 = nothing, 255 = a whole pixel).
    pub embolden: u8,
}

impl Default for Ink {
    fn default() -> Self {
        Ink::new(1.0, 0.0)
    }
}

impl Ink {
    /// `gamma`: 1.0 = linear, 1.5 = clearly darker edges;
    /// `embolden`: fraction of a pixel added to the right of each stroke (0–1).
    pub fn new(gamma: f32, embolden: f32) -> Self {
        let mut lut = [0u8; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = ((i as f32 / 255.0).powf(1.0 / gamma) * 255.0 + 0.5) as u8;
        }
        Ink {
            lut,
            embolden: (embolden.clamp(0.0, 1.0) * 255.0) as u8,
        }
    }
}

pub fn blit_glyph_ink(
    canvas: &mut Canvas,
    bmp: &GlyphBitmap,
    x: i32,
    y: i32,
    gray: u8,
    binary: bool,
    ink: &Ink,
) {
    let emb = ink.embolden as u32;
    for gy in 0..bmp.height {
        let cy = y + bmp.top + gy as i32;
        if cy < 0 || cy >= canvas.height as i32 {
            continue;
        }
        let row = (gy * bmp.width) as usize;
        // One more pixel on the right when emboldening.
        let gw = if emb > 0 { bmp.width + 1 } else { bmp.width };
        for gx in 0..gw {
            let cx = x + bmp.left + gx as i32;
            if cx < 0 || cx >= canvas.width as i32 {
                continue;
            }
            let own = if gx < bmp.width {
                bmp.data[row + gx as usize] as u32
            } else {
                0
            };
            let mut cov = if emb > 0 && gx > 0 {
                let left = bmp.data[row + gx as usize - 1] as u32;
                (own + left * emb / 255).min(255)
            } else {
                own
            };
            if cov == 0 {
                continue;
            }
            cov = ink.lut[cov as usize] as u32;
            if binary {
                // Slightly low threshold (40%): preserves the thickness of thin strokes.
                if cov < 100 {
                    continue;
                }
                cov = 255;
            }
            let idx = (cy as u32 * canvas.width + cx as u32) as usize;
            let bg = canvas.data[idx] as u32;
            // blend: bg * (1 - cov) + gray * cov
            canvas.data[idx] = ((bg * (255 - cov) + gray as u32 * cov) / 255) as u8;
        }
    }
}

/// Font family (4 styles) + cache, ready to typeset paragraphs.
pub struct Typesetter {
    /// Font families (4 styles each); the first one is used for the interface.
    fonts: Vec<[Font; 4]>,
    /// Current family: the one for books while typesetting and drawing
    /// pages, the first one elsewhere. It goes into every cache key.
    family: std::cell::Cell<u8>,
    pub cache: GlyphCache,
    pub hyphenator: Option<Hyphenator>,
    /// Shaping cache: (style, size ×64, word) → run. The words of a
    /// book repeat enormously; shaping each occurrence costs
    /// ~50 µs on a Cortex-A7, the cache reduces that to a lookup.
    shaped: std::cell::RefCell<HashMap<(u8, u32, String), std::rc::Rc<ShapedRun>>>,
    pub shape_hits: std::cell::Cell<u64>,
    pub shape_misses: std::cell::Cell<u64>,
    /// Hyphenation points per word (Liang), see `hyphen_points`.
    hyph_cache: std::cell::RefCell<HashMap<String, std::rc::Rc<Vec<usize>>>>,
    /// Advance of a single character (space, hyphen) by (style, size ×64):
    /// measured for every paragraph, it used to cost a full shaping (including creating
    /// the rustybuzz plan), i.e. 15 to 20% of typesetting time.
    char_advance: std::cell::RefCell<HashMap<(u8, u32, char), f32>>,
    /// Optimal paragraph breaking (Knuth-Plass); greedy otherwise.
    pub knuth_plass: std::cell::Cell<bool>,
    /// Pass counters: [strict tolerance, unlimited, greedy fallback].
    pub kp_passes: std::cell::Cell<[u32; 3]>,
    /// Knuth-Plass settings: tolerance of the first pass, hyphenation penalty.
    pub kp_tolerance: std::cell::Cell<f32>,
    pub kp_hyphen_penalty: std::cell::Cell<f32>,
    /// Penalty for a break that leaves fewer than three letters at the end of a line
    /// (« is-sues », « dé-vo- »): Apple Books forbids them (`hyphenate-limit-before: 3`).
    pub kp_short_head_penalty: std::cell::Cell<f32>,
    /// Maximum consecutive hyphenated lines (Apple Books: `hyphenate-limit-lines: 2`).
    pub kp_max_hyphen_run: std::cell::Cell<u8>,
    /// Demerits added when two neighboring lines differ by more than one spacing
    /// class (TeX: \adjdemerits); 0 to disable.
    pub kp_adj_demerits: std::cell::Cell<f32>,
    /// Maximum compression of a space, as a fraction of its width (TeX: ⅓,
    /// InDesign: 20%, i.e. at most 80% of the space at its tightest).
    pub kp_shrink: std::cell::Cell<f32>,
    /// Natural space between words, as a multiple of the font's space
    /// (Literata has a narrow space, 0.2 em, designed for ragged-right text).
    pub space_scale: std::cell::Cell<f32>,
    /// Cumulated times (profile): uncached shaping, uncached hyphenation, Knuth-Plass.
    pub t_shape: std::cell::Cell<Duration>,
    pub t_hyph: std::cell::Cell<Duration>,
    pub t_kp: std::cell::Cell<Duration>,
    pub hyph_misses: std::cell::Cell<u64>,
    /// Text rendering without anti-aliasing (fast e-ink mode).
    binary: bool,
    /// Ink contrast and weight.
    ink: Ink,
}

/// Position of a segment relative to the baseline: superscript (« XXIᵉ »,
/// footnote call) or subscript, set smaller and shifted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Script {
    #[default]
    Normal,
    Sup,
    Sub,
}

impl Script {
    /// (relative size, baseline offset as a fraction of the text size;
    /// negative = upward).
    fn metrics(self) -> (f32, f32) {
        match self {
            Script::Normal => (1.0, 0.0),
            Script::Sup => (0.65, -0.36),
            Script::Sub => (0.65, 0.14),
        }
    }
}

/// Small caps: `SmallCaps` changes lowercase letters (CSS `font-variant:
/// small-caps`, "smcp" feature), `AllSmallCaps` also the capitals
/// (« c2sc » + « smcp »: a century in Roman numerals, an acronym).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub enum Caps {
    #[default]
    Normal,
    SmallCaps,
    AllSmallCaps,
}

impl Caps {
    fn features(self) -> Vec<rustybuzz::Feature> {
        use std::str::FromStr;
        let f = |t: &str| rustybuzz::Feature::from_str(t).expect("fonctionnalité OpenType");
        match self {
            Caps::Normal => Vec::new(),
            Caps::SmallCaps => vec![f("smcp")],
            Caps::AllSmallCaps => vec![f("c2sc"), f("smcp")],
        }
    }
}

/// Formatting of a text segment: style (font), position (superscript),
/// relative size (reduced inline text, `<small>`), small caps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fmt {
    pub style: Style,
    pub script: Script,
    pub scale: f32,
    pub caps: Caps,
}

impl Fmt {
    pub fn plain(style: Style) -> Fmt {
        Fmt {
            style,
            script: Script::Normal,
            scale: 1.0,
            caps: Caps::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Left,
    Justify,
    Center,
    Right,
}

/// Paragraph typesetting parameters.
#[derive(Clone, Copy, Debug)]
pub struct ParagraphStyle {
    pub style: Style,
    pub px: f32,
    /// Line spacing, multiplier of `px` (1.0 = tight, 1.4 = comfortable).
    pub line_height: f32,
    pub align: Align,
    /// First-line indent, in px.
    pub indent: f32,
}

/// A shaped word, ready to be placed.
#[derive(Clone)]
struct Word {
    /// Word head that was split: the run ends with a hyphen.
    hyphen: bool,
    /// The following space is elastic and allows a break. Otherwise (high punctuation
    /// « ? ! : ; » », opening quotation mark « « (… »), the space is fixed:
    /// UAX 14 rules LB13/LB19, which is what a browser does.
    glue: bool,
    run: std::rc::Rc<ShapedRun>,
    /// Width of the following space (0 for the last word).
    space: f32,
    style: Style,
    /// Source text (for hyphenation).
    text: String,
    /// Index (in characters) of the word's first character in the paragraph.
    ch0: u32,
    /// Typesetting size (the paragraph's, smaller for superscripts).
    px: f32,
    /// Baseline offset (px; negative = superscript).
    rise: f32,
    caps: Caps,
}

/// Character index of a glyph: `ch0` + number of characters before its
/// cluster (byte offset in the word).
fn glyph_ch(text: &str, ch0: u32, cluster: u32) -> u32 {
    ch0 + text
        .get(..cluster as usize)
        .map(|t| t.chars().count())
        .unwrap_or(0) as u32
}

/// Width of each glyph in a run (distance to the next one, or the rest of the advance).
fn glyph_widths(run: &ShapedRun) -> Vec<f32> {
    let n = run.glyphs.len();
    (0..n)
        .map(|i| {
            if i + 1 < n {
                run.glyphs[i + 1].x - run.glyphs[i].x
            } else {
                run.advance - run.glyphs[i].x
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct PositionedGlyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub style: Style,
    /// Index (in characters) of the source character in the paragraph text
    /// (concatenation of the spans). Serves as an anchor for annotations.
    pub ch: u32,
    /// Width occupied by this glyph (up to the next glyph or the end of the word).
    pub w: f32,
    /// Relative size of the glyph (1; 0.65 for superscript or subscript).
    pub scale: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub glyphs: Vec<PositionedGlyph>,
    pub width: f32,
    /// Space adjustment ratio (Knuth): 0 = natural, +1 = each
    /// space stretched by half a space, −1 = compressed by a third. `NaN` for
    /// a non-justified line (last line, ragged, centered).
    pub ratio: f32,
    /// The line ends with a hyphenation break.
    pub hyphenated: bool,
}

impl Typesetter {
    pub fn new(regular: Font, italic: Font, bold: Font, bold_italic: Font) -> Self {
        Typesetter {
            fonts: vec![[regular, italic, bold, bold_italic]],
            family: std::cell::Cell::new(0),
            cache: GlyphCache::new(),
            hyphenator: None,
            shaped: Default::default(),
            shape_hits: Default::default(),
            shape_misses: Default::default(),
            hyph_cache: Default::default(),
            char_advance: Default::default(),
            knuth_plass: std::cell::Cell::new(true),
            kp_passes: Default::default(),
            kp_tolerance: std::cell::Cell::new(f32::INFINITY),
            kp_hyphen_penalty: std::cell::Cell::new(50.0),
            kp_short_head_penalty: std::cell::Cell::new(2000.0),
            kp_max_hyphen_run: std::cell::Cell::new(2),
            kp_adj_demerits: std::cell::Cell::new(10_000.0),
            kp_shrink: std::cell::Cell::new(0.2),
            space_scale: std::cell::Cell::new(1.0),
            t_shape: Default::default(),
            t_hyph: Default::default(),
            t_kp: Default::default(),
            hyph_misses: Default::default(),
            binary: false,
            ink: Ink::default(),
        }
    }

    /// Serializes the shaping cache (in-house binary format, dependency-free).
    /// Entry: style u8, size×64 u32, word (u16 + bytes), advance f32,
    /// glyphs (u16 + n × (id u16, x f32, y f32, cluster u32)).
    pub fn export_shaped(&self) -> Vec<u8> {
        let map = self.shaped.borrow();
        let mut out = Vec::with_capacity(map.len() * 48 + 8);
        out.extend_from_slice(b"SGSH");
        out.extend_from_slice(&(map.len() as u32).to_le_bytes());
        for ((style, px64, word), run) in map.iter() {
            out.push(*style);
            out.extend_from_slice(&px64.to_le_bytes());
            let w = word.as_bytes();
            out.extend_from_slice(&(w.len() as u16).to_le_bytes());
            out.extend_from_slice(w);
            out.extend_from_slice(&run.advance.to_le_bytes());
            out.extend_from_slice(&(run.glyphs.len() as u16).to_le_bytes());
            for g in &run.glyphs {
                out.extend_from_slice(&g.id.to_le_bytes());
                out.extend_from_slice(&g.x.to_le_bytes());
                out.extend_from_slice(&g.y.to_le_bytes());
                out.extend_from_slice(&g.cluster.to_le_bytes());
            }
        }
        out
    }

    /// Reloads a cache produced by [`export_shaped`](Self::export_shaped).
    /// Returns the number of entries loaded (0 if the format is unknown).
    pub fn import_shaped(&self, data: &[u8]) -> usize {
        fn rd<const N: usize>(d: &[u8], p: &mut usize) -> Option<[u8; N]> {
            let s = d.get(*p..*p + N)?;
            *p += N;
            s.try_into().ok()
        }
        if data.len() < 8 || &data[..4] != b"SGSH" {
            return 0;
        }
        let mut p = 4;
        let Some(n) = rd::<4>(data, &mut p).map(u32::from_le_bytes) else {
            return 0;
        };
        let mut map = self.shaped.borrow_mut();
        let mut loaded = 0;
        for _ in 0..n {
            let Some(style) = rd::<1>(data, &mut p).map(|b| b[0]) else {
                break;
            };
            let Some(px64) = rd::<4>(data, &mut p).map(u32::from_le_bytes) else {
                break;
            };
            let Some(wl) = rd::<2>(data, &mut p).map(u16::from_le_bytes) else {
                break;
            };
            let Some(wb) = data.get(p..p + wl as usize) else {
                break;
            };
            p += wl as usize;
            let Ok(word) = std::str::from_utf8(wb) else {
                break;
            };
            let Some(advance) = rd::<4>(data, &mut p).map(f32::from_le_bytes) else {
                break;
            };
            let Some(gn) = rd::<2>(data, &mut p).map(u16::from_le_bytes) else {
                break;
            };
            let mut glyphs = Vec::with_capacity(gn as usize);
            let mut ok = true;
            for _ in 0..gn {
                let (Some(id), Some(x), Some(y), Some(cluster)) = (
                    rd::<2>(data, &mut p).map(u16::from_le_bytes),
                    rd::<4>(data, &mut p).map(f32::from_le_bytes),
                    rd::<4>(data, &mut p).map(f32::from_le_bytes),
                    rd::<4>(data, &mut p).map(u32::from_le_bytes),
                ) else {
                    ok = false;
                    break;
                };
                glyphs.push(ShapedGlyph { id, x, y, cluster });
            }
            if !ok {
                break;
            }
            map.insert(
                (style, px64, word.to_string()),
                std::rc::Rc::new(ShapedRun { glyphs, advance }),
            );
            loaded += 1;
        }
        loaded
    }

    pub fn shaped_len(&self) -> usize {
        self.shaped.borrow().len()
    }

    /// Adds a font family; returns its index.
    pub fn add_family(&mut self, regular: Font, italic: Font, bold: Font, bold_italic: Font) -> u8 {
        self.fonts.push([regular, italic, bold, bold_italic]);
        (self.fonts.len() - 1) as u8
    }

    /// Chooses the family for the typesetting and drawing that follow (0:
    /// the interface's). Returns the previous one, to be restored afterwards.
    pub fn set_family(&self, family: u8) -> u8 {
        let f = family.min(self.fonts.len() as u8 - 1).min(3);
        self.family.replace(f)
    }

    pub fn family(&self) -> u8 {
        self.family.get()
    }

    pub fn families(&self) -> usize {
        self.fonts.len()
    }

    /// Style byte of the cache keys: style (bits 0–1), small caps
    /// (4–5), family (6–7).
    fn style_key(&self, style: Style, caps: Caps) -> u8 {
        style as u8 | (caps as u8) << 4 | self.family.get() << 6
    }

    /// Key of the current font in the glyph cache.
    fn glyph_key(&self, style: Style) -> usize {
        self.family.get() as usize * 4 + style as usize
    }

    /// Advance of a single character, cached.
    pub fn advance_of(&self, style: Style, px: f32, c: char) -> f32 {
        let key = (self.style_key(style, Caps::Normal), (px * 64.0) as u32, c);
        if let Some(&w) = self.char_advance.borrow().get(&key) {
            return w;
        }
        let mut buf = [0u8; 4];
        let w = self.font(style).measure(c.encode_utf8(&mut buf), px);
        self.char_advance.borrow_mut().insert(key, w);
        w
    }

    /// Shapes a word, going through the cache.
    pub fn shape_cached(&self, word: &str, style: Style, px: f32) -> std::rc::Rc<ShapedRun> {
        self.shape_fmt(word, style, px, Caps::Normal)
    }

    /// Like [`shape_cached`](Self::shape_cached), in small caps when needed.
    pub fn shape_fmt(
        &self,
        word: &str,
        style: Style,
        px: f32,
        caps: Caps,
    ) -> std::rc::Rc<ShapedRun> {
        // Small caps and family in the high bits of the style byte
        // (the persistent cache keeps the same format).
        let key = (
            self.style_key(style, caps),
            (px * 64.0) as u32,
            word.to_string(),
        );
        if let Some(r) = self.shaped.borrow().get(&key) {
            self.shape_hits.set(self.shape_hits.get() + 1);
            return r.clone();
        }
        self.shape_misses.set(self.shape_misses.get() + 1);
        let t0 = Instant::now();
        let run = std::rc::Rc::new(if caps == Caps::Normal {
            self.font(style).shape(word, px)
        } else {
            self.font(style).shape_with(word, px, &caps.features())
        });
        self.t_shape.set(self.t_shape.get() + t0.elapsed());
        let mut map = self.shaped.borrow_mut();
        if map.len() > 40_000 {
            map.clear();
        }
        map.insert(key, run.clone());
        run
    }

    pub fn with_hyphenation(mut self, h: Hyphenator) -> Self {
        self.hyphenator = Some(h);
        self
    }

    /// Tries to split `word` so that "prefix-" fits in `avail` px.
    /// Returns (prefix with shaped hyphen, shaped suffix, suffix text).
    fn try_hyphenate(
        &self,
        word: &str,
        style: Style,
        px: f32,
        caps: Caps,
        avail: f32,
    ) -> Option<(std::rc::Rc<ShapedRun>, std::rc::Rc<ShapedRun>, String)> {
        let points = self.hyphen_points(word);
        if points.is_empty() {
            return None;
        }
        let chars: Vec<char> = word.chars().collect();
        // From the farthest point to the nearest: the longest head that fits.
        // Candidates go through the cache: a word split once is
        // often split again (same column width, same words).
        for &p in points.iter().rev() {
            // After an existing hyphen, nothing to add; a soft hyphen is
            // removed and replaced with a real hyphen.
            let add = chars[p - 1] != '-';
            let head: String = chars[..p]
                .iter()
                .copied()
                .filter(|c| *c != '\u{ad}')
                .chain(add.then_some('-'))
                .collect();
            let run = self.shape_fmt(&head, style, px, caps);
            if run.advance <= avail {
                let tail: String = chars[p..].iter().collect();
                let tail_run = self.shape_fmt(&tail, style, px, caps);
                return Some((run, tail_run, tail));
            }
        }
        None
    }

    pub fn font(&self, style: Style) -> &Font {
        &self.fonts[self.family.get() as usize][style as usize]
    }

    /// Breaks `text` into lines of width ≤ `width`. Returns the lines
    /// (glyphs positioned relative to the paragraph's top-left corner,
    /// y = baseline) and the total height.
    pub fn layout(&self, text: &str, width: f32, st: ParagraphStyle) -> (Vec<Line>, f32) {
        self.layout_rich(&[(text, st.style)], width, st)
    }

    /// Like [`layout`](Self::layout), but each segment carries its style
    /// (italic in the middle of a paragraph, etc.). `st.style` serves as the base
    /// for the line metrics.
    pub fn layout_rich(
        &self,
        spans: &[(&str, Style)],
        width: f32,
        st: ParagraphStyle,
    ) -> (Vec<Line>, f32) {
        let scripted: Vec<(&str, Style, Script)> = spans
            .iter()
            .map(|(t, s)| (*t, *s, Script::Normal))
            .collect();
        self.layout_scripted(&scripted, width, st)
    }

    /// Like [`layout_rich`](Self::layout_rich), each segment being able to be
    /// superscript or subscript (« XXI » + « e » as superscript).
    pub fn layout_scripted(
        &self,
        spans: &[(&str, Style, Script)],
        width: f32,
        st: ParagraphStyle,
    ) -> (Vec<Line>, f32) {
        let f: Vec<(&str, Fmt)> = spans
            .iter()
            .map(|(t, s, sc)| {
                (
                    *t,
                    Fmt {
                        script: *sc,
                        ..Fmt::plain(*s)
                    },
                )
            })
            .collect();
        self.layout_formatted(&f, width, st)
    }

    /// Like [`layout_rich`](Self::layout_rich), with full formatting
    /// per segment (superscript, relative size, small caps).
    pub fn layout_formatted(
        &self,
        spans: &[(&str, Fmt)],
        width: f32,
        st: ParagraphStyle,
    ) -> (Vec<Line>, f32) {
        let font = self.font(st.style);
        let space_w = self.advance_of(st.style, st.px, ' ') * self.space_scale.get();
        // Whitespace that allows a break: everything except non-breaking ones (no-break
        // space, narrow no-break U+202F, figure space, word joiner).
        let is_break_space = |c: char| {
            c.is_whitespace() && !matches!(c, '\u{a0}' | '\u{202f}' | '\u{2007}' | '\u{2060}')
        };
        // Split into words while respecting span boundaries: a word that
        // straddles two styles is cut into two glued "words" (space = 0).
        let mut words: Vec<Word> = Vec::new();
        let mut ch_base: u32 = 0; // characters already seen in the previous spans
        for (i, (text, fmt)) in spans.iter().enumerate() {
            let starts_with_space = text.chars().next().map(is_break_space).unwrap_or(false);
            let ends_with_space = text.chars().last().map(is_break_space).unwrap_or(false);
            if starts_with_space {
                if let Some(last) = words.last_mut() {
                    last.space = space_w;
                }
            }
            // Split into words with their byte offset, to recover the character index.
            let mut byte = 0usize;
            let mut pending: Vec<(usize, &str)> = Vec::new();
            for w in text.split(is_break_space) {
                // word position: search starting from `byte`
                if let Some(pos) = text[byte..].find(w) {
                    let start = byte + pos;
                    if !w.is_empty() {
                        pending.push((start, w));
                    }
                    byte = start + w.len();
                }
            }
            let n = pending.len();
            let (mut seen_byte, mut seen_chars) = (0usize, 0u32);
            for (k, (start, w)) in pending.into_iter().enumerate() {
                let is_last = k + 1 == n;
                let space = if is_last && !ends_with_space && i + 1 < spans.len() {
                    0.0
                } else {
                    space_w
                };
                seen_chars += text[seen_byte..start].chars().count() as u32;
                seen_byte = start;
                let ch0 = ch_base + seen_chars;
                let (k_size, k_rise) = fmt.script.metrics();
                let wpx = st.px * k_size * fmt.scale;
                let style = &fmt.style;
                words.push(Word {
                    hyphen: false,
                    glue: true,
                    run: self.shape_fmt(w, *style, wpx, fmt.caps),
                    space,
                    style: *style,
                    text: w.to_string(),
                    ch0,
                    px: wpx,
                    rise: st.px * k_rise,
                    caps: fmt.caps,
                });
            }
            ch_base += text.chars().count() as u32;
        }
        if let Some(last) = words.last_mut() {
            last.space = 0.0;
        }
        // Tab stop: a tab after the first word of a paragraph with a negative
        // indent (numbered list « 1.\tTexte ») brings the next word to the paragraph
        // edge, where the following lines resume; fixed space, never
        // stretched by justification. Elsewhere, a tab is worth a space.
        {
            let full: String = spans.iter().map(|(t, _)| *t).collect();
            let chars: Vec<char> = full.chars().collect();
            let mut prefix = st.indent;
            for k in 0..words.len().saturating_sub(1) {
                let end = (words[k].ch0 as usize + words[k].text.chars().count()).min(chars.len());
                let next = (words[k + 1].ch0 as usize).min(chars.len());
                prefix += words[k].run.advance;
                if chars[end..next].contains(&'\t') {
                    if st.indent < 0.0 {
                        words[k].space = (-prefix).max(space_w * 0.5);
                        words[k].glue = false;
                    }
                    break;
                }
                prefix += words[k].space;
                if prefix > 0.0 {
                    break;
                }
            }
        }
        // Fixed spaces: no break before « ? ! : ; » ) ] . , … » nor after « « ( [ ».
        // Separator bullets (« A • B ») stay at the end of a line, never at the start.
        let no_break_before = |w: &str| {
            w.chars()
                .next()
                .map(|c| {
                    matches!(
                        c,
                        '?' | '!'
                            | ':'
                            | ';'
                            | '»'
                            | ')'
                            | ']'
                            | '}'
                            | '.'
                            | ','
                            | '…'
                            | '›'
                            | '•'
                            | '·'
                    )
                })
                .unwrap_or(false)
        };
        let no_break_after = |w: &str| {
            w.chars()
                .last()
                .map(|c| matches!(c, '«' | '(' | '[' | '{' | '‹'))
                .unwrap_or(false)
        };
        for i in 0..words.len().saturating_sub(1) {
            if no_break_after(&words[i].text) || no_break_before(&words[i + 1].text) {
                words[i].glue = false;
            }
        }

        let line_h = st.px * st.line_height;
        let ascent = font.ascent(st.px);
        // Optimal break over the whole paragraph (Knuth-Plass), greedy fallback
        // when no sequence of lines meets the tolerance.
        let mut passes = self.kp_passes.get();
        let broken = if self.knuth_plass.get() {
            // A single pass, with no stretch limit: Knuth-Plass minimizes the
            // demerits no matter what (a prior strict pass almost never succeeds
            // on a 40-character column and costs as much).
            // The greedy algorithm is now only used for impossible cases.
            let t0 = Instant::now();
            let r = self.break_kp(&words, width, st, self.kp_tolerance.get());
            self.t_kp.set(self.t_kp.get() + t0.elapsed());
            r.inspect(|_b| {
                passes[0] += 1;
            })
        } else {
            None
        };
        let broken = broken.unwrap_or_else(|| {
            passes[2] += 1;
            if std::env::var_os("SIGNET_KP_DEBUG").is_some() {
                let head: String = words.iter().take(6).map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                let widest = words.iter().map(|w| w.run.advance).fold(0.0f32, f32::max);
                eprintln!("repli glouton : {} mots, largeur {width:.0}, mot le plus large {widest:.0} : « {head}… »", words.len());
            }
            self.break_greedy(words.clone(), width, st)
        });
        self.kp_passes.set(passes);
        let mut lines: Vec<Line> = Vec::new();
        let n_lines = broken.len();
        for (li, pieces) in broken.into_iter().enumerate() {
            let first = li == 0;
            let avail = if first { width - st.indent } else { width };
            let count = pieces.len();
            let used: f32 = pieces
                .iter()
                .enumerate()
                .map(|(k, w)| w.run.advance + if k + 1 < count { w.space } else { 0.0 })
                .sum();
            let last_line = li + 1 == n_lines;
            let gaps = pieces[..count.saturating_sub(1)]
                .iter()
                .filter(|w| w.space > 0.0 && w.glue)
                .count() as f32;
            // Last line: never stretched, but compressed if Knuth-Plass planned it so.
            let extra = if st.align == Align::Justify && gaps > 0.0 && (!last_line || used > avail)
            {
                (avail - used) / gaps
            } else {
                0.0
            };
            let mut x = if first { st.indent } else { 0.0 };
            if st.align == Align::Center {
                x += (avail - used) / 2.0;
            } else if st.align == Align::Right {
                x += avail - used;
            }
            let baseline = lines.len() as f32 * line_h + ascent;
            let mut line = Line {
                ratio: f32::NAN,
                hyphenated: pieces.last().map(|w| w.hyphen).unwrap_or(false),
                ..Line::default()
            };
            if extra != 0.0 || (st.align == Align::Justify && !last_line && gaps > 0.0) {
                // Same unit as Knuth-Plass: stretch = ½ space, compression = ⅓.
                line.ratio = if extra >= 0.0 {
                    extra / (space_w / 2.0)
                } else {
                    extra / (space_w / 3.0)
                };
            }
            for (k, word) in pieces.iter().enumerate() {
                let widths = glyph_widths(&word.run);
                for (gi, g) in word.run.glyphs.iter().enumerate() {
                    line.glyphs.push(PositionedGlyph {
                        id: g.id,
                        x: x + g.x,
                        y: baseline + g.y + word.rise,
                        style: word.style,
                        ch: glyph_ch(&word.text, word.ch0, g.cluster),
                        w: widths[gi],
                        scale: word.px / st.px,
                    });
                }
                x += word.run.advance;
                if k + 1 < count && word.space > 0.0 {
                    x += word.space + if word.glue { extra } else { 0.0 };
                }
            }
            line.width = x;
            lines.push(line);
        }
        let height = lines.len() as f32 * line_h;
        (lines, height)
    }

    /// Hyphenation points of a word, cached (Liang costs a few tens
    /// of microseconds on the e-reader, and words repeat).
    fn hyphen_points(&self, word: &str) -> std::rc::Rc<Vec<usize>> {
        let Some(h) = self.hyphenator.as_ref() else {
            return std::rc::Rc::new(Vec::new());
        };
        if word.chars().count() < 5 {
            return std::rc::Rc::new(Vec::new());
        }
        if let Some(v) = self.hyph_cache.borrow().get(word) {
            return v.clone();
        }
        // The composer's "word" may carry its punctuation (« aujourd’hui ? »
        // with its no-break space, « (fin) », « bientôt », »): we only hyphenate
        // the alphabetic core, and shift the points accordingly.
        let t0 = Instant::now();
        self.hyph_misses.set(self.hyph_misses.get() + 1);
        let chars: Vec<char> = word.chars().collect();
        let v = if chars.contains(&'\u{ad}') {
            // Editor's soft hyphens: these are the only break points.
            std::rc::Rc::new(
                chars
                    .iter()
                    .enumerate()
                    .filter(|(i, c)| **c == '\u{ad}' && *i > 0 && i + 1 < chars.len())
                    .map(|(i, _)| i + 1)
                    .collect(),
            )
        } else {
            let is_core = |c: &char| c.is_alphabetic() || *c == '\'' || *c == '’' || *c == '-';
            let lead = chars.iter().take_while(|c| !is_core(c)).count();
            let core_len = chars[lead..].iter().take_while(|c| is_core(c)).count();
            let core: String = chars[lead..lead + core_len].iter().collect();
            std::rc::Rc::new(if core_len >= 5 {
                h.hyphenate(&core).into_iter().map(|p| p + lead).collect()
            } else {
                Vec::new()
            })
        };
        self.hyph_cache
            .borrow_mut()
            .insert(word.to_string(), v.clone());
        self.t_hyph.set(self.t_hyph.get() + t0.elapsed());
        v
    }

    /// Greedy breaking: as many words as possible per line, hyphenating the
    /// overflowing word if a head fits.
    fn break_greedy(&self, mut words: Vec<Word>, width: f32, st: ParagraphStyle) -> Vec<Vec<Word>> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut first = true;
        while i < words.len() {
            let avail = if first { width - st.indent } else { width };
            let mut j = i;
            let mut used = 0.0f32;
            while j < words.len() {
                let w = words[j].run.advance;
                let need = if j == i {
                    w
                } else {
                    used + words[j - 1].space + w
                };
                if j == i && w > avail && st.align != Align::Center {
                    // First word wider than the line: split off its longest possible head.
                    if let Some((head, tail, tail_text)) = self.try_hyphenate(
                        &words[j].text,
                        words[j].style,
                        words[j].px,
                        words[j].caps,
                        avail,
                    ) {
                        let style = words[j].style;
                        let tail_space = words[j].space;
                        let tail_glue = words[j].glue;
                        let ch0 = words[j].ch0;
                        let head_chars =
                            words[j].text.chars().count() as u32 - tail_text.chars().count() as u32;
                        let head_text: String =
                            words[j].text.chars().take(head_chars as usize).collect();
                        let added = !head_text.ends_with('-');
                        let (wpx, rise, caps) = (words[j].px, words[j].rise, words[j].caps);
                        words[j] = Word {
                            hyphen: added,
                            glue: true,
                            run: head,
                            space: 0.0,
                            style,
                            text: head_text,
                            ch0,
                            px: wpx,
                            rise,
                            caps,
                        };
                        words.insert(
                            j + 1,
                            Word {
                                hyphen: false,
                                glue: tail_glue,
                                run: tail,
                                space: tail_space,
                                style,
                                text: tail_text,
                                ch0: ch0 + head_chars,
                                px: wpx,
                                rise,
                                caps,
                            },
                        );
                        j += 1;
                        break;
                    }
                }
                if need > avail && j > i {
                    let gap = words[j - 1].space;
                    if gap == 0.0 || !words[j - 1].glue {
                        // Glued to the previous one (« Reflect » + « . », « aujourd’hui ? »): we step back
                        // to the last elastic space.
                        let mut k = j - 1;
                        while k > i && (words[k - 1].space == 0.0 || !words[k - 1].glue) {
                            k -= 1;
                        }
                        if k > i {
                            j = k;
                        }
                        break;
                    }
                    let room = avail - used - gap;
                    if let Some((head, tail, tail_text)) = self.try_hyphenate(
                        &words[j].text,
                        words[j].style,
                        words[j].px,
                        words[j].caps,
                        room,
                    ) {
                        let style = words[j].style;
                        let tail_space = words[j].space;
                        let ch0 = words[j].ch0;
                        let head_chars =
                            words[j].text.chars().count() as u32 - tail_text.chars().count() as u32;
                        let head_text: String =
                            words[j].text.chars().take(head_chars as usize).collect();
                        let tail_glue = words[j].glue;
                        let added = !head_text.ends_with('-');
                        let (wpx, rise, caps) = (words[j].px, words[j].rise, words[j].caps);
                        words[j] = Word {
                            hyphen: added,
                            glue: true,
                            run: head,
                            space: 0.0,
                            style,
                            text: head_text,
                            ch0,
                            px: wpx,
                            rise,
                            caps,
                        };
                        words.insert(
                            j + 1,
                            Word {
                                hyphen: false,
                                glue: tail_glue,
                                run: tail,
                                space: tail_space,
                                style,
                                text: tail_text,
                                ch0: ch0 + head_chars,
                                px: wpx,
                                rise,
                                caps,
                            },
                        );
                        j += 1;
                    }
                    break;
                }
                used = need;
                j += 1;
            }
            out.push(words[i..j].to_vec());
            i = j;
            first = false;
        }
        out
    }

    /// Knuth-Plass breaking: minimizes the sum of "demerits" (spaces
    /// too stretched or too tight, hyphenations) over the whole paragraph. Candidate
    /// hyphenations are measured without reshaping (glyph position
    /// at the break point + hyphen width); only the chosen breaks
    /// are shaped. `None` if no sequence of lines fits
    /// within the tolerance (maximum space stretch, in "stretch").
    fn break_kp(
        &self,
        words: &[Word],
        width: f32,
        st: ParagraphStyle,
        tol: f32,
    ) -> Option<Vec<Vec<Word>>> {
        if words.is_empty() {
            return Some(Vec::new());
        }
        #[derive(Clone, Copy)]
        enum It {
            Boxed {
                w: f32,
                word: usize,
                from: usize,
                to: usize,
            },
            Glue {
                w: f32,
                s: f32,
                z: f32,
            },
            Pen {
                w: f32,
                p: f32,
                flag: bool,
            },
        }
        let styles = [
            Style::Regular,
            Style::Italic,
            Style::Bold,
            Style::BoldItalic,
        ];
        let hyphen_ws: [f32; 4] = std::array::from_fn(|i| {
            if words.iter().any(|w| w.style as usize == i) {
                self.advance_of(styles[i], st.px, '-')
            } else {
                0.0
            }
        });
        let mut items: Vec<It> = Vec::with_capacity(words.len() * 3);
        for (k, wd) in words.iter().enumerate() {
            let hyphen_w = hyphen_ws[wd.style as usize] * wd.px / st.px;
            let nchars = wd.text.chars().count();
            let mut prev_x = 0.0f32;
            let mut prev_c = 0usize;
            // Hyphenation points (Liang, penalty 50, hyphen added); failing that,
            // for a word wider than the line (URL, unknown compound word), fallback
            // breaks: after « / . - _ » (penalty 100, no hyphen added) or, if there are none,
            // after any character
            // (penalty 400, hyphen).
            let too_wide = wd.run.advance > width - st.indent.max(0.0);
            let chars: Vec<char> = wd.text.chars().collect();
            // Proper noun: initial capital anywhere other than at the start of a sentence. Never
            // hyphenated (« Ey-rolles » is a mistake); an ordinary word at the start of a
            // sentence (« Chaque ») keeps its points, otherwise justification suffers.
            let capitalized = chars
                .iter()
                .find(|c| c.is_alphabetic())
                .map(|c| c.is_uppercase())
                .unwrap_or(false);
            let sentence_start = k == 0
                || words[k - 1]
                    .text
                    .chars()
                    .rev()
                    .find(|c| {
                        !matches!(
                            c,
                            '»' | ')' | ']' | '"' | '”' | '’' | '\'' | '\u{a0}' | '\u{202f}'
                        )
                    })
                    .map(|c| matches!(c, '.' | '?' | '!' | '…' | ':' | '«' | '(' | '“'))
                    .unwrap_or(true);
            let proper_noun = capitalized && !sentence_start;
            // Centered headings: no hyphenation, except for a word that does not fit on a line.
            let pts = if st.align == Align::Center && !too_wide {
                std::rc::Rc::new(Vec::new())
            } else {
                self.hyphen_points(&wd.text)
            };
            // Fallback breaks only for a word wider than the entire line.
            let emergency: Vec<(usize, f32, f32)> = if pts.is_empty() && too_wide && nchars >= 6 {
                let seps: Vec<usize> = (1..nchars - 1)
                    .filter(|&i| {
                        matches!(chars[i - 1], '/' | '.' | '-' | '_' | '?' | '=' | '&')
                            && !chars[i].is_whitespace()
                    })
                    .collect();
                if !seps.is_empty() {
                    seps.into_iter().map(|i| (i, 0.0, 100.0)).collect()
                } else {
                    (3..nchars - 2).map(|i| (i, hyphen_w, 400.0)).collect()
                }
            } else {
                Vec::new()
            };
            // A proper noun is six times more expensive to split: only if the line would be
            // really loose otherwise (forbidding it entirely costs 1.3 points of average
            // ratio on a book full of brand names and names).
            let hp = self.kp_hyphen_penalty.get() * if proper_noun { 6.0 } else { 1.0 };
            // Letters (and apostrophes) before the break point, from the start of the word
            // or the last hyphen: fewer than three, the break is very expensive.
            let short_hp = self.kp_short_head_penalty.get();
            let head_letters = |p: usize| {
                chars[..p]
                    .iter()
                    .rev()
                    .take_while(|c| **c != '-')
                    .filter(|c| c.is_alphanumeric() || **c == '’' || **c == '\'')
                    .count()
            };
            // After an existing hyphen (« porte-monnaie »), no hyphen added.
            let mut candidates: Vec<(usize, f32, f32)> = if emergency.is_empty() {
                pts.iter()
                    .map(|&p| {
                        (
                            p,
                            if chars[p - 1] == '-' { 0.0 } else { hyphen_w },
                            if chars[p - 1] != '-' && head_letters(p) < 3 {
                                hp.max(short_hp)
                            } else {
                                hp
                            },
                        )
                    })
                    .collect()
            } else {
                emergency
            };
            // After a slash (« fascination/addiction », URL): break without hyphen (UAX 14, class SY).
            for i in 1..nchars.saturating_sub(1) {
                if chars[i - 1] == '/' && !candidates.iter().any(|c| c.0 == i) {
                    candidates.push((i, 0.0, 50.0));
                }
            }
            candidates.sort_by_key(|c| c.0);
            if !candidates.is_empty() {
                // Byte of each character and x of the glyph starting at that byte,
                // computed once per word (not per break point).
                let byte_of: Vec<usize> = wd.text.char_indices().map(|(b, _)| b).collect();
                let mut x_at_byte: Vec<Option<f32>> = vec![None; wd.text.len() + 1];
                for g in &wd.run.glyphs {
                    if let Some(slot) = x_at_byte.get_mut(g.cluster as usize) {
                        if slot.is_none() {
                            *slot = Some(g.x);
                        }
                    }
                }
                for (pt, pw, pp) in candidates {
                    let Some(&byte) = byte_of.get(pt) else {
                        continue;
                    };
                    let Some(Some(gx)) = x_at_byte.get(byte) else {
                        continue;
                    };
                    let gx = *gx;
                    if gx <= prev_x {
                        continue;
                    }
                    items.push(It::Boxed {
                        w: gx - prev_x,
                        word: k,
                        from: prev_c,
                        to: pt,
                    });
                    items.push(It::Pen {
                        w: pw,
                        p: pp,
                        flag: true,
                    });
                    prev_x = gx;
                    prev_c = pt;
                }
            }
            items.push(It::Boxed {
                w: wd.run.advance - prev_x,
                word: k,
                from: prev_c,
                to: nchars,
            });
            if k + 1 < words.len() && wd.space > 0.0 {
                if wd.glue {
                    // In ragged text (headings, quotations) spaces are not compressed.
                    let z = if st.align == Align::Justify {
                        wd.space * self.kp_shrink.get()
                    } else {
                        0.0
                    };
                    items.push(It::Glue {
                        w: wd.space,
                        s: wd.space / 2.0,
                        z,
                    });
                } else {
                    // Fixed space: an empty box, neither break nor stretch.
                    items.push(It::Boxed {
                        w: wd.space,
                        word: usize::MAX,
                        from: 0,
                        to: 0,
                    });
                }
            }
        }
        items.push(It::Glue {
            w: 0.0,
            s: 1.0e6,
            z: 0.0,
        });
        items.push(It::Pen {
            w: 0.0,
            p: f32::NEG_INFINITY,
            flag: false,
        });
        // Cumulative sums (width, stretch, shrink) before each item.
        let n = items.len();
        let (mut sw, mut ss, mut sz) = (
            vec![0.0f32; n + 1],
            vec![0.0f32; n + 1],
            vec![0.0f32; n + 1],
        );
        for (i, it) in items.iter().enumerate() {
            let (w, s, z) = match *it {
                It::Boxed { w, .. } => (w, 0.0, 0.0),
                It::Glue { w, s, z } => (w, s, z),
                It::Pen { .. } => (0.0, 0.0, 0.0),
            };
            sw[i + 1] = sw[i] + w;
            ss[i + 1] = ss[i] + s;
            sz[i + 1] = sz[i] + z;
        }
        // One node per (break, spacing class, run of hyphenations): this is what
        // allows penalizing badly matched neighbors (TeX) and bounding
        // consecutive hyphenations (Apple Books), without losing optimality.
        struct Node {
            item: usize,
            line: usize,
            dem: f64,
            prev: usize,
            flag: bool,
            /// Spacing class of the line ending here: 0 very loose, 1 loose, 2 normal, 3 tight.
            fit: u8,
            /// Consecutive hyphenated lines ending here.
            run: u8,
        }
        let max_run = self.kp_max_hyphen_run.get().max(1);
        // Ragged text (left, right, centered): the line end is free, like
        // TeX's \rightskip. Without this spring, a short line looks "loose" and
        // the algorithm hyphenates to fill it (« princi-pal »). 6 em: a line
        // that is missing a word is not penalized, a half-empty line is.
        let ragged = if st.align == Align::Justify {
            0.0
        } else {
            st.px * 6.0
        };
        let adj = self.kp_adj_demerits.get() as f64;
        let mut nodes: Vec<Node> = vec![Node {
            item: usize::MAX,
            line: 0,
            dem: 0.0,
            prev: usize::MAX,
            flag: false,
            fit: 2,
            run: 0,
        }];
        let mut active: Vec<usize> = vec![0];
        let mut last: Option<usize> = None;
        for b in 0..n {
            let (is_break, pen_w, pen_p, flag, forced) = match items[b] {
                It::Glue { .. } => (
                    b > 0 && matches!(items[b - 1], It::Boxed { .. }),
                    0.0,
                    0.0,
                    false,
                    false,
                ),
                It::Pen { w, p, flag, .. } => (true, w, p, flag, p == f32::NEG_INFINITY),
                It::Boxed { .. } => (false, 0.0, 0.0, false, false),
            };
            if !is_break {
                continue;
            }
            let mut best: [[Option<(f64, usize)>; 4]; 4] = [[None; 4]; 4];
            let mut kept = 0usize;
            for ai in 0..active.len() {
                let a = active[ai];
                let node = &nodes[a];
                let mut start = if node.item == usize::MAX {
                    0
                } else {
                    node.item + 1
                };
                while start < b && matches!(items[start], It::Glue { .. }) {
                    start += 1;
                }
                let natural = sw[b] - sw[start] + pen_w;
                let stretch = ss[b] - ss[start] + ragged;
                let shrink = sz[b] - sz[start];
                let avail = if node.line == 0 {
                    width - st.indent
                } else {
                    width
                };
                let diff = avail - natural;
                // Without an elastic space (single-block line), we reason as if there were
                // a space: a short line remains very bad, not infinite.
                let one_gap = st.px * 0.1;
                let r = if diff > 0.0 {
                    diff / if stretch > 0.0 { stretch } else { one_gap }
                } else if diff < 0.0 {
                    if shrink > 0.0 {
                        diff / shrink
                    } else {
                        f32::NEG_INFINITY
                    }
                } else {
                    0.0
                };
                if r < -1.0 {
                    // Too long: this node will no longer be used (lines only get longer).
                    continue;
                }
                active[kept] = a;
                kept += 1;
                if r > tol && !forced {
                    // Too loose for this pass; the node stays active for a break further on.
                    continue;
                }
                let run = if flag { node.run + 1 } else { 0 };
                if run > max_run {
                    continue; // too many hyphenated lines in a row
                }
                let fit: u8 = if r < -0.5 {
                    3
                } else if r <= 0.5 {
                    2
                } else if r <= 1.0 {
                    1
                } else {
                    0
                };
                // TeX demerits: (\linepenalty + badness)² + penalty², the penalty
                // is added squared instead of going inside the square: a hyphenation stays
                // affordable when it avoids a loose line.
                let badness = 100.0 * (r.abs() as f64).powi(3);
                let base = (10.0 + badness).powi(2);
                let mut dem = if pen_p.is_finite() && pen_p > 0.0 {
                    base + (pen_p as f64).powi(2)
                } else {
                    base
                };
                if flag && node.flag {
                    dem += 10_000.0; // two hyphenations in a row (TeX's \doublehyphendemerits)
                }
                if forced && node.flag {
                    dem += 5_000.0; // hyphenation on the second-to-last line (\finalhyphendemerits)
                }
                if (fit as i32 - node.fit as i32).abs() > 1 {
                    dem += adj; // loose neighbor next to a tight one (\adjdemerits)
                }
                dem += node.dem;
                let slot = &mut best[fit as usize][run.min(3) as usize];
                if slot.map(|(d, _)| dem < d).unwrap_or(true) {
                    *slot = Some((dem, a));
                }
            }
            active.truncate(kept);
            let mut best_forced: Option<(f64, usize)> = None;
            for (fit, row) in best.iter().enumerate() {
                for (run, cell) in row.iter().enumerate() {
                    if let Some((dem, a)) = *cell {
                        let line = nodes[a].line + 1;
                        nodes.push(Node {
                            item: b,
                            line,
                            dem,
                            prev: a,
                            flag,
                            fit: fit as u8,
                            run: run as u8,
                        });
                        let idx = nodes.len() - 1;
                        if forced {
                            if best_forced.map(|(d, _)| dem < d).unwrap_or(true) {
                                best_forced = Some((dem, idx));
                            }
                        } else {
                            active.push(idx);
                        }
                    }
                }
            }
            if let Some((_, idx)) = best_forced {
                last = Some(idx);
            }
            if active.is_empty() {
                return None;
            }
        }
        let last = last?;
        // Backtrack through the breaks.
        let mut breaks: Vec<usize> = Vec::new();
        let mut cur = last;
        while nodes[cur].item != usize::MAX {
            breaks.push(nodes[cur].item);
            cur = nodes[cur].prev;
        }
        breaks.reverse();
        // Rebuild the lines: word pieces, shaped hyphenations.
        let mut out: Vec<Vec<Word>> = Vec::with_capacity(breaks.len());
        let mut start = 0usize;
        for &b in &breaks {
            while start < b && matches!(items[start], It::Glue { .. }) {
                start += 1;
            }
            // (word, first character, last character exclusive) per word of the line
            let mut parts: Vec<(usize, usize, usize)> = Vec::new();
            for it in &items[start..b] {
                if let It::Boxed { word, from, to, .. } = *it {
                    if word == usize::MAX {
                        continue; // fixed space
                    }
                    match parts.last_mut() {
                        Some(p) if p.0 == word => p.2 = to,
                        _ => parts.push((word, from, to)),
                    }
                }
            }
            let mut line: Vec<Word> = Vec::with_capacity(parts.len());
            for (k, from, to) in parts {
                let wd = &words[k];
                let nchars = wd.text.chars().count();
                if from == 0 && to == nchars {
                    line.push(wd.clone());
                } else {
                    let chars: Vec<char> = wd.text.chars().collect();
                    let text: String = chars[from..to].iter().collect();
                    // Fallback break after a separator (« / », « . », « - »): no hyphen added.
                    let add_hyphen = to < nchars
                        && !matches!(chars[to - 1], '/' | '.' | '-' | '_' | '?' | '=' | '&');
                    let visible: String = text.chars().filter(|c| *c != '\u{ad}').collect();
                    let shaped_text = if add_hyphen {
                        format!("{visible}-")
                    } else {
                        visible
                    };
                    let run = self.shape_fmt(&shaped_text, wd.style, wd.px, wd.caps);
                    line.push(Word {
                        hyphen: add_hyphen,
                        glue: wd.glue,
                        run,
                        space: if to == nchars { wd.space } else { 0.0 },
                        style: wd.style,
                        text,
                        ch0: wd.ch0 + from as u32,
                        px: wd.px,
                        rise: wd.rise,
                        caps: wd.caps,
                    });
                }
            }
            if !line.is_empty() {
                out.push(line);
            }
            start = b + 1;
        }
        Some(out)
    }

    /// Draws lines typeset by [`layout`](Self::layout) at (`x0`, `y0`).
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        lines: &[Line],
        x0: i32,
        y0: i32,
        st: ParagraphStyle,
        gray: u8,
    ) {
        let binary = self.binary;
        for line in lines {
            for g in &line.glyphs {
                let gx = x0 + g.x.round() as i32;
                let gy = y0 + g.y.round() as i32;
                let key = self.glyph_key(g.style);
                let font = &self.fonts[self.family.get() as usize][g.style as usize];
                if let Some(bmp) = self.cache.get(key, font, g.id, st.px * g.scale) {
                    blit_glyph_ink(canvas, bmp, gx, gy, gray, binary, &self.ink);
                }
            }
        }
    }

    /// Enables binary rendering (pure black and white) of page text.
    pub fn set_binary(&mut self, on: bool) {
        self.binary = on;
    }
    /// Ink contrast (gamma) and emboldening, see [`Ink::new`].
    pub fn set_ink(&mut self, gamma: f32, embolden: f32) {
        self.ink = Ink::new(gamma, embolden);
    }
    /// Enables or disables hinting (empties the glyph cache).
    pub fn set_hinting(&mut self, on: bool) {
        for f in self.fonts.iter().flatten() {
            f.hinting.set(on);
        }
        self.cache = GlyphCache::new();
    }
    pub fn binary(&self) -> bool {
        self.binary
    }

    /// Shortcut: typesets and draws a single unbroken line.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        baseline_y: i32,
        style: Style,
        px: f32,
        gray: u8,
    ) -> f32 {
        let run = self.font(style).shape(text, px);
        let key = self.glyph_key(style);
        for g in &run.glyphs {
            let font = &self.fonts[self.family.get() as usize][style as usize];
            if let Some(bmp) = self.cache.get(key, font, g.id, px) {
                blit_glyph_ink(
                    canvas,
                    bmp,
                    x + g.x.round() as i32,
                    baseline_y + g.y.round() as i32,
                    gray,
                    false,
                    &self.ink,
                );
            }
        }
        run.advance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literata() -> Typesetter {
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/literata/");
        let load =
            |n: &str| Font::from_bytes(std::fs::read(format!("{base}{n}.ttf")).unwrap()).unwrap();
        let fr = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/hyphenation/hyph-fr.tex"
        ))
        .unwrap();
        Typesetter::new(
            load("Literata-Regular"),
            load("Literata-Italic"),
            load("Literata-Bold"),
            load("Literata-BoldItalic"),
        )
        .with_hyphenation(Hyphenator::from_tex(&fr))
    }

    #[test]
    fn hinting_instance_exists_and_rasterizes_fast() {
        let ts = literata();
        let f = ts.font(Style::Regular);
        assert!(
            f.hinter(26.0).is_some(),
            "pas d'instance de hinting pour Literata"
        );
        let ids: Vec<u16> = f
            .shape(
                "Mais faut-il vraiment être toujours occupé ? Le divertissement sans fin",
                26.0,
            )
            .glyphs
            .iter()
            .map(|g| g.id)
            .collect();
        let t = std::time::Instant::now();
        let mut n = 0;
        for id in &ids {
            if rasterize(f, *id, 26.0).is_some() {
                n += 1;
            }
        }
        let per = t.elapsed() / ids.len() as u32;
        eprintln!("{n} glyphes hintés rastérisés, {per:?} par glyphe");
        assert!(n > 40);
        assert!(
            per < std::time::Duration::from_millis(5),
            "trop lent : {per:?}"
        );
        // Same glyph twice → same image (hinting is deterministic).
        let a = rasterize(f, ids[0], 26.0).unwrap();
        let b = rasterize(f, ids[0], 26.0).unwrap();
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn insecables_coupures_de_secours_et_tirets() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        // Narrow no-break U+202F before « ? »: never a line starting with « ? ».
        let text = "Vous avez vraiment tout fait aujourd’hui\u{202f}? Non, pas encore, loin de là.";
        let chars: Vec<char> = text.chars().collect();
        for width in (180..420).step_by(9) {
            let (lines, _) = ts.layout_rich(&[(text, Style::Regular)], width as f32, st);
            for l in &lines {
                let first = chars[l.glyphs[0].ch as usize];
                assert_ne!(first, '?', "« ? » en début de ligne à {width}px");
            }
        }
        // URL wider than the column: split without a hyphen, without overflow.
        let url = "voir https://exemple.org/un/chemin/vraiment/tres/long/page.html ici";
        let (lines, _) = ts.layout_rich(&[(url, Style::Regular)], 300.0, st);
        assert!(lines.len() >= 3, "URL non coupée : {} lignes", lines.len());
        for l in &lines {
            assert!(l.width <= 300.5, "débordement : {}", l.width);
            assert!(!l.hyphenated, "trait d'union ajouté dans une URL");
        }
        // Compound word split at the hyphen: only one visible hyphen (not « porte-- »).
        let (lines, _) = ts.layout_rich(&[("un porte-monnaie", Style::Regular)], 150.0, st);
        let hyphens = lines
            .iter()
            .flat_map(|l| &l.glyphs)
            .filter(|g| chars_of("un porte-monnaie")[g.ch as usize] == '-')
            .count();
        assert!(hyphens <= 1, "{hyphens} tirets");
        // Editor's soft hyphen: the only break point, with no duplicate hyphen added.
        let shy = "aaaa\u{ad}bbbb\u{ad}cccc";
        let (lines, _) = ts.layout_rich(&[(shy, Style::Regular)], 90.0, st);
        assert!(lines.len() >= 2, "tiret conditionnel ignoré");
        assert!(
            lines[0].hyphenated,
            "pas de trait d'union au tiret conditionnel"
        );
        let hyphen_id = ts.font(Style::Regular).shape("-", 24.0).glyphs[0].id;
        assert!(
            lines[0].glyphs.iter().any(|g| g.id == hyphen_id),
            "trait d'union absent"
        );
        // Centered heading with a long word (that fits): never chopped up.
        let stc = ParagraphStyle {
            align: Align::Center,
            ..st
        };
        let (lines, _) = ts.layout_rich(
            &[("Chapitre premier : Introduction", Style::Bold)],
            300.0,
            stc,
        );
        assert!(lines.iter().all(|l| !l.hyphenated), "titre centré césuré");
        // Centered heading: never hyphenated.
        let stc = ParagraphStyle {
            align: Align::Center,
            ..st
        };
        let (lines, _) = ts.layout_rich(
            &[("La productivité retrouvée enfin", Style::Bold)],
            200.0,
            stc,
        );
        assert!(lines.iter().all(|l| !l.hyphenated));
    }

    fn chars_of(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn literata_has_superscript_and_subscript_digits() {
        let ts = literata();
        let f = ts.font(Style::Regular);
        let notdef = f.shape("\u{10FFFD}", 24.0).glyphs[0].id;
        for (label, text) in [("exposants", "⁰¹²³⁴⁵⁶⁷⁸⁹"), ("indices", "₀₁₂₃₄₅₆₇₈₉")]
        {
            let run = f.shape(text, 24.0);
            let missing = run
                .glyphs
                .iter()
                .filter(|g| g.id == notdef || g.id == 0)
                .count();
            assert_eq!(missing, 0, "{label} : {missing} glyphes manquants");
        }
    }

    /// At most two hyphenated lines in a row (Apple Books: hyphenate-limit-lines: 2),
    /// whereas without the rule the same narrow column chains more of them.
    #[test]
    fn at_most_two_hyphenated_lines_in_a_row() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        let text = "anticonstitutionnellement extraordinairement particulièrement incompréhensiblement réorganisation \
            développement administrativement institutionnalisation professionnellement internationalisation \
            caractéristiques expérimentalement indépendamment psychologiquement intellectuellement \
            responsabilité communication imperturbablement substantiellement occasionnellement".repeat(4);
        let max_run = |lines: &[Line]| {
            lines
                .iter()
                .fold((0usize, 0usize), |(cur, best), l| {
                    if l.hyphenated {
                        (cur + 1, best.max(cur + 1))
                    } else {
                        (0, best)
                    }
                })
                .1
        };
        let (lines, _) = ts.layout_rich(&[(&text, Style::Regular)], 260.0, st);
        assert!(
            max_run(&lines) <= 2,
            "{} lignes césurées d'affilée",
            max_run(&lines)
        );
        ts.kp_max_hyphen_run.set(9);
        let (free, _) = ts.layout_rich(&[(&text, Style::Regular)], 260.0, st);
        assert!(
            max_run(&free) > 2,
            "le test ne met pas la règle à l'épreuve"
        );
    }

    /// In ragged text, hyphenation is only a last resort: table-of-contents lines
    /// that fit on two lines without splitting are not hyphenated.
    #[test]
    fn ragged_text_avoids_hyphenation() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Left,
            indent: 0.0,
        };
        for text in ["Highlight (Priorité) : démarrez votre journée en choisissant un objectif principal", "Laser (Concentration) : chassez les distractions afin de libérer du temps pour votre Highlight"] {
            let (lines, _) = ts.layout_rich(&[(text, Style::Regular)], 585.0, st);
            assert!(lines.iter().all(|l| !l.hyphenated), "césure en drapeau : « {text} »");
        }
    }

    /// Numbered list with negative indent: each item's text starts at the
    /// same place (the paragraph edge), whatever the width of the number,
    /// and justification does not widen the space after the number.
    #[test]
    fn numbered_list_text_starts_at_the_hanging_indent() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: -36.0,
        };
        let first_x = |text: &str| {
            let (lines, _) = ts.layout_rich(&[(text, Style::Regular)], 560.0, st);
            let tab = text.chars().position(|c| c == '\t').unwrap() as u32;
            lines[0]
                .glyphs
                .iter()
                .find(|g| g.ch > tab)
                .map(|g| g.x)
                .unwrap()
        };
        let a = first_x("1.\tChoisir ses priorités : Highlight.");
        let b = first_x("3.\tAugmenter son niveau d’énergie quotidienne : Energize.");
        assert!(
            a.abs() < 0.5 && b.abs() < 0.5,
            "texte à x = {a} et {b}, attendu 0"
        );
    }

    /// Superscript: smaller, raised, glued to the preceding word, and drawn at its
    /// size (« XXIᵉ siècle »).
    #[test]
    fn superscript_letters_are_small_and_raised() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Left,
            indent: 0.0,
        };
        let spans = [
            ("Au XXI", Style::Regular, Script::Normal),
            ("e", Style::Regular, Script::Sup),
            (" siècle", Style::Regular, Script::Normal),
        ];
        let (lines, _) = ts.layout_scripted(&spans, 600.0, st);
        let g: Vec<&PositionedGlyph> = lines[0].glyphs.iter().collect();
        let e = g.iter().find(|p| p.ch == 6).expect("glyphe de l'exposant");
        let i = g.iter().find(|p| p.ch == 5).unwrap();
        assert!((e.scale - 0.65).abs() < 1e-3, "taille {}", e.scale);
        assert!(e.y < i.y - 5.0, "pas surélevé : {} contre {}", e.y, i.y);
        assert!(e.x - (i.x + i.w) < 1.0, "pas collé au I");
        // The next word stays in place: a normal space after the superscript.
        let s = g.iter().find(|p| p.ch == 8).unwrap();
        assert!(s.x > e.x + 5.0 && (s.scale - 1.0).abs() < 1e-3);
    }

    /// Small caps: the glyphs change (c2sc), the height drops, and a
    /// reduced segment (`<small>`) is typeset smaller.
    #[test]
    fn small_caps_and_inline_sizes() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Left,
            indent: 0.0,
        };
        let normal = ts.shape_cached("XXI", Style::Regular, 30.0);
        let sc = ts.shape_fmt("XXI", Style::Regular, 30.0, Caps::AllSmallCaps);
        assert!(
            normal
                .glyphs
                .iter()
                .zip(&sc.glyphs)
                .all(|(a, b)| a.id != b.id),
            "glyphes inchangés"
        );
        assert!(
            sc.advance < normal.advance,
            "petites capitales pas plus étroites"
        );
        let spans = [
            ("Au ", Fmt::plain(Style::Regular)),
            (
                "note",
                Fmt {
                    scale: 0.8,
                    ..Fmt::plain(Style::Regular)
                },
            ),
            (" fin", Fmt::plain(Style::Regular)),
        ];
        let (lines, _) = ts.layout_formatted(&spans, 600.0, st);
        let small = lines[0].glyphs.iter().find(|g| g.ch == 3).unwrap();
        assert!((small.scale - 0.8).abs() < 1e-3);
    }

    #[test]
    fn right_aligned_lines_end_at_the_margin() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Right,
            indent: 0.0,
        };
        let (lines, _) =
            ts.layout_rich(&[("www.editions-eyrolles.com", Style::Regular)], 400.0, st);
        assert_eq!(lines.len(), 1);
        assert!((lines[0].width - 400.0).abs() < 1.0, "{}", lines[0].width);
        assert!(lines[0].glyphs[0].x > 50.0, "x = {}", lines[0].glyphs[0].x);
    }

    #[test]
    fn no_break_between_glued_words() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        // « Reflect » in bold followed by a regular « . »: glued, never separated.
        let spans = [
            ("Faire le point en fin de journée : ", Style::Regular),
            ("Reflect", Style::Bold),
            (".", Style::Regular),
        ];
        let full = ts.layout_rich(&spans, 2000.0, st).0[0].width;
        let dot = spans
            .iter()
            .map(|(t, _)| t.chars().count() as u32)
            .sum::<u32>()
            - 1;
        for width in (200..(full as i32)).step_by(7) {
            let (lines, _) = ts.layout_rich(&spans, width as f32, st);
            for l in &lines {
                let first = l.glyphs.first().unwrap();
                // A line never starts with the period (ch = last character).
                assert_ne!(first.ch, dot, "point isolé en début de ligne à {width}px");
                assert!(
                    l.width <= width as f32 + 0.5,
                    "débordement à {width}px : {}",
                    l.width
                );
            }
        }
    }

    #[test]
    fn glyph_char_indices_follow_source_text() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Left,
            indent: 0.0,
        };
        let spans = [
            ("Il ", Style::Regular),
            ("marcha", Style::Italic),
            (" encore.", Style::Regular),
        ];
        let (lines, _) = ts.layout_rich(&spans, 1000.0, st);
        let g = &lines[0].glyphs;
        // "Il marcha encore.": I=0 l=1 m=3 … e=10 (after "Il marcha ")
        assert_eq!(g[0].ch, 0);
        assert_eq!(g[1].ch, 1);
        let m = g.iter().find(|x| x.style == Style::Italic).unwrap();
        assert_eq!(m.ch, 3);
        let last_word = g.iter().filter(|x| x.ch >= 10).count();
        assert!((6..=7).contains(&last_word), "{last_word}");
        assert!(g.iter().all(|x| x.w > 0.0));
    }

    #[test]
    fn shape_cache_roundtrip() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        ts.layout(
            "La forêt s'était tue. Seul persistait le murmure léger du vent.",
            400.0,
            st,
        );
        let n = ts.shaped_len();
        assert!(n >= 10);
        let data = ts.export_shaped();
        let ts2 = literata();
        assert_eq!(ts2.import_shaped(&data), n);
        let a = ts.shape_cached("murmure", Style::Regular, 30.0);
        let b = ts2.shape_cached("murmure", Style::Regular, 30.0);
        assert_eq!(a.advance, b.advance);
        assert_eq!(a.glyphs.len(), b.glyphs.len());
        assert_eq!(ts2.shape_hits.get(), 1);
    }

    #[test]
    fn hyphenation_splits_long_word_at_line_end() {
        let ts = literata();
        // Justified text: hyphenation avoids a stretched single-word line (in ragged text,
        // « comme une / respiration / lointaine » is preferable, see the next test).
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        // Width chosen so that « respiration » just overflows.
        let text = "comme une respiration lointaine";
        let w = ts.font(Style::Regular).measure("comme une respi-", 30.0) + 2.0;
        let (lines, _) = ts.layout(text, w, st);
        assert_eq!(lines.len(), 2, "{lines:?}");
        // The first line ends with a hyphen.
        let hyphen_id = ts.font(Style::Regular).shape("-", 30.0).glyphs[0].id;
        assert_eq!(lines[0].glyphs.last().unwrap().id, hyphen_id);
    }

    #[test]
    fn shaping_applies_ligature_and_kerning() {
        let ts = literata();
        let f = ts.font(Style::Regular);
        // "fi" must become a ligature: fewer glyphs than characters.
        let run = f.shape("fi", 24.0);
        assert!(run.glyphs.len() <= 2);
        // The "AV" kerning must tighten the pair.
        let av = f.measure("AV", 48.0);
        let a = f.measure("A", 48.0);
        let v = f.measure("V", 48.0);
        assert!(av < a + v, "AV={av} A={a} V={v}");
    }

    #[test]
    fn rich_layout_keeps_styles_and_justifies() {
        let ts = literata();
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 24.0,
            line_height: 1.4,
            align: Align::Justify,
            indent: 0.0,
        };
        let spans = [
            ("La forêt ", Style::Regular),
            ("s'était ", Style::Italic),
            ("tue. Seul ", Style::Regular),
            ("persistait", Style::Bold),
            (
                " le murmure léger du vent dans les branches, comme une respiration lointaine.",
                Style::Regular,
            ),
        ];
        let (lines, _) = ts.layout_rich(&spans, 300.0, st);
        assert!(lines.len() >= 3);
        assert!(lines[0].glyphs.iter().any(|g| g.style == Style::Italic));
        assert!(lines
            .iter()
            .flat_map(|l| &l.glyphs)
            .any(|g| g.style == Style::Bold));
        for l in &lines[..lines.len() - 1] {
            assert!(
                (l.width - 300.0).abs() < 1.0,
                "ligne non justifiée : {}",
                l.width
            );
        }
    }

    #[test]
    fn layout_justifies_and_renders_page() {
        let mut ts = literata();
        let text = "La forêt s'était tue. Seul persistait le murmure léger du vent dans les branches, \
                    comme une respiration lointaine. Il marcha encore un moment, sans savoir vraiment \
                    où il allait, mais avec cette sensation rare d'être exactement à sa place. \
                    Au détour du sentier, la vallée s'ouvrit devant lui, immense et silencieuse.";
        let st = ParagraphStyle {
            style: Style::Regular,
            px: 30.0,
            line_height: 1.45,
            align: Align::Justify,
            indent: 0.0,
        };
        let width = 758.0 - 2.0 * 64.0;
        let t0 = std::time::Instant::now();
        let (lines, h) = ts.layout(text, width, st);
        let layout_t = t0.elapsed();
        assert!(lines.len() >= 5);
        for l in &lines[..lines.len() - 1] {
            assert!(
                (l.width - width).abs() < 1.0,
                "ligne non justifiée : {} vs {width}",
                l.width
            );
        }
        let mut c = Canvas::new(758, 1024);
        c.clear(0xFF);
        ts.draw_text(&mut c, "CHAPITRE IV", 64, 120, Style::Regular, 34.0, 0x00);
        let t1 = std::time::Instant::now();
        ts.draw(&mut c, &lines, 64, 200, st, 0x00);
        let draw_t = t1.elapsed();
        let t2 = std::time::Instant::now();
        ts.draw(&mut c, &lines, 64, 200 + h as i32 + 20, st, 0x00);
        let draw_cached_t = t2.elapsed();
        let out = std::env::var("SIGNET_TEST_OUT")
            .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        c.save_pgm(&format!("{out}/signet-text-page.pgm")).unwrap();
        eprintln!("layout {layout_t:?}, premier rendu {draw_t:?}, rendu avec cache {draw_cached_t:?}, cache {} glyphes", ts.cache.len());
    }
}
