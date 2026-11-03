//! Pagination of a chapter: from blocks (EPUB) to pages (lists of positioned
//! lines), for a given typographic configuration.

use signet_epub::Block;
use signet_text::{Align, Line, ParagraphStyle, Style, Typesetter};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageConfig {
    pub px: f32,
    pub line_height: f32,
    pub margin_x: i32,
    pub margin_top: i32,
    pub margin_bottom: i32,
    pub screen_w: i32,
    pub screen_h: i32,
    /// Text font family (0: Literata), see `theme::FAMILIES`.
    pub family: u8,
}

impl PageConfig {
    pub fn text_width(&self) -> f32 {
        (self.screen_w - 2 * self.margin_x) as f32
    }
    pub fn text_height(&self) -> f32 {
        (self.screen_h - self.margin_top - self.margin_bottom) as f32
    }
}

/// A typeset block, placed at an ordinate in the page.
#[derive(Clone, Debug)]
pub struct PageItem {
    pub y: f32,
    pub lines: Vec<Line>,
    pub style: ParagraphStyle,
    /// Horizontal offset (quotations).
    pub x: f32,
    pub kind: ItemKind,
    /// Index of the source block in the chapter (anchor for annotations).
    pub block: usize,
    /// Width available for the block (centering of images in a box).
    pub width: f32,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Text,
    Separator,
    ImagePlaceholder {
        alt: String,
    },
    /// Image, at its display size; its pixels are decoded at display time.
    Image(std::rc::Rc<PageImage>),
}

/// Image of a page. Typesetting only needs the space it occupies
/// (read from the file header): pixels are decoded, at this exact
/// size, only for displayed pages. Typesetting an illustrated chapter no longer decodes
/// anything, and an image shrunk to fit at the bottom of a page is no longer
/// recomputed on every typesetting pass.
#[derive(Debug)]
pub struct PageImage {
    pub href: String,
    pub width: u32,
    pub height: u32,
    pixels: std::cell::RefCell<Option<std::rc::Rc<crate::draw::Gray>>>,
}

impl PageImage {
    pub fn new(href: &str, width: u32, height: u32) -> Self {
        PageImage {
            href: href.to_string(),
            width,
            height,
            pixels: Default::default(),
        }
    }
    /// Pixels, if they were supplied.
    pub fn pixels(&self) -> Option<std::rc::Rc<crate::draw::Gray>> {
        self.pixels.borrow().clone()
    }
    pub fn set_pixels(&self, g: std::rc::Rc<crate::draw::Gray>) {
        *self.pixels.borrow_mut() = Some(g);
    }
}

/// Images of an open book: display size of each for the current page
/// configuration (read from the header), and pixels decoded per exact size.
#[derive(Default)]
pub struct BookImages {
    cfg: Option<PageConfig>,
    sizes: std::collections::HashMap<String, Option<(u32, u32)>>,
    pixels: std::collections::HashMap<(String, u32, u32), std::rc::Rc<crate::draw::Gray>>,
    /// Cumulated times: header reading, decoding.
    pub t_sizes: std::time::Duration,
    pub t_decode: std::time::Duration,
}

impl BookImages {
    /// Display size: one CSS pixel is the text size divided by 16
    /// (Apple Books typesets its text at 16 CSS px), within the text width and
    /// 90% of its height.
    pub fn size(
        &mut self,
        book: &mut signet_epub::Book,
        href: &str,
        cfg: &PageConfig,
    ) -> Option<(u32, u32)> {
        if self.cfg != Some(*cfg) {
            self.sizes.clear();
            self.cfg = Some(*cfg);
        }
        if let Some(s) = self.sizes.get(href) {
            return *s;
        }
        let t0 = std::time::Instant::now();
        let (max_w, max_h) = (cfg.text_width() as u32, (cfg.text_height() * 0.9) as u32);
        let s = book
            .read_bytes(href)
            .ok()
            .and_then(|b| crate::draw::image_size(&b))
            .map(|(w, h)| crate::draw::fit_size(w, h, cfg.px / 16.0, max_w, max_h));
        self.sizes.insert(href.to_string(), s);
        self.t_sizes += t0.elapsed();
        s
    }

    /// Pixels at the exact size `w × h` (decoded as close as possible, then adjusted).
    pub fn pixels(
        &mut self,
        book: &mut signet_epub::Book,
        href: &str,
        w: u32,
        h: u32,
    ) -> Option<std::rc::Rc<crate::draw::Gray>> {
        let key = (href.to_string(), w, h);
        if let Some(g) = self.pixels.get(&key) {
            return Some(g.clone());
        }
        let t0 = std::time::Instant::now();
        let g = book
            .read_bytes(href)
            .ok()
            .and_then(|b| crate::draw::decode_image_for(&b, w, h))
            .map(|g| std::rc::Rc::new(g.resize_exact(w, h)));
        self.t_decode += t0.elapsed();
        if let Some(g) = &g {
            // At most about forty images in memory (one illustrated chapter).
            if self.pixels.len() >= 40 {
                self.pixels.clear();
            }
            self.pixels.insert(key, g.clone());
        }
        g
    }
}

/// Decodes an image at a given size: (href, width, height).
type ImageLoader<'a> = dyn FnMut(&str, u32, u32) -> Option<std::rc::Rc<crate::draw::Gray>> + 'a;

