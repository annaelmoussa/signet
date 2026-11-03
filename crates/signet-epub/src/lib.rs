//! `signet-epub` — open an EPUB and extract structured text from it.
//!
//! An EPUB is a zip containing, in the order we read them:
//!
//! 1. `META-INF/container.xml` → path of the OPF file;
//! 2. the **OPF**: metadata (title, author), the `manifest` (all files)
//!    and the `spine` (the reading order of the chapters);
//! 3. the table of contents: `nav.xhtml` (EPUB 3) or `toc.ncx` (EPUB 2);
//! 4. the XHTML chapters, which we reduce to a list of **blocks** (headings,
//!    paragraphs, images) made of **spans** (text + italic/bold).
//!
//! We deliberately ignore the CSS: the reader imposes its own typography,
//! which is the whole point of a device "built for reading". Only the markup
//! semantics (p, h1…h6, em, strong, img, br…) are kept.

pub mod css;
pub use css::{Align, CapsVariant, Len, Script, Sheet};
use roxmltree::{Document, Node, NodeType, ParsingOptions};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use zip::ZipArchive;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: String,
    pub author: String,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineItem {
    pub id: String,
    /// Path in the zip (already resolved relative to the OPF).
    pub href: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub label: String,
    /// Path in the zip, without the `#…` fragment.
    pub href: String,
    pub fragment: Option<String>,
    pub depth: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SpanStyle {
    pub italic: bool,
    pub bold: bool,
    /// Superscript or subscript (letters: set small and raised; digits
    /// are already converted to ¹²³ by the parser).
    pub script: Option<Script>,
    /// Relative size of inline text (`<small>`, `font-size` of a
    /// segment), in percent; `None` = paragraph size.
    pub size_pct: Option<u8>,
    /// Small caps (`font-variant`).
    pub caps: Option<CapsVariant>,
}

/// Formatting of an inline element: relative size (`<small>`, `font-size`)
/// and small caps, accumulated with those of the parents. Superscripts and
/// subscripts already have their own reduction.
fn inline_style(mut s: SpanStyle, d: &css::Decl, tag: &str) -> SpanStyle {
    if s.script.is_none() && !matches!(tag, "sup" | "sub") {
        let f = d
            .font_size
            .or(if tag == "small" { Some(0.83) } else { None });
        if let Some(f) = f {
            if (f - 1.0).abs() > 0.04 {
                let cur = s.size_pct.unwrap_or(100) as f32 / 100.0;
                s.size_pct = Some(((cur * f).clamp(0.5, 1.5) * 100.0).round() as u8);
            }
        }
    }
    if let Some(c) = d.caps {
        s.caps = c;
    }
    s
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: SpanStyle,
}

/// Frame of a box (`border`, `background-color` on a container or a block).
/// All the blocks of the frame carry the same `id`; the paginator draws the frame
/// around them, across several pages if needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStyle {
    pub id: u16,
    /// All frame lengths are in em of the reading size.
    /// Outer edges of the frame (accumulated container margins), from the column.
    pub left: Option<Len>,
    pub right: Option<Len>,
    /// Vertical margins of the frame, and top / bottom inner space (border included).
    pub margin_top: Option<Len>,
    pub margin_bottom: Option<Len>,
    pub pad_top: Option<Len>,
    pub pad_bottom: Option<Len>,
    pub border: Option<Len>,
    /// Border gray level (0 = black) and sides (1 top, 2 right, 4 bottom, 8 left).
    pub border_gray: u8,
    pub sides: u8,
    pub background: Option<u8>,
    pub radius: Option<Len>,
    /// Width of the frame if it has one (otherwise all the room between `left` and `right`).
    pub width: Option<Len>,
}

/// What the stylesheet says about a block (nothing = reader values).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BlockStyle {
    pub align: Option<Align>,
    /// First-line indent, in em of the block itself.
    pub indent: Option<Len>,
    /// Accumulated left and right indents (margins, borders and inner spacing
    /// of the containers and of the block), in em of the reading size: the em of each
    /// element are converted to its own text size.
    pub margin_left: Option<Len>,
    pub margin_right: Option<Len>,
    /// Vertical margins of the block itself (not inherited from containers), in em of the block.
    pub margin_top: Option<Len>,
    pub margin_bottom: Option<Len>,
    /// Relative size (1.0 = reading size).
    pub font_size: Option<f32>,
    /// Line height requested by the stylesheet (multiple of the text size).
    pub line_height: Option<f32>,
    /// Box the block belongs to.
    pub frame: Option<FrameStyle>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
        style: BlockStyle,
    },
    Paragraph {
        spans: Vec<Span>,
        style: BlockStyle,
    },
    Quote {
        spans: Vec<Span>,
        style: BlockStyle,
    },
    Image {
        href: String,
        alt: String,
        style: BlockStyle,
    },
    Separator,
}

/// Link in the text (`<a href>`): character range of a block and its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub block: usize,
    /// Range [from, to) in the block text (`u32::MAX`: up to the end of the block).
    pub from: u32,
    pub to: u32,
    /// Target document, path in the zip (the document itself for `#…`).
    pub href: String,
    pub fragment: Option<String>,
    /// Address outside the book (http, mailto…).
    pub external: bool,
    /// Looks like a footnote call: a digit (superscript or in brackets),
    /// an asterisk, a superscript letter, or `epub:type="noteref"`.
    pub note: bool,
    /// Link text ("¹", "p. 178"), if it fits in a block.
    pub label: String,
}

/// Identified element (`id="…"`): its start in the chapter, and the block following
/// its end (a multi-paragraph note in an `<aside>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub block: usize,
    pub ch: u32,
    pub end: usize,
}

/// A parsed chapter: blocks, links and identified elements.
#[derive(Debug, Clone, Default)]
pub struct Chapter {
    pub blocks: Vec<Block>,
    pub links: Vec<Link>,
    pub anchors: HashMap<String, Anchor>,
}