/// Supplies the pixels of a page's images that don't have any yet: `load`
/// receives (href, width, height) and returns the image decoded at that size.
pub fn resolve_images(page: &Page, load: &mut ImageLoader<'_>) {
    for it in &page.items {
        if let ItemKind::Image(img) = &it.kind {
            if img.pixels().is_none() {
                if let Some(g) = load(&img.href, img.width, img.height) {
                    img.set_pixels(g);
                }
            }
        }
    }
}

impl PartialEq for ItemKind {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ItemKind::Text, ItemKind::Text) | (ItemKind::Separator, ItemKind::Separator) => true,
            (ItemKind::ImagePlaceholder { alt: a }, ItemKind::ImagePlaceholder { alt: b }) => {
                a == b
            }
            (ItemKind::Image(a), ItemKind::Image(b)) => std::rc::Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub items: Vec<PageItem>,
    /// Boxes of the page (background and border drawn under the text).
    pub frames: Vec<FrameRect>,
}

/// Part of a box on a page, in pixels from the top-left corner of the
/// text area. `open_top` / `open_bottom`: the frame comes from the previous
/// page or continues onto the next (no edge or rounded corners there).
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRect {
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
    pub open_top: bool,
    pub open_bottom: bool,
    pub border: f32,
    pub border_gray: u8,
    pub sides: u8,
    pub background: Option<u8>,
    pub radius: f32,
}

/// Box being paginated.
struct OpenFrame {
    style: signet_epub::FrameStyle,
    /// Reading size (the frame's lengths are in em of this size).
    px: f32,
    y0: f32,
    open_top: bool,
    /// Some of the frame's content has already been placed on the current page.
    placed: bool,
}

impl OpenFrame {
    fn len(&self, l: Option<signet_epub::Len>, width: f32) -> f32 {
        l.map(|l| l.px(self.px, width)).unwrap_or(0.0)
    }
    fn rect(&self, width: f32, y1: f32, open_bottom: bool) -> FrameRect {
        let f = &self.style;
        let x0 = self.len(f.left, width);
        let x1 = width - self.len(f.right, width);
        FrameRect {
            x0,
            x1: f
                .width
                .map(|w| (x0 + w.px(self.px, width)).min(x1))
                .unwrap_or(x1),
            y0: self.y0,
            y1,
            open_top: self.open_top,
            open_bottom,
            border: self.len(f.border, width),
            border_gray: f.border_gray,
            sides: f.sides,
            // A dark background is painted too: its text is then written in white.
            background: f.background,
            radius: self.len(f.radius, width),
        }
    }
}

/// Page break: an open box is closed (bottom edge open) on the page that
/// ends if it already has content, and resumes at the top of the next one. As
/// in Apple Books (`box-decoration-break: slice`), the continuation has neither edge nor
/// inner space at the top: the text resumes flush with the text area. A
/// box with nothing placed yet restarts whole on the next page.
fn break_page(
    pages: &mut Vec<Page>,
    y: &mut f32,
    first: &mut bool,
    open: &mut Option<OpenFrame>,
    width: f32,
    height: f32,
) {
    let mut continued = false;
    if let Some(o) = open.as_mut() {
        if o.placed {
            let r = o.rect(width, height, true);
            pages.last_mut().unwrap().frames.push(r);
            o.open_top = true;
            o.placed = false;
            continued = true;
        }
    }
    pages.push(Page::default());
    *y = 0.0;
    *first = true;
    if let Some(o) = open.as_mut() {
        o.y0 = 0.0;
        if !continued && !o.open_top {
            *y = o.len(o.style.pad_top, width);
        }
    }
}

/// Typesetting parameters of a block according to its nature, then according to what
/// the book's stylesheet says about it (alignment, indent, margins, size).
fn style_for(
    block: &Block,
    cfg: &PageConfig,
) -> (ParagraphStyle, f32, f32, f32, f32, (bool, bool)) {
    // (style, space before, space after, x indent, right margin, (before, after) coming from the stylesheet)
    let (mut st, before, after, mut x_off) = base_style_for(block, cfg);
    let css = block.style();
    let width = cfg.text_width();
    if let Some(f) = css.font_size {
        // Size relative to the reading size, headings included (an h2 at 1.4 em
        // from the stylesheet wins over our default 1.2).
        st.px = (cfg.px * f).clamp(cfg.px * 0.6, cfg.px * 1.8);
    }
    // Line height from the stylesheet: for headings and display text (larger
    // or centered), like « MAKE / TIME » at 1 em. Body text keeps
    // the line height chosen in the settings.
    if let Some(lh) = css.line_height {
        let display = matches!(block, Block::Heading { .. })
            || css.font_size.unwrap_or(1.0) > 1.05
            || css.align == Some(signet_epub::Align::Center);
        if display {
            st.line_height = lh.clamp(0.9, 2.0);
        }
    }
    if let Some(a) = css.align {
        st.align = match a {
            signet_epub::Align::Left => Align::Left,
            signet_epub::Align::Right => Align::Right,
            signet_epub::Align::Center => Align::Center,
            signet_epub::Align::Justify => Align::Justify,
        };
    }
    // Vertical margins from the stylesheet: space before / after, capped at three lines.
    let (mut before, mut after) = (before, after);
    let mut from_css = (false, false);
    if let Some(m) = css.margin_top {
        before = m.px(st.px, width).clamp(0.0, cfg.px * 3.0);
        from_css.0 = true;
    }
    if let Some(m) = css.margin_bottom {
        after = m.px(st.px, width).clamp(0.0, cfg.px * 3.0).max(
            if matches!(block, Block::Paragraph { .. }) {
                cfg.px * 0.2
            } else {
                0.0
            },
        );
        from_css.1 = true;
    }
    // Side indents: in em of the reading size (accumulated over containers).
    let mut right = 0.0;
    if let Some(m) = css.margin_left {
        x_off += m.px(cfg.px, width).clamp(0.0, width * 0.4);
    }
    if let Some(m) = css.margin_right {
        right = m.px(cfg.px, width).clamp(0.0, width * 0.4);
    }
    if let Some(i) = css.indent {
        // Negative indent (hanging-number list): never beyond the left margin.
        st.indent = i.px(st.px, width).clamp(-x_off, width * 0.4);
    }
    (st, before, after, x_off, right, from_css)
}