/// Is a link label that of a footnote call? "1", "¹²", "[3]",
/// "(4)", "*", "†", or a superscript letter ("a").
fn is_note_label(label: &str, sup: bool) -> bool {
    let t = label.trim();
    let core = t.trim_matches(|c| matches!(c, '[' | ']' | '(' | ')'));
    let n = core.chars().count();
    if n == 0 {
        return false;
    }
    let digit = |c: char| {
        c.is_ascii_digit() || matches!(c, '⁰' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹')
    };
    (n <= 4 && core.chars().all(digit))
        || (n <= 3 && core.chars().all(|c| matches!(c, '*' | '†' | '‡' | '§')))
        || (sup && t.chars().count() <= 3)
}

impl Block {
    pub fn text(&self) -> String {
        match self {
            Block::Heading { spans, .. }
            | Block::Paragraph { spans, .. }
            | Block::Quote { spans, .. } => spans.iter().map(|s| s.text.as_str()).collect(),
            Block::Image { alt, .. } => alt.clone(),
            Block::Separator => String::new(),
        }
    }
    /// CSS style of the block (empty for an image or a separator).
    pub fn style(&self) -> BlockStyle {
        match self {
            Block::Heading { style, .. }
            | Block::Paragraph { style, .. }
            | Block::Quote { style, .. }
            | Block::Image { style, .. } => *style,
            _ => BlockStyle::default(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Zip(zip::result::ZipError),
    Xml(roxmltree::Error),
    Format(&'static str),
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<zip::result::ZipError> for Error {
    fn from(e: zip::result::ZipError) -> Self {
        Error::Zip(e)
    }
}
impl From<roxmltree::Error> for Error {
    fn from(e: roxmltree::Error) -> Self {
        Error::Xml(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "E/S : {e}"),
            Error::Zip(e) => write!(f, "zip : {e}"),
            Error::Xml(e) => write!(f, "XML : {e}"),
            Error::Format(s) => write!(f, "format : {s}"),
        }
    }
}

pub struct Book {
    archive: ZipArchive<File>,
    pub metadata: Metadata,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocEntry>,
    pub cover_href: Option<String>,
    /// Stylesheets already read, by path in the zip.
    sheets: HashMap<String, std::rc::Rc<Sheet>>,
}

impl Book {
    pub fn open(path: impl AsRef<Path>) -> Result<Book, Error> {
        let file = File::open(path)?;
        let mut archive = ZipArchive::new(file)?;

        // 1. container.xml → OPF
        let container = read_string(&mut archive, "META-INF/container.xml")?;
        let doc = Document::parse(&container)?;
        let opf_path = doc
            .descendants()
            .find(|n| n.has_tag_name("rootfile"))
            .and_then(|n| n.attribute("full-path"))
            .ok_or(Error::Format("container.xml sans rootfile"))?
            .to_string();
        let opf_dir = parent_dir(&opf_path);

        // 2. OPF
        let opf = read_string(&mut archive, &opf_path)?;
        let doc = Document::parse(&opf)?;
        let root = doc.root_element();

        let mut metadata = Metadata::default();
        if let Some(md) = root.children().find(|n| n.has_tag_name("metadata")) {
            for n in md.children().filter(|n| n.is_element()) {
                let v = n.text().unwrap_or("").trim();
                match n.tag_name().name() {
                    "title" if metadata.title.is_empty() => metadata.title = v.to_string(),
                    "creator" if metadata.author.is_empty() => metadata.author = v.to_string(),
                    "language" if metadata.language.is_empty() => metadata.language = v.to_string(),
                    _ => {}
                }
            }
        }

        let mut manifest: HashMap<String, (String, String, String)> = HashMap::new(); // id → (href, type, properties)
        let mut cover_id: Option<String> = None;
        if let Some(md) = root.children().find(|n| n.has_tag_name("metadata")) {
            cover_id = md
                .children()
                .find(|n| n.has_tag_name("meta") && n.attribute("name") == Some("cover"))
                .and_then(|n| n.attribute("content"))
                .map(String::from);
        }
        let mut nav_href: Option<String> = None;
        let mut ncx_href: Option<String> = None;
        let mut cover_href: Option<String> = None;
        if let Some(m) = root.children().find(|n| n.has_tag_name("manifest")) {
            for item in m.children().filter(|n| n.has_tag_name("item")) {
                let id = item.attribute("id").unwrap_or("").to_string();
                let href = join(&opf_dir, item.attribute("href").unwrap_or(""));
                let mt = item.attribute("media-type").unwrap_or("").to_string();
                let props = item.attribute("properties").unwrap_or("").to_string();
                if props.split_whitespace().any(|p| p == "nav") {
                    nav_href = Some(href.clone());
                }
                if props.split_whitespace().any(|p| p == "cover-image") {
                    cover_href = Some(href.clone());
                }
                if mt == "application/x-dtbncx+xml" {
                    ncx_href = Some(href.clone());
                }
                if cover_id.as_deref() == Some(id.as_str()) && cover_href.is_none() {
                    cover_href = Some(href.clone());
                }
                manifest.insert(id, (href, mt, props));
            }
        }

        let mut spine = Vec::new();
        if let Some(s) = root.children().find(|n| n.has_tag_name("spine")) {
            if ncx_href.is_none() {
                if let Some(toc_id) = s.attribute("toc") {
                    ncx_href = manifest.get(toc_id).map(|m| m.0.clone());
                }
            }
            for it in s.children().filter(|n| n.has_tag_name("itemref")) {
                if it.attribute("linear") == Some("no") {
                    continue;
                }
                let idref = it.attribute("idref").unwrap_or("");
                if let Some((href, mt, _)) = manifest.get(idref) {
                    spine.push(SpineItem {
                        id: idref.to_string(),
                        href: href.clone(),
                        media_type: mt.clone(),
                    });
                }
            }
        }
        if spine.is_empty() {
            return Err(Error::Format("spine vide"));
        }

        // 3. Table of contents
        let toc = if let Some(h) = nav_href.as_deref() {
            read_string(&mut archive, h)
                .ok()
                .and_then(|s| parse_nav(&s, &parent_dir(h)).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let toc = if toc.is_empty() {
            if let Some(h) = ncx_href.as_deref() {
                read_string(&mut archive, h)
                    .ok()
                    .and_then(|s| parse_ncx(&s, &parent_dir(h)).ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        } else {
            toc
        };

        Ok(Book {
            archive,
            metadata,
            spine,
            toc,
            cover_href,
            sheets: HashMap::new(),
        })
    }

    /// Reads a raw file from the container (cover image, etc.).
    pub fn read_bytes(&mut self, href: &str) -> Result<Vec<u8>, Error> {
        let mut f = self.archive.by_name(href)?;
        let mut v = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut v)?;
        Ok(v)
    }

    /// Extracts the blocks of chapter `index` (in spine order), with
    /// the stylesheets linked by the document.
    pub fn chapter(&mut self, index: usize) -> Result<Vec<Block>, Error> {
        Ok(self.chapter_full(index)?.blocks)
    }

    /// Spine index of document `href` (path in the zip).
    pub fn spine_index(&self, href: &str) -> Option<usize> {
        self.spine.iter().position(|s| s.href == href)
    }

    /// Like [`Book::chapter`], with the chapter's links and identified elements.
    pub fn chapter_full(&mut self, index: usize) -> Result<Chapter, Error> {
        let href = self
            .spine
            .get(index)
            .ok_or(Error::Format("chapitre hors spine"))?
            .href
            .clone();
        let xhtml = read_string(&mut self.archive, &href)?;
        let base = parent_dir(&href);
        let mut sheet = Sheet::default();
        for link in linked_stylesheets(&xhtml) {
            let path = join(&base, &link);
            let rc = match self.sheets.get(&path) {
                Some(rc) => rc.clone(),
                None => {
                    let rc = std::rc::Rc::new(
                        read_string(&mut self.archive, &path)
                            .map(|s| Sheet::parse(&s))
                            .unwrap_or_default(),
                    );
                    self.sheets.insert(path.clone(), rc.clone());
                    rc
                }
            };
            sheet.extend(&rc);
        }
        parse_document(&xhtml, &base, &href, sheet)
    }
}

fn read_string(archive: &mut ZipArchive<File>, name: &str) -> Result<String, Error> {
    let mut f = archive.by_name(name)?;
    let mut s = String::with_capacity(f.size() as usize);
    f.read_to_string(&mut s)?;
    Ok(s)
}

fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// Resolves `href` (relative, possibly with `../`) against the `base` folder.
fn join(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// EPUB XHTML files reference the XHTML DTD and use HTML entities
/// (`&nbsp;`, `&eacute;`…) that a strict XML parser does not know. We
/// replace them before parsing; numeric entities are handled natively.
fn normalize_entities(src: &str) -> String {
    const TABLE: &[(&str, &str)] = &[
        ("&nbsp;", "\u{a0}"),
        ("&ensp;", "\u{2002}"),
        ("&emsp;", "\u{2003}"),
        ("&thinsp;", "\u{2009}"),
        ("&shy;", "\u{ad}"),
        ("&ndash;", "–"),
        ("&mdash;", "—"),
        ("&lsquo;", "‘"),
        ("&rsquo;", "’"),
        ("&ldquo;", "“"),
        ("&rdquo;", "”"),
        ("&laquo;", "«"),
        ("&raquo;", "»"),
        ("&hellip;", "…"),
        ("&eacute;", "é"),
        ("&egrave;", "è"),
        ("&ecirc;", "ê"),
        ("&agrave;", "à"),
        ("&acirc;", "â"),
        ("&ccedil;", "ç"),
        ("&ugrave;", "ù"),
        ("&ucirc;", "û"),
        ("&icirc;", "î"),
        ("&ocirc;", "ô"),
        ("&euml;", "ë"),
        ("&iuml;", "ï"),
        ("&oelig;", "œ"),
        ("&OElig;", "Œ"),
        ("&copy;", "©"),
        ("&reg;", "®"),
        ("&trade;", "™"),
        ("&bull;", "•"),
        ("&middot;", "·"),
        ("&times;", "×"),
        ("&deg;", "°"),
        ("&euro;", "€"),
        ("&pound;", "£"),
    ];
    if !src.contains('&') {
        return src.to_string();
    }
    let mut s = src.to_string();
    for (from, to) in TABLE {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }
    s
}

fn parse_opts() -> ParsingOptions {
    ParsingOptions {
        allow_dtd: true,
        ..ParsingOptions::default()
    }
}

/// EPUB 3 : `<nav epub:type="toc"><ol><li><a href>…`
fn parse_nav(src: &str, base: &str) -> Result<Vec<TocEntry>, Error> {
    let src = normalize_entities(src);
    let doc = Document::parse_with_options(&src, parse_opts())?;
    let nav = doc
        .descendants()
        .find(|n| {
            n.has_tag_name("nav")
                && n.attributes()
                    .any(|a| a.name() == "type" && a.value() == "toc")
        })
        .or_else(|| doc.descendants().find(|n| n.has_tag_name("nav")));
    let mut out = Vec::new();
    if let Some(nav) = nav {
        if let Some(ol) = nav.children().find(|n| n.has_tag_name("ol")) {
            walk_nav_list(ol, base, 0, &mut out);
        }
    }
    Ok(out)
}

fn walk_nav_list(ol: Node, base: &str, depth: u8, out: &mut Vec<TocEntry>) {
    for li in ol.children().filter(|n| n.has_tag_name("li")) {
        if let Some(a) = li
            .children()
            .find(|n| n.has_tag_name("a") || n.has_tag_name("span"))
        {
            let label = collapse_ws(&text_of(a));
            let raw = a.attribute("href").unwrap_or("");
            let (href, fragment) = split_fragment(raw);
            out.push(TocEntry {
                label,
                href: join(base, &href),
                fragment,
                depth,
            });
        }
        if let Some(sub) = li.children().find(|n| n.has_tag_name("ol")) {
            walk_nav_list(sub, base, depth + 1, out);
        }
    }
}

/// EPUB 2 : `<navMap><navPoint><navLabel><text/></navLabel><content src/>…`
fn parse_ncx(src: &str, base: &str) -> Result<Vec<TocEntry>, Error> {
    let src = normalize_entities(src);
    let doc = Document::parse_with_options(&src, parse_opts())?;
    let mut out = Vec::new();
    if let Some(map) = doc.descendants().find(|n| n.has_tag_name("navMap")) {
        walk_ncx(map, base, 0, &mut out);
    }
    Ok(out)
}

fn walk_ncx(parent: Node, base: &str, depth: u8, out: &mut Vec<TocEntry>) {
    for np in parent.children().filter(|n| n.has_tag_name("navPoint")) {
        let label = np
            .children()
            .find(|n| n.has_tag_name("navLabel"))
            .map(|n| collapse_ws(&text_of(n)))
            .unwrap_or_default();
        let raw = np
            .children()
            .find(|n| n.has_tag_name("content"))
            .and_then(|n| n.attribute("src"))
            .unwrap_or("");
        let (href, fragment) = split_fragment(raw);
        out.push(TocEntry {
            label,
            href: join(base, &href),
            fragment,
            depth,
        });
        walk_ncx(np, base, depth + 1, out);
    }
}

fn split_fragment(raw: &str) -> (String, Option<String>) {
    match raw.split_once('#') {
        Some((h, f)) => (h.to_string(), Some(f.to_string())),
        None => (raw.to_string(), None),
    }
}

fn text_of(n: Node) -> String {
    n.descendants()
        .filter(|d| d.node_type() == NodeType::Text)
        .map(|d| d.text().unwrap_or(""))
        .collect()
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = true;
    for ch in s.chars() {
        let sp = ch.is_whitespace() && !matches!(ch, '\u{a0}' | '\u{202f}' | '\u{2007}');
        if sp {
            if !prev_space {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
        prev_space = sp;
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Span accumulator for the current block.
struct BlockBuilder {
    spans: Vec<Span>,
    pending_space: bool,
    /// Current superscript / subscript: digits become ⁰¹²³ / ₀₁₂₃
    /// (Literata has them), without changing the typesetting size.
    script: Option<Script>,
    /// Characters already laid down (position of anchors and links).
    chars: u32,
}

/// Unicode superscript or subscript digit, otherwise the character unchanged. Only
/// the characters Literata draws are converted (digits, parentheses):
/// an "ᵉ" missing from the font would show as a square ("XXI□"). Superscript
/// letters go through the span style (`SpanStyle::sup`), set small and
/// raised.
fn script_char(c: char, script: Script) -> char {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    const SUB: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    match c.to_digit(10) {
        Some(d) => match script {
            Script::Sup => SUP[d as usize],
            Script::Sub => SUB[d as usize],
        },
        None => match (script, c) {
            (Script::Sup, '(') => '⁽',
            (Script::Sup, ')') => '⁾',
            (Script::Sub, '(') => '₍',
            (Script::Sub, ')') => '₎',
            _ => c,
        },
    }
}

impl BlockBuilder {
    fn new() -> Self {
        BlockBuilder {
            spans: Vec::new(),
            pending_space: false,
            script: None,
            chars: 0,
        }
    }
    fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.trim().is_empty())
    }
    /// Tab: after an inline element with reserved width (`span.spa` in a numbered
    /// list), the text resumes at the paragraph indent. Kept as is
    /// (not merged with whitespace); the compositor turns it into a tab stop.
    fn push_tab(&mut self, style: SpanStyle) {
        if !self.is_empty() {
            self.pending_space = false;
            self.append('\t', style);
        }
    }
    fn push_text(&mut self, text: &str, style: SpanStyle) {
        for ch in text.chars() {
            // Non-breaking spaces (U+00A0, narrow U+202F, tabular U+2007) count as text.
            let sp = ch.is_whitespace() && !matches!(ch, '\u{a0}' | '\u{202f}' | '\u{2007}');
            if sp {
                self.pending_space = !self.spans.is_empty() || !self.is_empty();
                continue;
            }
            if self.pending_space {
                self.pending_space = false;
                // The space takes the style of the preceding text: "word <em>word</em>"
                // keeps the space in roman, and an italic run never starts
                // with whitespace.
                let prev = self.spans.last().map(|s| s.style).unwrap_or(style);
                self.append(' ', prev);
            }
            match self.script {
                Some(sc) => {
                    let mapped = script_char(ch, sc);
                    if mapped != ch {
                        // Font superscript glyph (¹, ₂): set at normal size.
                        self.append(
                            mapped,
                            SpanStyle {
                                script: None,
                                ..style
                            },
                        );
                    } else {
                        self.append(
                            ch,
                            SpanStyle {
                                script: Some(sc),
                                ..style
                            },
                        );
                    }
                }
                None => self.append(ch, style),
            }
        }
    }
    fn append(&mut self, ch: char, style: SpanStyle) {
        self.chars += 1;
        match self.spans.last_mut() {
            Some(last) if last.style == style => last.text.push(ch),
            _ => self.spans.push(Span {
                text: ch.to_string(),
                style,
            }),
        }
    }
    /// Text of the range [from, to), and whether part of it is superscript.
    fn range(&self, from: u32, to: u32) -> (String, bool) {
        let mut out = String::new();
        let mut sup = false;
        let mut i = 0u32;
        for s in &self.spans {
            for c in s.text.chars() {
                if i >= from && i < to {
                    out.push(c);
                    sup |= s.style.script == Some(Script::Sup);
                }
                i += 1;
            }
        }
        (out, sup)
    }
    fn finish(self) -> Vec<Span> {
        self.spans
    }
}

struct Walker {
    blocks: Vec<Block>,
    current: BlockBuilder,
    /// Type of the current block (heading level n / paragraph / quote).
    kind: BlockKind,
    /// CSS style of the current block.
    style: BlockStyle,
    base: String,
    sheet: Sheet,
    /// Last frame identifier assigned.
    next_frame: u16,
    /// Parsed document (target of `#fragment` links).
    doc: String,
    links: Vec<Link>,
    anchors: HashMap<String, Anchor>,
    /// Identifiers seen whose content has not started: anchored to the next
    /// text, image or rule (a `<p id>` designates its own text, not the end
    /// of the preceding paragraph).
    pending_ids: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Paragraph,
    Heading(u8),
    Quote,
}

impl Walker {
    /// Position of the next character: (block, character).
    fn here(&self) -> (usize, u32) {
        (
            self.blocks.len(),
            self.current.chars + self.current.pending_space as u32,
        )
    }

    fn anchor_pending(&mut self) {
        if self.pending_ids.is_empty() {
            return;
        }
        let (block, ch) = self.here();
        for id in std::mem::take(&mut self.pending_ids) {
            self.anchors.entry(id).or_insert(Anchor {
                block,
                ch,
                end: block + 1,
            });
        }
    }

    /// End of a link started at `start`: its range in each block it covers.
    fn close_link(&mut self, n: &Node, href: &str, start: (usize, u32)) {
        let (b0, c0) = start;
        let (b1, c1) = (self.blocks.len(), self.current.chars);
        let external = href
            .split_once(':')
            .map(|(s, _)| s.len() > 1 && s.chars().all(|c| c.is_ascii_alphabetic()))
            .unwrap_or(false);
        let (path, fragment) = split_fragment(href);
        let target = if external {
            String::new()
        } else if path.is_empty() {
            self.doc.clone()
        } else {
            join(&self.base, &path)
        };
        let fragment = fragment
            .map(|f| percent_decode(&f))
            .filter(|f| !f.is_empty());
        let noteref = n
            .attributes()
            .any(|a| (a.name() == "type" || a.name() == "role") && a.value().contains("noteref"));
        let link = |block: usize, from: u32, to: u32, label: String, note: bool| Link {
            block,
            from,
            to,
            href: target.clone(),
            fragment: fragment.clone(),
            external,
            note,
            label,
        };
        if b1 == b0 {
            if c1 > c0 {
                let (label, sup) = self.current.range(c0, c1);
                let note = !external && (noteref || is_note_label(&label, sup));
                self.links.push(link(b0, c0, c1, label, note));
            }
            return;
        }
        // Link spanning blocks (`<a><p>…</p></a>`): each one in full.
        self.links
            .push(link(b0, c0, u32::MAX, String::new(), false));
        for b in b0 + 1..b1 {
            self.links.push(link(b, 0, u32::MAX, String::new(), false));
        }
        if c1 > 0 {
            self.links.push(link(b1, 0, c1, String::new(), false));
        }
    }

    fn flush(&mut self) {
        let b = std::mem::replace(&mut self.current, BlockBuilder::new());
        if !b.is_empty() {
            let spans = b.finish();
            let style = self.style;
            self.blocks.push(match self.kind {
                BlockKind::Heading(level) => Block::Heading {
                    level,
                    spans,
                    style,
                },
                BlockKind::Quote => Block::Quote { spans, style },
                BlockKind::Paragraph => Block::Paragraph { spans, style },
            });
        }
        self.kind = BlockKind::Paragraph;
        self.style = BlockStyle::default();
    }

    /// After the end of a child block: the bare text that follows in the container forms
    /// an anonymous block, which takes the container's style (indents, alignment,
    /// size, box) but not its vertical margins.
    fn resume(&mut self, container: BlockStyle, in_quote: bool) {
        self.style = BlockStyle {
            margin_top: None,
            margin_bottom: None,
            ..container
        };
        self.kind = if in_quote {
            BlockKind::Quote
        } else {
            BlockKind::Paragraph
        };
    }

    /// CSS declarations applicable to the element (tag, classes, `style=""`).
    fn decl(&self, n: &Node) -> css::Decl {
        let classes: Vec<&str> = n
            .attribute("class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default();
        self.sheet
            .lookup(n.tag_name().name(), &classes, n.attribute("style"))
    }

    /// Block style inherited from a declaration (container or block itself). The
    /// text is indented by the margin, border and inner
    /// spacing; a bordered or colored-background element opens a frame.
    fn block_style(&mut self, parent: BlockStyle, d: &css::Decl) -> BlockStyle {
        let font_size = match (parent.font_size, d.font_size) {
            (Some(a), Some(b)) => Some((a * b).clamp(0.6, 2.0)),
            (p, c) => c.or(p),
        };
        // The element's em, converted to the reading size.
        let fs = font_size.unwrap_or(1.0);
        let em = |l: Option<Len>| l.map(|l| l.scale_em(fs));
        let border = if d.border_sides != 0 {
            em(d.border_width)
        } else {
            None
        };
        let side = |bit: u8| {
            if d.border_sides & bit != 0 {
                border
            } else {
                None
            }
        };
        let framed =
            border.map(|b| b.px(16.0, 1000.0) > 0.0).unwrap_or(false) || d.background.is_some();
        let frame = if framed {
            self.next_frame += 1;
            Some(FrameStyle {
                id: self.next_frame,
                left: Len::sum(&[parent.margin_left, em(d.margin_left)]),
                right: Len::sum(&[parent.margin_right, em(d.margin_right)]),
                margin_top: em(d.margin_top),
                margin_bottom: em(d.margin_bottom),
                pad_top: Len::sum(&[em(d.padding_top), side(1)]),
                pad_bottom: Len::sum(&[em(d.padding_bottom), side(4)]),
                border,
                border_gray: d.border_gray.unwrap_or(0),
                sides: d.border_sides,
                background: d.background,
                radius: em(d.radius),
                width: em(d.width),
            })
        } else {
            parent.frame
        };
        BlockStyle {
            align: d.align.or(parent.align),
            indent: d.indent.or(parent.indent),
            margin_left: Len::sum(&[
                parent.margin_left,
                em(d.margin_left),
                em(d.padding_left),
                side(8),
            ]),
            margin_right: Len::sum(&[
                parent.margin_right,
                em(d.margin_right),
                em(d.padding_right),
                side(2),
            ]),
            font_size,
            // A frame carries its own vertical margins and spacing.
            margin_top: if framed {
                None
            } else {
                Len::sum(&[d.margin_top, d.padding_top])
            },
            margin_bottom: if framed {
                None
            } else {
                Len::sum(&[d.margin_bottom, d.padding_bottom])
            },
            line_height: d.line_height.or(parent.line_height),
            frame,
        }
    }

    fn walk(&mut self, n: Node, style: SpanStyle, in_quote: bool) {
        self.walk_in(n, style, in_quote, BlockStyle::default());
    }

    /// `inherited`: block style accumulated by the containers (`div.x`).
    fn walk_in(&mut self, n: Node, style: SpanStyle, in_quote: bool, inherited: BlockStyle) {
        match n.node_type() {
            NodeType::Text => {
                let text = n.text().unwrap_or("");
                if !self.pending_ids.is_empty() && text.chars().any(|c| !c.is_whitespace()) {
                    self.anchor_pending();
                }
                self.current.push_text(text, style);
            }
            NodeType::Element => {
                let tag = n.tag_name().name();
                let id = n.attribute("id");
                if let Some(id) = id {
                    self.pending_ids.push(id.to_string());
                }
                let d = if self.sheet.is_empty() && n.attribute("style").is_none() {
                    css::Decl::default()
                } else {
                    self.decl(&n)
                };
                if d.hidden {
                    return;
                }
                let link = if tag == "a" {
                    n.attribute("href").map(|h| (h, self.here()))
                } else {
                    None
                };
                // Weight and italic coming from classes ("`.calibre2 { font-weight: bold }`").
                let style = SpanStyle {
                    bold: d.bold.unwrap_or(style.bold),
                    italic: d.italic.unwrap_or(style.italic),
                    ..style
                };
                // Superscript / subscript: tag or `vertical-align`, for the duration of the subtree.
                let script = match tag {
                    "sup" => Some(Script::Sup),
                    "sub" => Some(Script::Sub),
                    _ => d.script,
                };
                let saved_script = self.current.script;
                if script.is_some() {
                    self.current.script = script;
                }
                match tag {
                    "head" | "script" | "style" | "title" => {}
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        self.flush();
                        self.kind = BlockKind::Heading(tag.as_bytes()[1] - b'0');
                        self.style = self.block_style(inherited, &d);
                        for c in n.children() {
                            self.walk_in(c, style, in_quote, inherited);
                        }
                        self.flush();
                        self.resume(inherited, in_quote);
                    }
                    "p" | "li" | "dd" | "dt" | "pre" | "tr" | "figcaption" => {
                        self.flush();
                        self.kind = if in_quote {
                            BlockKind::Quote
                        } else {
                            BlockKind::Paragraph
                        };
                        let own = self.block_style(inherited, &d);
                        self.style = own;
                        let count = self.blocks.len();
                        for c in n.children() {
                            self.walk_in(c, style, in_quote, inherited);
                        }
                        self.flush();
                        // Empty but bordered paragraph (`<p class="footline"></p>`: the rule
                        // above the notes): a block without text, which the paginator
                        // draws by its frame alone.
                        if self.blocks.len() == count
                            && own
                                .frame
                                .map(|f| f.sides & 5 != 0 && f.border.is_some())
                                .unwrap_or(false)
                            && own.frame != inherited.frame
                        {
                            self.blocks.push(Block::Paragraph {
                                spans: Vec::new(),
                                style: own,
                            });
                        }
                        self.resume(inherited, in_quote);
                    }
                    "blockquote" => {
                        self.flush();
                        let inh = self.block_style(inherited, &d);
                        // Bare text directly in the quote: Quote block, with its style.
                        self.kind = BlockKind::Quote;
                        self.style = inh;
                        for c in n.children() {
                            self.walk_in(c, style, true, inh);
                        }
                        self.flush();
                        self.resume(inherited, in_quote);
                    }
                    "div" | "section" | "article" | "body" | "html" | "ul" | "ol" | "table"
                    | "tbody" | "thead" | "figure" | "aside" | "header" | "footer" | "nav"
                    | "main" | "dl" => {
                        self.flush();
                        // The book's page margins (body, html) do not apply to a 6-inch screen.
                        let inh = if tag == "body" || tag == "html" {
                            inherited
                        } else {
                            self.block_style(inherited, &d)
                        };
                        // Bare text directly in a `<div class="x">`: it takes the container's style.
                        self.style = inh;
                        if in_quote {
                            self.kind = BlockKind::Quote;
                        }
                        for c in n.children() {
                            self.walk_in(c, style, in_quote, inh);
                        }
                        self.flush();
                        self.resume(inherited, in_quote);
                    }
                    "hr" => {
                        self.flush();
                        self.anchor_pending();
                        self.blocks.push(Block::Separator);
                    }
                    "br" => {
                        // Forced line break: treated as an end of paragraph,
                        // the next block keeps the paragraph's type and CSS style.
                        let (k, bs) = (self.kind, self.style);
                        self.flush();
                        self.kind = k;
                        self.style = bs;
                    }
                    "img" | "image" => {
                        let src = n
                            .attribute("src")
                            .or_else(|| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                            .unwrap_or("");
                        if !src.is_empty() {
                            // The image takes the position and alignment of the block that contains it
                            // (indents, box, `text-align`), not its vertical margins,
                            // except a negative top margin (an ornament overlapping the
                            // frame above it); the rest of the paragraph keeps its style.
                            let (k, bs) = (self.kind, self.style);
                            self.flush();
                            self.anchor_pending();
                            let lift = bs.margin_top.filter(|m| m.px(16.0, 1000.0) < 0.0);
                            self.blocks.push(Block::Image {
                                style: BlockStyle {
                                    margin_top: lift,
                                    margin_bottom: None,
                                    indent: None,
                                    line_height: None,
                                    ..bs
                                },
                                href: join(&self.base, src),
                                alt: n.attribute("alt").unwrap_or("").to_string(),
                            });
                            self.kind = k;
                            self.style = bs;
                        }
                    }
                    "em" | "i" | "cite" | "dfn" | "var" => {
                        let st = inline_style(
                            SpanStyle {
                                italic: d.italic.unwrap_or(true),
                                ..style
                            },
                            &d,
                            tag,
                        );
                        for c in n.children() {
                            self.walk_in(c, st, in_quote, inherited);
                        }
                        if d.reserve_after {
                            self.current.push_tab(st);
                        }
                    }
                    "strong" | "b" | "th" => {
                        let st = inline_style(
                            SpanStyle {
                                bold: d.bold.unwrap_or(true),
                                ..style
                            },
                            &d,
                            tag,
                        );
                        for c in n.children() {
                            self.walk_in(c, st, in_quote, inherited);
                        }
                        if d.reserve_after {
                            self.current.push_tab(st);
                        }
                    }
                    "td" => {
                        self.current.push_text(" ", style);
                        for c in n.children() {
                            self.walk_in(c, style, in_quote, inherited);
                        }
                        self.current.push_text(" ", style);
                    }
                    _ => {
                        let style = inline_style(style, &d, tag);
                        for c in n.children() {
                            self.walk_in(c, style, in_quote, inherited);
                        }
                        // "<span class="spa">1.</span>Choisir": the class reserves 1.2 em → one space.
                        if d.reserve_after {
                            self.current.push_tab(style);
                        }
                    }
                }
                if let Some((href, start)) = link {
                    self.close_link(&n, href, start);
                }
                // Extent of the identified element: up to and including the current block.
                if let Some(a) = id.and_then(|id| self.anchors.get_mut(id)) {
                    let end = self.blocks.len() + (!self.current.is_empty()) as usize;
                    a.end = a.end.max(end);
                }
                if script.is_some() {
                    self.current.script = saved_script;
                }
            }
            _ => {}
        }
    }
}

/// Reduces an XHTML document to its text blocks (the document's `<style>`
/// sheets only).
pub fn parse_xhtml(src: &str, base: &str) -> Result<Vec<Block>, Error> {
    parse_xhtml_styled(src, base, Sheet::default())
}

/// Like [`parse_xhtml`], with a stylesheet already loaded (the document's
/// `<style>` elements are added to it).
pub fn parse_xhtml_styled(src: &str, base: &str, sheet: Sheet) -> Result<Vec<Block>, Error> {
    Ok(parse_document(src, base, "", sheet)?.blocks)
}

/// Full parse of a document: blocks, links, anchors. `doc_path`: its
/// path in the zip (target of internal `#…` links).
pub fn parse_document(
    src: &str,
    base: &str,
    doc_path: &str,
    mut sheet: Sheet,
) -> Result<Chapter, Error> {
    let src = normalize_entities(src);
    let doc = Document::parse_with_options(&src, parse_opts())?;
    for st in doc.descendants().filter(|n| n.has_tag_name("style")) {
        if let Some(t) = st.text() {
            sheet.add(t);
        }
    }
    let body = doc
        .descendants()
        .find(|n| n.has_tag_name("body"))
        .unwrap_or(doc.root_element());
    let mut w = Walker {
        blocks: Vec::new(),
        current: BlockBuilder::new(),
        kind: BlockKind::Paragraph,
        style: BlockStyle::default(),
        base: base.to_string(),
        sheet,
        next_frame: 0,
        doc: doc_path.to_string(),
        links: Vec::new(),
        anchors: HashMap::new(),
        pending_ids: Vec::new(),
    };
    w.walk(body, SpanStyle::default(), false);
    w.flush();
    w.anchor_pending();
    Ok(Chapter {
        blocks: w.blocks,
        links: w.links,
        anchors: w.anchors,
    })
}

/// Paths (relative to the document) of the sheets linked by `<link rel="stylesheet">`.
pub fn linked_stylesheets(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("<link") {
        let tag_end = rest[i..].find('>').map(|e| i + e).unwrap_or(rest.len());
        let tag = &rest[i..tag_end];
        let attr = |name: &str| -> Option<&str> {
            let k = tag.find(&format!("{name}="))? + name.len() + 1;
            let q = tag.as_bytes().get(k).copied()?;
            if q != b'"' && q != b'\'' {
                return None;
            }
            let v = &tag[k + 1..];
            let e = v.find(q as char)?;
            Some(&v[..e])
        };
        let is_css = attr("rel")
            .map(|r| r.contains("stylesheet"))
            .unwrap_or(false)
            || attr("type").map(|t| t.contains("css")).unwrap_or(false);
        if is_css {
            if let Some(h) = attr("href") {
                out.push(h.to_string());
            }
        }
        rest = &rest[tag_end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xhtml_to_blocks() {
        let src = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><h1>Chapitre   IV</h1>
<p>La forêt <em>s'était</em> tue.&nbsp;Seul <strong>persistait</strong>
le murmure.</p>
<div><img src="../images/a.jpg" alt="Une image"/></div>
<blockquote><p>Citation.</p></blockquote>
<hr/></body></html>"#;
        let blocks = parse_xhtml(src, "OEBPS/text").unwrap();
        assert_eq!(
            blocks[0],
            Block::Heading {
                level: 1,
                spans: vec![Span {
                    text: "Chapitre IV".into(),
                    style: SpanStyle::default()
                }],
                style: BlockStyle::default()
            }
        );
        match &blocks[1] {
            Block::Paragraph { spans, .. } => {
                assert_eq!(spans.len(), 5);
                assert_eq!(spans[0].text, "La forêt ");
                assert_eq!(
                    spans[1],
                    Span {
                        text: "s'était ".into(),
                        style: SpanStyle {
                            italic: true,
                            ..SpanStyle::default()
                        }
                    }
                );
                assert_eq!(spans[2].text, "tue.\u{a0}Seul ");
                assert!(spans[3].style.bold);
                assert_eq!(spans[3].text, "persistait ");
                assert_eq!(spans[4].text, "le murmure.");
            }
            b => panic!("attendu un paragraphe, obtenu {b:?}"),
        }
        assert_eq!(
            blocks[2],
            Block::Image {
                href: "OEBPS/images/a.jpg".into(),
                alt: "Une image".into(),
                style: BlockStyle::default()
            }
        );
        assert!(matches!(blocks[3], Block::Quote { .. }));
        assert_eq!(blocks[4], Block::Separator);
    }

    #[test]
    fn classes_calibre_appliquees() {
        let src = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head>
<link rel="stylesheet" type="text/css" href="../stylesheet.css"/>
<style>.spa { display: inline-block; width: 1.2em } .bm4 { text-indent: -1.2em; margin: 0.3em 0 0 1.2em; text-align: justify }
.noindent { text-indent: 0 } .calibre2 { font-weight: bold } .cache { display: none } .petit { font-size: 0.75em }</style></head>
<body><p class="bm4"><span class="spa">1.</span>Choisir ses priorités : <b class="calibre2">Highlight</b>.</p>
<p class="noindent">Mais faut-il</p><p class="cache">invisible</p><p class="petit" style="text-align: center">note</p></body></html>"#;
        assert_eq!(
            linked_stylesheets(src),
            vec!["../stylesheet.css".to_string()]
        );
        let blocks = parse_xhtml(src, "OEBPS/text").unwrap();
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        let Block::Paragraph { spans, style } = &blocks[0] else {
            panic!()
        };
        assert_eq!(blocks[0].text(), "1.\tChoisir ses priorités : Highlight.");
        assert!(spans.iter().any(|s| s.text == "Highlight" && s.style.bold));
        assert_eq!(style.indent, Some(Len::Em(-1.2)));
        assert_eq!(style.margin_left, Some(Len::Em(1.2)));
        assert_eq!(style.align, Some(Align::Justify));
        assert_eq!(blocks[1].style().indent, Some(Len::Em(0.0)));
        assert_eq!(blocks[2].style().font_size, Some(0.75));
        assert_eq!(blocks[2].style().align, Some(Align::Center));
        // <br/> keeps the paragraph's style; bare text in a classed div; narrow no-break space kept.
        let src2 = r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.c { text-align: center } .q { margin-left: 2em }</style></head>
<body><p class="c">A<br/>B</p><div class="q">Texte nu</div><p>Oui&#8239;? Non.</p></body></html>"#;
        let b2 = parse_xhtml(src2, "x").unwrap();
        assert_eq!(b2[0].style().align, Some(Align::Center));
        assert_eq!(b2[1].style().align, Some(Align::Center), "{b2:?}");
        assert_eq!(b2[2].style().margin_left, Some(Len::Em(2.0)), "{b2:?}");
        assert!(b2[3].text().contains('\u{202f}'), "{:?}", b2[3].text());
        // Footnote calls: <sup> and vertical-align: super → Unicode superscript digits.
        let src3 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.note { vertical-align: super; font-size: 0.7em }</style></head>
<body><p>Nous, c’est Jake et JZ<sup>1</sup>. La semaine de 4 heures<a class="note" href="#n2">2</a>, H<sub>2</sub>O.</p></body></html>"##;
        let b3 = parse_xhtml(src3, "x").unwrap();
        assert_eq!(
            b3[0].text(),
            "Nous, c’est Jake et JZ¹. La semaine de 4 heures², H₂O."
        );
        // Superscript letters missing from Literata: kept as they are, but
        // marked as superscript in their style (the compositor raises them).
        let src4 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Au XXI<sup>e</sup> siècle, le 1<sup>er</sup> jour.</p></body></html>"##;
        let b4 = parse_xhtml(src4, "x").unwrap();
        assert_eq!(b4[0].text(), "Au XXIe siècle, le 1er jour.");
        let Block::Paragraph { spans, .. } = &b4[0] else {
            panic!()
        };
        // (the space that follows takes the style of what precedes: "e  ")
        assert!(
            spans
                .iter()
                .any(|s| s.text.trim() == "e" && s.style.script == Some(Script::Sup)),
            "{spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.text.trim() == "er" && s.style.script == Some(Script::Sup)),
            "{spans:?}"
        );
        // <small> (book size or default "smaller") and font-variant.
        let src5 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.c8 { font-size: 0.75em } .sc { font-variant: small-caps }</style></head><body><p>Au <small class="c8">XXI</small> siècle, <small>note</small>, <span class="sc">Victor Hugo</span>.</p></body></html>"##;
        let b5 = parse_xhtml(src5, "x").unwrap();
        let Block::Paragraph { spans, .. } = &b5[0] else {
            panic!()
        };
        assert!(
            spans
                .iter()
                .any(|s| s.text.trim() == "XXI" && s.style.size_pct == Some(75)),
            "{spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.text.trim() == "note" && s.style.size_pct == Some(83)),
            "{spans:?}"
        );
        assert!(
            spans
                .iter()
                .any(|s| s.text.trim() == "Victor Hugo"
                    && s.style.caps == Some(CapsVariant::SmallCaps)),
            "{spans:?}"
        );
        // Box: all its blocks (image included) carry the same frame; the text is
        // indented by the margin, border and inner spacing.
        let src6 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.box { background-color: #ECEDEE; border: #BCBDC0 solid 2px; border-radius: 1em; margin: 1em 0; padding: 0.5em 1em 1em 1.5em }</style></head><body><p>Avant.</p><div class="box"><p>Un.</p><img src="a.png" alt=""/><p>Deux.</p></div><p>Après.</p></body></html>"##;
        let b6 = parse_xhtml(src6, "x").unwrap();
        let ids: Vec<Option<u16>> = b6.iter().map(|b| b.style().frame.map(|f| f.id)).collect();
        assert_eq!(ids, vec![None, Some(1), Some(1), Some(1), None], "{b6:?}");
        let st = b6[1].style();
        assert_eq!(st.margin_left, Some(Len::Em(1.625)));
        let f = st.frame.unwrap();
        assert_eq!((f.background, f.border_gray, f.sides), (Some(237), 189, 15));
        // Bare text after a paragraph of the box: same frame, same indents;
        // image in a left-aligned paragraph: its alignment; empty rule.
        let src7 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.box { font-size: .75em; border: #BCBDC0 solid 2px; padding: 0 1em 0 1.5em } .boxh { text-align: left; margin: 1em 0 0.3em } .footline { width: 7em; margin: 2em 0 0.3em; border-top: currentColor solid 1px }</style></head><body><p class="boxh"><img src="i.jpg" alt=""/></p><div class="box"><p>Un.</p>Texte nu.</div><p class="footline"></p><p>Note.</p></body></html>"##;
        let b7 = parse_xhtml(src7, "x").unwrap();
        assert_eq!(b7.len(), 5, "{b7:?}");
        match &b7[0] {
            Block::Image { style, .. } => {
                assert_eq!((style.align, style.margin_top), (Some(Align::Left), None))
            }
            b => panic!("attendu une image, obtenu {b:?}"),
        }
        let (s1, s2) = (b7[1].style(), b7[2].style());
        assert_eq!(b7[2].text().trim(), "Texte nu.");
        assert_eq!(
            (s1.frame.map(|f| f.id), s2.frame.map(|f| f.id)),
            (Some(1), Some(1))
        );
        // Indents in em of the reading size: (1.5 + 0.125) × 0.75.
        assert_eq!(
            (s1.margin_left, s2.margin_left, s2.font_size),
            (Some(Len::Em(1.21875)), Some(Len::Em(1.21875)), Some(0.75))
        );
        assert_eq!(s1.frame.unwrap().border, Some(Len::Em(0.09375)));
        match &b7[3] {
            Block::Paragraph { spans, style } => {
                assert!(spans.is_empty());
                let f = style.frame.unwrap();
                assert_eq!(
                    (f.sides, f.width, f.border_gray),
                    (1, Some(Len::Em(7.0)), 0)
                );
            }
            b => panic!("attendu le filet, obtenu {b:?}"),
        }
        assert_eq!(b7[4].style().frame, None);
        // Ornament overlapping the top of a frame: the negative margin of its
        // paragraph is kept (positive margins are not).
        let src8 = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.fm { border: currentColor solid 2px } .fig1 { text-align: center; margin: -2.2em 0 0 } .fig { margin: 1em 0 0 }</style></head><body><div class="fm"><p class="fig1"><img src="v.png" alt=""/></p><p>Citation.</p></div><p class="fig"><img src="w.png" alt=""/></p></body></html>"##;
        let b8 = parse_xhtml(src8, "x").unwrap();
        assert_eq!(b8.len(), 3, "{b8:?}");
        assert_eq!(
            (b8[0].style().margin_top, b8[0].style().frame.map(|f| f.id)),
            (Some(Len::Em(-2.2)), Some(1))
        );
        assert_eq!(b8[2].style().margin_top, None);
    }

    #[test]
    fn liens_et_ancres() {
        let src = r##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
<h2 id="h_1">Titre</h2>
<p class="noindent">Nous, c’est Jake et JZ<sup><a href="intro.xhtml#A_1" id="B_1">1</a></sup>. Voir <a href="ch2.xhtml#page178">p. 178</a> et <a href="http://x.org">le site</a>.</p>
<p>Un appel EPUB 3<a epub:type="noteref" href="#n3">note</a>, une lettre<sup><a href="#n4">a</a></sup>.</p>
<p class="note"><span class="spa1"><a href="intro.xhtml#B_1" id="A_1">1</a></span>Pas de fausse joie.</p>
<aside id="n3"><p>Premier paragraphe.</p><p>Second.</p></aside>
<p><a id="vide"/>Après une ancre vide.</p>
<a href="#h_1"><p>Bloc entier</p></a>
</body></html>"##;
        let c = parse_document(src, "OEBPS", "OEBPS/intro.xhtml", Sheet::default()).unwrap();
        let texts: Vec<String> = c.blocks.iter().map(|b| b.text()).collect();
        assert_eq!(texts[1], "Nous, c’est Jake et JZ¹. Voir p. 178 et le site.");
        let a = |id: &str| c.anchors.get(id).map(|a| (a.block, a.ch, a.end));
        assert_eq!(a("h_1"), Some((0, 0, 1)));
        assert_eq!(a("B_1"), Some((1, 22, 2)));
        assert_eq!(a("A_1"), Some((3, 0, 4)));
        assert_eq!(a("n3"), Some((4, 0, 6)), "une note de deux paragraphes");
        assert_eq!(a("vide"), Some((6, 0, 7)));
        #[allow(clippy::type_complexity)]
        let l: Vec<(usize, u32, u32, &str, Option<&str>, bool, bool, &str)> = c
            .links
            .iter()
            .map(|l| {
                (
                    l.block,
                    l.from,
                    l.to,
                    l.href.as_str(),
                    l.fragment.as_deref(),
                    l.external,
                    l.note,
                    l.label.as_str(),
                )
            })
            .collect();
        assert_eq!(
            l[0],
            (
                1,
                22,
                23,
                "OEBPS/intro.xhtml",
                Some("A_1"),
                false,
                true,
                "¹"
            )
        );
        assert_eq!(
            l[1],
            (
                1,
                30,
                36,
                "OEBPS/ch2.xhtml",
                Some("page178"),
                false,
                false,
                "p. 178"
            )
        );
        assert_eq!((l[2].5, l[2].6, l[2].7), (true, false, "le site"));
        assert_eq!(
            (l[3].3, l[3].4, l[3].6),
            ("OEBPS/intro.xhtml", Some("n3"), true),
            "epub:type=noteref"
        );
        assert_eq!((l[4].6, l[4].7), (true, "a"), "lettre en exposant");
        // The note's back-reference to its call also looks like a call: the
        // application decides (target in the middle of a paragraph → jump).
        assert_eq!(
            (l[5].0, l[5].1, l[5].2, l[5].4, l[5].6),
            (3, 0, 1, Some("B_1"), true)
        );
        assert_eq!(
            (l[6].0, l[6].1, l[6].2, l[6].4),
            (7, 0, u32::MAX, Some("h_1")),
            "lien autour d'un paragraphe"
        );
        assert_eq!(l.len(), 7);
    }

    #[test]
    fn join_resolves_relative_paths() {
        assert_eq!(join("OEBPS", "text/ch1.xhtml"), "OEBPS/text/ch1.xhtml");
        assert_eq!(
            join("OEBPS/text", "../images/a%20b.jpg"),
            "OEBPS/images/a b.jpg"
        );
        assert_eq!(join("", "ch1.xhtml#frag"), "ch1.xhtml");
    }

    /// Integration test on a real book if `SIGNET_EPUB_SAMPLE` is set.
    #[test]
    fn open_real_book() {
        let Ok(path) = std::env::var("SIGNET_EPUB_SAMPLE") else {
            return;
        };
        let t0 = std::time::Instant::now();
        let mut book = Book::open(&path).unwrap();
        let open_t = t0.elapsed();
        eprintln!("titre={:?} auteur={:?} langue={:?} spine={} toc={} couverture={:?} ouvert en {open_t:?}",
            book.metadata.title, book.metadata.author, book.metadata.language, book.spine.len(), book.toc.len(), book.cover_href);
        for e in book.toc.iter().take(8) {
            eprintln!(
                "  {}{} → {}",
                "  ".repeat(e.depth as usize),
                e.label,
                e.href
            );
        }
        let mut total_blocks = 0;
        let mut total_chars = 0;
        let (mut links, mut notes, mut anchors, mut unresolved) = (0, 0, 0, 0);
        let t1 = std::time::Instant::now();
        for i in 0..book.spine.len() {
            let ch = book.chapter_full(i).unwrap();
            links += ch.links.len();
            notes += ch.links.iter().filter(|l| l.note).count();
            anchors += ch.anchors.len();
            for l in ch.links.iter().filter(|l| !l.external) {
                let Some(target) = book.spine_index(&l.href) else {
                    unresolved += 1;
                    continue;
                };
                if let Some(f) = &l.fragment {
                    if !book.chapter_full(target).unwrap().anchors.contains_key(f) {
                        unresolved += 1;
                    }
                }
            }
            let blocks = ch.blocks;
            total_blocks += blocks.len();
            total_chars += blocks
                .iter()
                .map(|b| b.text().chars().count())
                .sum::<usize>();
            if i == 3 {
                for b in blocks.iter().take(6) {
                    eprintln!(
                        "  [{}] {}",
                        match b {
                            Block::Heading { level, .. } => format!("h{level}"),
                            Block::Paragraph { .. } => "p".into(),
                            Block::Quote { .. } => "q".into(),
                            Block::Image { .. } => "img".into(),
                            Block::Separator => "hr".into(),
                        },
                        b.text().chars().take(90).collect::<String>()
                    );
                }
            }
        }
        eprintln!(
            "{} chapitres, {total_blocks} blocs, {total_chars} caractères, parsés en {:?}",
            book.spine.len(),
            t1.elapsed()
        );
        eprintln!("{links} liens dont {notes} appels de note, {anchors} ancres, {unresolved} cibles introuvables");
        assert!(total_chars > 1000);
    }
}