fn base_style_for(block: &Block, cfg: &PageConfig) -> (ParagraphStyle, f32, f32, f32) {
    let lh = cfg.line_height;
    match block {
        Block::Heading { level, .. } => {
            let px = match level {
                1 => cfg.px * 1.35,
                2 => cfg.px * 1.2,
                _ => cfg.px * 1.05,
            };
            (
                ParagraphStyle {
                    style: Style::Bold,
                    px,
                    line_height: lh * 0.95,
                    align: if *level <= 2 {
                        Align::Center
                    } else {
                        Align::Left
                    },
                    indent: 0.0,
                },
                cfg.px * lh * if *level <= 2 { 1.5 } else { 1.0 },
                cfg.px * lh * 0.6,
                0.0,
            )
        }
        Block::Quote { .. } => (
            ParagraphStyle {
                style: Style::Italic,
                px: cfg.px,
                line_height: lh,
                align: Align::Left,
                indent: 0.0,
            },
            cfg.px * 0.4,
            cfg.px * 0.4,
            cfg.px * 1.2,
        ),
        Block::Paragraph { .. } => (
            ParagraphStyle {
                style: Style::Regular,
                px: cfg.px,
                line_height: lh,
                align: Align::Justify,
                indent: cfg.px * 1.2,
            },
            0.0,
            cfg.px * 0.35,
            0.0,
        ),
        Block::Image { .. } => (
            ParagraphStyle {
                style: Style::Italic,
                px: cfg.px * 0.85,
                line_height: lh,
                align: Align::Center,
                indent: 0.0,
            },
            cfg.px * 0.5,
            cfg.px * 0.5,
            0.0,
        ),
        Block::Separator => (
            ParagraphStyle {
                style: Style::Regular,
                px: cfg.px,
                line_height: lh,
                align: Align::Center,
                indent: 0.0,
            },
            cfg.px * 0.5,
            cfg.px * 0.5,
            0.0,
        ),
    }
}

fn spans_of(block: &Block) -> Vec<(&str, signet_text::Fmt)> {
    let spans = match block {
        Block::Heading { spans, .. }
        | Block::Paragraph { spans, .. }
        | Block::Quote { spans, .. } => spans,
        _ => return Vec::new(),
    };
    spans
        .iter()
        .map(|s| {
            let st = match (s.style.bold, s.style.italic) {
                (true, true) => Style::BoldItalic,
                (true, false) => Style::Bold,
                (false, true) => Style::Italic,
                (false, false) => Style::Regular,
            };
            let script = match s.style.script {
                Some(signet_epub::Script::Sup) => signet_text::Script::Sup,
                Some(signet_epub::Script::Sub) => signet_text::Script::Sub,
                None => signet_text::Script::Normal,
            };
            let mut caps = match s.style.caps {
                Some(signet_epub::CapsVariant::SmallCaps) => signet_text::Caps::SmallCaps,
                Some(signet_epub::CapsVariant::AllSmallCaps) => signet_text::Caps::AllSmallCaps,
                None => signet_text::Caps::Normal,
            };
            let mut scale = s.style.size_pct.map(|p| p as f32 / 100.0).unwrap_or(1.0);
            // Reduced capitals (« <small>XXI</small> »: a century, an acronym): the
            // right form is the drawn small cap, not the shrunken capital.
            let letters: Vec<char> = s.text.chars().filter(|c| c.is_alphabetic()).collect();
            if scale < 0.95
                && letters.len() >= 2
                && letters.iter().all(|c| c.is_uppercase())
                && script == signet_text::Script::Normal
            {
                caps = signet_text::Caps::AllSmallCaps;
                scale = 1.0;
            }
            (
                s.text.as_str(),
                signet_text::Fmt {
                    style: st,
                    script,
                    scale,
                    caps,
                },
            )
        })
        .collect()
}

/// A note for its window: block text with its italics and superscripts,
/// left-aligned, without the book's indent or paragraph formatting. (In
/// the current font family: the book's, chosen by the caller.)
pub fn layout_note(
    block: &Block,
    ts: &Typesetter,
    width: f32,
    px: f32,
) -> (ParagraphStyle, Vec<Line>) {
    let st = ParagraphStyle {
        style: Style::Regular,
        px,
        line_height: 1.35,
        align: Align::Left,
        indent: 0.0,
    };
    // Tab of a numbered note (« 1\tTexte »): a space.
    let spans: Vec<(String, signet_text::Fmt)> = spans_of(block)
        .into_iter()
        .map(|(t, f)| (t.replace('\t', " "), f))
        .collect();
    let refs: Vec<(&str, signet_text::Fmt)> = spans.iter().map(|(t, f)| (t.as_str(), *f)).collect();
    let (lines, _) = ts.layout_formatted(&refs, width, st);
    (st, lines)
}

/// The block without the note number that opens it (« 1. », « 12\t », « [3] »):
/// the window already shows it as a title.
pub fn strip_note_number(block: &Block, number: &str) -> Block {
    let text = block.text();
    let mut skip = 0usize;
    let trimmed = text.trim_start();
    skip += text.chars().count() - trimmed.chars().count();
    let mut rest = trimmed.trim_start_matches(['[', '(']);
    skip += trimmed.chars().count() - rest.chars().count();
    let digits = |c: char| {
        c.is_ascii_digit() || matches!(c, '⁰' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹')
    };
    let head: String = rest.chars().take_while(|&c| digits(c)).collect();
    if number.is_empty() || head.chars().count() != number.chars().count() {
        return block.clone();
    }
    skip += head.chars().count();
    rest = &rest[head.len()..];
    let after = rest.trim_start_matches([']', ')', '.', ' ', '\t', '\u{a0}', '\u{202f}']);
    skip += rest.chars().count() - after.chars().count();
    let mut out = block.clone();
    if let Block::Heading { spans, .. }
    | Block::Paragraph { spans, .. }
    | Block::Quote { spans, .. } = &mut out
    {
        let mut left = skip;
        for s in spans.iter_mut() {
            let n = s.text.chars().count();
            if left >= n {
                s.text.clear();
                left -= n;
            } else {
                s.text = s.text.chars().skip(left).collect();
                break;
            }
        }
        spans.retain(|s| !s.text.is_empty());
    }
    out
}

/// Layout of a text block: final style and lines.
fn layout_text_block(
    block: &Block,
    ts: &Typesetter,
    cfg: &PageConfig,
) -> (ParagraphStyle, Vec<Line>) {
    let (mut st, _, _, x_off, right, _) = style_for(block, cfg);
    let width = cfg.text_width();
    let block_width = (width - x_off - right).max(width * 0.3);
    if let Block::Heading { .. } = block {
        st.style = Style::Bold;
    }
    let spans = spans_of(block);
    let (l, _) = ts.layout_formatted(&spans, block_width, st);
    (st, l)
}

/// Height of the beginning of what follows block `index`, to keep on the same page as
/// it (a heading, a rule): two lines of the next block (all of them if it has three
/// or fewer, otherwise the widow rule would send it whole to the next page),
/// the image that follows, or a whole chain of headings. The next block typeset for
/// measuring is kept in `laid_ahead`.
fn head_of_next(
    blocks: &[Block],
    index: usize,
    ts: &Typesetter,
    cfg: &PageConfig,
    image: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
    laid_ahead: &mut Option<(usize, Vec<Line>)>,
) -> f32 {
    let height = cfg.text_height();
    let mut tail = 0.0f32;
    let mut k = index + 1;
    while let Some(next) = blocks.get(k) {
        match next {
            Block::Image { href, .. } => {
                tail += image(href)
                    .map(|(_, h)| (h as f32).min(height * 0.5))
                    .unwrap_or(0.0);
                break;
            }
            Block::Separator => break,
            Block::Heading { .. } if k < index + 4 => {
                let (nst, nbefore, nafter, _, _, _) = style_for(next, cfg);
                tail += nbefore + 2.0 * nst.px * nst.line_height + nafter;
                k += 1;
            }
            _ => {
                let (nst, nbefore, _, _, _, _) = style_for(next, cfg);
                let nl = match laid_ahead.take() {
                    Some((i, l)) if i == k => l,
                    _ => layout_text_block(next, ts, cfg).1,
                };
                let n = nl.len();
                let need = if n <= 3 { n } else { 2 };
                tail += nbefore + need as f32 * nst.px * nst.line_height;
                if k == index + 1 {
                    *laid_ahead = Some((k, nl));
                }
                break;
            }
        }
    }
    tail.min(height * 0.6)
}

/// Typesets a whole chapter into pages. `image` supplies, for an `href`,
/// the decoded image (already fitted to the text width).
pub fn paginate(
    blocks: &[Block],
    ts: &Typesetter,
    cfg: &PageConfig,
    image: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> Vec<Page> {
    // The book's text in its font; the interface then gets its own back.
    let previous = ts.set_family(cfg.family);
    let pages = paginate_in_family(blocks, ts, cfg, image);
    ts.set_family(previous);
    pages
}

fn paginate_in_family(
    blocks: &[Block],
    ts: &Typesetter,
    cfg: &PageConfig,
    image: &mut dyn FnMut(&str) -> Option<(u32, u32)>,
) -> Vec<Page> {
    let width = cfg.text_width();
    let height = cfg.text_height();
    let mut pages = vec![Page::default()];
    let mut y = 0.0f32;
    let mut first_on_page = true;
    // CSS bottom margin of the previous block: vertical margins collapse (the largest
    // wins), they do not add up.
    let mut last_css_after = 0.0f32;
    // Lines of the next block already typeset (to keep a heading with it).
    let mut laid_ahead: Option<(usize, Vec<Line>)> = None;
    // Box in progress.
    let mut open: Option<OpenFrame> = None;

    for (block_index, block) in blocks.iter().enumerate() {
        let (mut st, before, after, x_off, right, from_css) = style_for(block, cfg);
        // CSS bottom margin of what precedes (already added), with which the
        // top margin of what follows collapses.
        let mut prev_after = last_css_after;
        // Entering or leaving a box.
        let bf = block.style().frame;
        if bf.map(|f| f.id) != open.as_ref().map(|o| o.style.id) {
            if let Some(o) = open.take() {
                y += o.len(o.style.pad_bottom, width);
                if o.placed || o.open_top {
                    pages
                        .last_mut()
                        .unwrap()
                        .frames
                        .push(o.rect(width, y.min(height), false));
                }
                let mb = o.len(o.style.margin_bottom, width).min(cfg.px * 3.0);
                y += mb;
                prev_after = mb;
            }
            if let Some(f) = bf {
                let mut o = OpenFrame {
                    style: f,
                    px: cfg.px,
                    y0: 0.0,
                    open_top: false,
                    placed: false,
                };
                if !first_on_page {
                    y += (o.len(f.margin_top, width).min(cfg.px * 3.0) - prev_after).max(0.0);
                } else if pages.len() == 1 {
                    // Start of chapter: the top margin is kept (see `gap`).
                    y += o.len(f.margin_top, width).min(height * 0.4);
                }
                o.y0 = y;
                y += o.len(f.pad_top, width);
                open = Some(o);
                // The border and inner space separate the margins: no collapsing.
                prev_after = 0.0;
            }
        }
        let before = if from_css.0 {
            (before - prev_after).max(0.0)
        } else {
            before
        };
        last_css_after = if from_css.1 { after } else { 0.0 };
        let block_width = (width - x_off - right).max(width * 0.3);
        if let Block::Heading { .. } = block {
            // Headings are bold; an all-caps chapter heading
            // stays readable thanks to Literata's natural letter-spacing.
            st.style = Style::Bold;
        }
        let (lines, mut kind, mut line_h) = match block {
            Block::Separator => (Vec::new(), ItemKind::Separator, cfg.px * 1.0),
            Block::Image { alt, href, .. } => match image(href) {
                Some((w, h)) => {
                    // In a box, the image fits within its inner width.
                    let (w, h) = if w as f32 > block_width + 0.5 {
                        crate::draw::fit_size(w, h, 1.0, block_width as u32, height as u32)
                    } else {
                        (w, h)
                    };
                    // An image is a single-line "block" of its height.
                    (
                        Vec::new(),
                        ItemKind::Image(std::rc::Rc::new(PageImage::new(href, w, h))),
                        (h as f32).min(height),
                    )
                }
                None => {
                    let label = if alt.trim().is_empty() {
                        "[illustration]".to_string()
                    } else {
                        format!("[{alt}]")
                    };
                    let (l, _) = ts.layout(&label, block_width, st);
                    (
                        l,
                        ItemKind::ImagePlaceholder { alt: alt.clone() },
                        st.px * st.line_height,
                    )
                }
            },
            _ => {
                let l = match laid_ahead.take() {
                    Some((i, l)) if i == block_index => l,
                    _ => layout_text_block(block, ts, cfg).1,
                };
                (l, ItemKind::Text, st.px * st.line_height)
            }
        };
        if lines.is_empty() && kind == ItemKind::Text {
            // Empty block of a box (a rule): only its frame is drawn. Like a
            // heading, it does not end a page: the notes rule stays with them.
            if let Some(o) = open.as_ref() {
                if !first_on_page {
                    let rest = o.len(o.style.pad_bottom, width)
                        + o.len(o.style.margin_bottom, width).min(cfg.px * 3.0);
                    if y + rest + head_of_next(blocks, block_index, ts, cfg, image, &mut laid_ahead)
                        > height
                    {
                        break_page(
                            &mut pages,
                            &mut y,
                            &mut first_on_page,
                            &mut open,
                            width,
                            height,
                        );
                    }
                }
            }
            if let Some(o) = open.as_mut() {
                o.placed = true;
            }
            continue;
        }
        // Image that doesn't fit in the remaining space but would fit at three
        // quarters of its size or more: we shrink it rather than leave a large
        // blank and send it to the next page.
        // Space before the block. At the top of a page, a margin disappears (as at a
        // column break in WebKit); but at the start of the chapter, with nothing
        // placed yet, the stylesheet's top margin is kept, as in Apple
        // Books: chapter headings lower, dedication and part pages at one
        // third of the height (`margin-top: 30%`).
        let start_gap = block
            .style()
            .margin_top
            .map(|m| m.px(st.px, width).clamp(0.0, height * 0.4))
            .unwrap_or(0.0);
        // Image with a negative top margin (`.fig1 { margin-top: -2.2em }`): it moves up
        // by that much, over the edge of the box it decorates, never above
        // the page.
        let lift = match kind {
            ItemKind::Image(_) => block
                .style()
                .margin_top
                .map(|m| -m.px(st.px, width))
                .filter(|l| *l > 0.0)
                .map(|l| l.min(cfg.px * 3.0))
                .unwrap_or(0.0),
            _ => 0.0,
        };
        let gap = |first: bool, pages_len: usize| {
            if !first {
                before
            } else if pages_len == 1 {
                start_gap
            } else {
                0.0
            }
        };
        if let ItemKind::Image(g) = &kind {
            let space_before = gap(first_on_page, pages.len());
            let avail = (height - (y + space_before)).floor();
            let gh = g.height as f32;
            if !first_on_page && avail < gh && avail >= gh * 0.75 {
                let (w, h) = crate::draw::fit_size(
                    g.width,
                    g.height,
                    avail / gh,
                    width as u32,
                    avail as u32,
                );
                line_h = h as f32;
                kind = ItemKind::Image(std::rc::Rc::new(PageImage::new(&g.href, w, h)));
            }
        }
        let n_lines = match kind {
            ItemKind::Separator | ItemKind::Image(_) => 1,
            _ => lines.len(),
        };
        // For a heading: height of the start of the next block to keep with it.
        // Headings that follow each other (numbered steps tagged as headings) form a
        // chain: the whole chain and the start of the first regular block are kept together.
        let heading_tail: Option<f32> = if matches!(block, Block::Heading { .. }) {
            Some(head_of_next(
                blocks,
                block_index,
                ts,
                cfg,
                image,
                &mut laid_ahead,
            ))
        } else {
            None
        };
        // Does the block fit entirely? Otherwise we split it line by line.
        let mut start = 0;
        while start < n_lines {
            // Recomputed on every iteration: after a page break, no more space before.
            let space_before = gap(first_on_page, pages.len());
            let avail = height - (y + space_before);
            // On a blank page, at least one line (or the image) fits, no matter
            // what: otherwise an image of exactly the page height would
            // skip pages endlessly.
            let fit =
                ((avail / line_h).floor().max(0.0) as usize).max(if first_on_page { 1 } else { 0 });
            let remaining = n_lines - start;
            // A heading never ends a page: it needs, on the same page, the
            // start of the block that follows it (two lines, or the whole image).
            let keep_with_next = if start == 0 && remaining <= fit && !first_on_page {
                heading_tail.unwrap_or(0.0)
            } else {
                0.0
            };
            if keep_with_next > 0.0
                && y + space_before + remaining as f32 * line_h + after + keep_with_next > height
            {
                break_page(
                    &mut pages,
                    &mut y,
                    &mut first_on_page,
                    &mut open,
                    width,
                    height,
                );
                continue;
            }
            let mut take = remaining.min(fit);
            // A heading is not split across pages: if it doesn't fit
            // entirely, it moves to the next page.
            if take < remaining && !first_on_page && matches!(block, Block::Heading { .. }) {
                break_page(
                    &mut pages,
                    &mut y,
                    &mut first_on_page,
                    &mut open,
                    width,
                    height,
                );
                continue;
            }
            if take < remaining {
                // Widow: at least two lines of the paragraph at the top of the next page
                // (widows: 2, as in Apple Books).
                if remaining - take < 2 {
                    take = remaining.saturating_sub(2);
                }
                // Orphan: at least two lines at the bottom of the page (orphans: 2), otherwise
                // the whole paragraph moves to the next page.
                if take < 2 && !first_on_page {
                    break_page(
                        &mut pages,
                        &mut y,
                        &mut first_on_page,
                        &mut open,
                        width,
                        height,
                    );
                    continue;
                }
                if take == 0 {
                    // Blank page too short for both rules: we take what fits.
                    take = fit.max(1).min(remaining);
                }
            }
            let y0 = (y + space_before - lift).max(0.0);
            let item_lines: Vec<Line> = if matches!(kind, ItemKind::Separator | ItemKind::Image(_))
            {
                Vec::new()
            } else {
                lines[start..start + take]
                    .iter()
                    .map(|l| Line {
                        glyphs: l
                            .glyphs
                            .iter()
                            .map(|g| signet_text::PositionedGlyph {
                                id: g.id,
                                x: g.x,
                                y: g.y - start as f32 * line_h,
                                style: g.style,
                                ch: g.ch,
                                w: g.w,
                                scale: g.scale,
                            })
                            .collect(),
                        width: l.width,
                        ratio: l.ratio,
                        hyphenated: l.hyphenated,
                    })
                    .collect()
            };
            pages.last_mut().unwrap().items.push(PageItem {
                y: y0,
                lines: item_lines,
                style: st,
                x: x_off,
                kind: kind.clone(),
                block: block_index,
                width: block_width,
            });
            if let Some(o) = open.as_mut() {
                o.placed = true;
            }
            y = y0 + take as f32 * line_h;
            start += take;
            first_on_page = false;
            if start < n_lines {
                break_page(
                    &mut pages,
                    &mut y,
                    &mut first_on_page,
                    &mut open,
                    width,
                    height,
                );
            }
        }
        y += after;
    }
    if let Some(o) = open.take() {
        y += o.len(o.style.pad_bottom, width);
        if o.placed || o.open_top {
            pages
                .last_mut()
                .unwrap()
                .frames
                .push(o.rect(width, y.min(height), false));
        }
    }
    if pages.len() > 1 && pages.last().map(|p| p.items.is_empty()).unwrap_or(false) {
        pages.pop();
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use signet_epub::{BlockStyle, Span, SpanStyle};

    fn para(text: String) -> Block {
        Block::Paragraph {
            spans: vec![Span {
                text,
                style: SpanStyle::default(),
            }],
            style: BlockStyle::default(),
        }
    }

    /// Apple Books' layout rules (widows, orphans) and those of any
    /// typographer (a heading does not end a page) hold over a long chapter
    /// with paragraphs of varied lengths.
    #[test]
    fn widows_orphans_and_headings() {
        let ts = crate::typesetter();
        let cfg = PageConfig {
            px: 30.0,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        let vocab = [
            "lorem",
            "ipsum",
            "méthode",
            "concentration",
            "journée",
            "temps",
            "priorité",
            "et",
            "de",
            "la",
        ];
        let mut blocks = Vec::new();
        for i in 0..120usize {
            if i % 9 == 4 {
                // One heading in three is long (three or four lines, like the titles
                // of works in a bibliography): it must not be split either.
                let long = if i % 27 == 13 {
                    " : méthode de concentration et de priorité pour la journée de travail et la vie de famille"
                } else {
                    ""
                };
                blocks.push(Block::Heading {
                    level: 3,
                    spans: vec![Span {
                        text: format!("Titre numéro {i}{long}"),
                        style: SpanStyle::default(),
                    }],
                    style: BlockStyle::default(),
                });
                if i % 18 == 4 {
                    blocks.push(Block::Heading {
                        level: 3,
                        spans: vec![Span {
                            text: format!("Sous-titre {i}"),
                            style: SpanStyle::default(),
                        }],
                        style: BlockStyle::default(),
                    });
                }
            }
            let words = (i * 37) % 70 + 3;
            blocks.push(para(
                (0..words)
                    .map(|w| vocab[(w * 7 + i) % vocab.len()])
                    .collect::<Vec<_>>()
                    .join(" "),
            ));
        }
        let pages = paginate(&blocks, &ts, &cfg, &mut |_| None);
        assert!(pages.len() > 10);
        for (pi, page) in pages.iter().enumerate() {
            let last = page.items.last().expect("page vide");
            if pi + 1 < pages.len() {
                assert!(
                    !matches!(blocks[last.block], Block::Heading { .. }),
                    "page {pi} : titre en bas de page"
                );
                let next = pages[pi + 1].items.first().unwrap();
                if next.block == last.block {
                    assert!(
                        last.lines.len() >= 2,
                        "page {pi} : orpheline ({} ligne)",
                        last.lines.len()
                    );
                    assert!(
                        next.lines.len() >= 2,
                        "page {} : veuve ({} ligne)",
                        pi + 1,
                        next.lines.len()
                    );
                }
            }
        }
    }

    /// `.box { font-size: .75em; margin: 1em 0; padding: .5em 1em 1em 1.5em;
    /// border: #BCBDC0 solid 2px; border-radius: 1em; background-color: #ECEDEE }`,
    /// lengths converted to em of the reading size (× `k`).
    fn boxed_style(k: f32) -> BlockStyle {
        use signet_epub::{FrameStyle, Len};
        let frame = FrameStyle {
            id: 1,
            left: Some(Len::Em(0.0)),
            right: Some(Len::Em(0.0)),
            margin_top: Some(Len::Em(k)),
            margin_bottom: Some(Len::Em(k)),
            pad_top: Some(Len::Em(0.625 * k)),
            pad_bottom: Some(Len::Em(1.125 * k)),
            border: Some(Len::Em(0.125 * k)),
            border_gray: 189,
            sides: 15,
            background: Some(237),
            radius: Some(Len::Em(k)),
            width: None,
        };
        BlockStyle {
            margin_left: Some(Len::Em(1.625 * k)),
            margin_right: Some(Len::Em(1.125 * k)),
            font_size: Some(k),
            frame: Some(frame),
            ..BlockStyle::default()
        }
    }

    fn words(n: usize) -> String {
        (0..n)
            .map(|i| ["temps", "méthode", "journée", "priorité", "et", "la"][(i * 5) % 6])
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A box that spans two pages: bottom edge open on the first
    /// page, top edge open on the next, where the text resumes flush with the text
    /// area (as in Apple Books); the text is indented from the frame, and the
    /// frame measured in the size of its own text.
    #[test]
    fn frame_across_pages() {
        let ts = crate::typesetter();
        let cfg = PageConfig {
            px: 30.0,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        let boxed = boxed_style(0.75);
        let mut blocks = vec![para(words(60))];
        for _ in 0..6 {
            blocks.push(Block::Paragraph {
                spans: vec![Span {
                    text: words(80),
                    style: SpanStyle::default(),
                }],
                style: boxed,
            });
        }
        blocks.push(para(words(30)));
        let pages = paginate(&blocks, &ts, &cfg, &mut |_| None);
        let framed: Vec<(usize, &FrameRect)> = pages
            .iter()
            .enumerate()
            .flat_map(|(i, p)| p.frames.iter().map(move |f| (i, f)))
            .collect();
        assert!(framed.len() >= 2, "{framed:?}");
        let (first, last) = (framed[0].1, framed[framed.len() - 1].1);
        assert!(!first.open_top && first.open_bottom, "{first:?}");
        assert!(last.open_top && !last.open_bottom, "{last:?}");
        assert_eq!(first.y1, cfg.text_height());
        let px = 30.0 * 0.75;
        for (_, f) in &framed {
            assert_eq!((f.x0, f.x1), (0.0, cfg.text_width()));
            assert_eq!((f.background, f.border_gray, f.sides), (Some(237), 189, 15));
            assert!(
                (f.radius - px).abs() < 0.01 && (f.border - 0.125 * px).abs() < 0.01,
                "{f:?}"
            );
        }
        // The rest of the box resumes at the top of the page, without inner space.
        let (p2, cont) = framed[1];
        assert_eq!(cont.y0, 0.0);
        assert_eq!(pages[p2].items[0].y, 0.0);
        // The first boxed block is below the frame's margin and its inner space.
        let first_boxed = pages[framed[0].0]
            .items
            .iter()
            .find(|it| it.block == 1)
            .unwrap();
        assert!(
            (first_boxed.y - (first.y0 + 0.625 * px)).abs() < 0.01,
            "{} {}",
            first_boxed.y,
            first.y0
        );
        for p in &pages {
            for it in &p.items {
                if blocks[it.block].style().frame.is_some() {
                    assert!((it.x - 1.625 * px).abs() < 0.01, "retrait {}", it.x);
                    assert!(it.y + it.lines.len() as f32 * px * 1.45 <= cfg.text_height() + 0.5);
                }
            }
        }
        // The following paragraph is below the closed frame and its margin, or at the top of
        // the next page.
        let last_page = framed[framed.len() - 1].0;
        let (ap, after) = pages
            .iter()
            .enumerate()
            .find_map(|(i, p)| p.items.iter().find(|it| it.block == 7).map(|it| (i, it)))
            .unwrap();
        if ap == last_page {
            assert!(after.y >= last.y1 + px - 0.01, "{} {}", after.y, last.y1);
        } else {
            assert_eq!((ap, after.y), (last_page + 1, 0.0));
        }
    }

    /// Empty paragraph with a top border (the rule above the notes): a frame of
    /// the requested width, without text.
    #[test]
    fn empty_bordered_block_is_a_rule() {
        use signet_epub::{FrameStyle, Len};
        let ts = crate::typesetter();
        let cfg = PageConfig {
            px: 30.0,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        let rule = FrameStyle {
            id: 3,
            left: None,
            right: None,
            margin_top: Some(Len::Em(2.0)),
            margin_bottom: Some(Len::Em(0.3)),
            pad_top: Some(Len::Em(1.0 / 16.0)),
            pad_bottom: None,
            border: Some(Len::Em(1.0 / 16.0)),
            border_gray: 0,
            sides: 1,
            background: None,
            radius: None,
            width: Some(Len::Em(7.0)),
        };
        let blocks = vec![
            para(words(20)),
            Block::Paragraph {
                spans: Vec::new(),
                style: BlockStyle {
                    frame: Some(rule),
                    ..BlockStyle::default()
                },
            },
            para(words(20)),
        ];
        let pages = paginate(&blocks, &ts, &cfg, &mut |_| None);
        assert_eq!(pages.len(), 1);
        let f = &pages[0].frames;
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            (
                f[0].x0,
                f[0].x1,
                f[0].sides,
                f[0].open_top,
                f[0].open_bottom
            ),
            (0.0, 210.0, 1, false, false)
        );
        assert!((f[0].y1 - f[0].y0 - 30.0 / 16.0).abs() < 0.01, "{:?}", f[0]);
        let next = pages[0].items.iter().find(|it| it.block == 2).unwrap();
        assert!(next.y >= f[0].y1, "{} {}", next.y, f[0].y1);
        // The rule never ends a page: whatever the length of the text
        // that precedes it, it is on the page where the notes begin.
        for n in (20..200).step_by(3) {
            let blocks = vec![
                para(words(n)),
                Block::Paragraph {
                    spans: Vec::new(),
                    style: BlockStyle {
                        frame: Some(rule),
                        ..BlockStyle::default()
                    },
                },
                para(words(40)),
            ];
            let pages = paginate(&blocks, &ts, &cfg, &mut |_| None);
            let rule_page = pages.iter().position(|p| !p.frames.is_empty()).unwrap();
            let note_page = pages
                .iter()
                .position(|p| p.items.iter().any(|it| it.block == 2))
                .unwrap();
            assert_eq!(rule_page, note_page, "{n} mots");
        }
    }

    /// A multi-line heading (a work's reference in a bibliography)
    /// is never split across pages, and stays with the start of the block that
    /// follows it, whatever height the preceding text brings it to.
    #[test]
    fn long_heading_never_split() {
        let ts = crate::typesetter();
        let cfg = PageConfig {
            px: 30.0,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        // At least four lines: with three, widows and orphans are enough to prevent it.
        let title = "S’organiser pour réussir, la méthode GTD ou l’art de l’efficacité sans le stress, David Allen, traduction de l’américain, Alisio, Paris, 2015, édition revue";
        for n in (10..260).step_by(3) {
            let blocks = vec![
                para(words(n)),
                Block::Heading {
                    level: 3,
                    spans: vec![Span {
                        text: title.into(),
                        style: SpanStyle::default(),
                    }],
                    style: BlockStyle::default(),
                },
                para(words(60)),
            ];
            let pages = paginate(&blocks, &ts, &cfg, &mut |_| None);
            let lines: usize = pages
                .iter()
                .flat_map(|p| p.items.iter())
                .filter(|it| it.block == 1)
                .map(|it| it.lines.len())
                .sum();
            assert!(lines >= 4, "{lines} lignes");
            let parts: Vec<usize> = pages
                .iter()
                .enumerate()
                .filter(|(_, p)| p.items.iter().any(|it| it.block == 1))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(
                parts.len(),
                1,
                "{n} mots : titre coupé sur les pages {parts:?}"
            );
            assert!(
                pages[parts[0]].items.iter().any(|it| it.block == 2),
                "{n} mots : titre séparé de son texte"
            );
        }
    }
}
