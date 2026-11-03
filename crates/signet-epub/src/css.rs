//! Subset of CSS sufficient for laying out a book: simple selectors
//! (`p`, `.class`, `p.class`, comma-separated lists),
//! alignment, first-line indent, horizontal margins, weight,
//! italic, relative size, `display: none` and inline elements
//! that reserve a width (`inline-block` + `width`, right margins).
//! Everything else is ignored without error.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    /// Multiple of the block's font size.
    Em(f32),
    /// Percentage of the typesetting width.
    Pct(f32),
    /// Sum of the two (margin + border + inner spacing, mixed units).
    Both { em: f32, pct: f32 },
}

impl Len {
    /// Value in pixels for a font size `px` and a width `width`.
    pub fn px(self, px: f32, width: f32) -> f32 {
        match self {
            Len::Em(e) => e * px,
            Len::Pct(p) => p / 100.0 * width,
            Len::Both { em, pct } => em * px + pct / 100.0 * width,
        }
    }

    /// The same length, its em multiplied by `k` (converting the em of an element of
    /// size `k` into em of the reading size); percentages do not change.
    pub fn scale_em(self, k: f32) -> Len {
        match self {
            Len::Em(e) => Len::Em(e * k),
            Len::Pct(p) => Len::Pct(p),
            Len::Both { em, pct } => Len::Both { em: em * k, pct },
        }
    }

    fn parts(self) -> (f32, f32) {
        match self {
            Len::Em(e) => (e, 0.0),
            Len::Pct(p) => (0.0, p),
            Len::Both { em, pct } => (em, pct),
        }
    }

    /// Sum of optional lengths (absent = 0); `None` if all are absent.
    pub fn sum(items: &[Option<Len>]) -> Option<Len> {
        if items.iter().all(|i| i.is_none()) {
            return None;
        }
        let (mut em, mut pct) = (0.0, 0.0);
        for (e, p) in items.iter().flatten().map(|l| l.parts()) {
            em += e;
            pct += p;
        }
        Some(if pct == 0.0 {
            Len::Em(em)
        } else if em == 0.0 {
            Len::Pct(pct)
        } else {
            Len::Both { em, pct }
        })
    }
    fn parse(v: &str) -> Option<Len> {
        let v = v.trim();
        if v == "0" || v == "auto" || v == "inherit" {
            return if v == "0" { Some(Len::Em(0.0)) } else { None };
        }
        let num_end = v
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
            .unwrap_or(v.len());
        let n: f32 = v[..num_end].parse().ok()?;
        match v[num_end..].trim() {
            "em" | "rem" => Some(Len::Em(n)),
            "ex" => Some(Len::Em(n * 0.5)),
            "px" => Some(Len::Em(n / 16.0)),
            "pt" => Some(Len::Em(n / 12.0)),
            "%" => Some(Len::Pct(n)),
            "" => Some(Len::Em(n / 16.0)),
            _ => None,
        }
    }
}

/// Small caps requested by the stylesheet (`font-variant`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CapsVariant {
    /// `small-caps`: lowercase letters become small caps.
    SmallCaps,
    /// `all-small-caps`: capitals too.
    AllSmallCaps,
}

/// Superscript or subscript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Script {
    Sup,
    Sub,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
}

/// Declarations retained from a set of rules (`None` = not specified).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decl {
    pub align: Option<Align>,
    pub indent: Option<Len>,
    pub margin_left: Option<Len>,
    pub margin_right: Option<Len>,
    pub margin_top: Option<Len>,
    pub margin_bottom: Option<Len>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// Relative size (em), for blocks only.
    pub font_size: Option<f32>,
    /// Line height, as a multiple of the element's text size.
    pub line_height: Option<f32>,
    /// Small caps (`font-variant`), `Some(None)` to go back to normal.
    pub caps: Option<Option<CapsVariant>>,
    /// Inner spacing (`padding`), distinct from the margin.
    pub padding_left: Option<Len>,
    pub padding_right: Option<Len>,
    pub padding_top: Option<Len>,
    pub padding_bottom: Option<Len>,
    /// Border: thickness, gray level (0 = black), sides (1 top, 2 right, 4 bottom, 8 left).
    pub border_width: Option<Len>,
    pub border_gray: Option<u8>,
    pub border_sides: u8,
    /// Background (gray) and corner radius.
    pub background: Option<u8>,
    pub radius: Option<Len>,
    /// Width of a block (`width`), for the frame of a rule (`.footline { width: 7em }`).
    pub width: Option<Len>,
    pub hidden: bool,
    /// `vertical-align: super` / `sub` (appels de note).
    pub script: Option<Script>,
    /// Inline element that takes up a width (`inline-block` + `width`,
    /// `padding-right`, `margin-right`): a space after it is guaranteed.
    pub reserve_after: bool,
}

impl Decl {
    fn merge(&mut self, o: &Decl) {
        if o.align.is_some() {
            self.align = o.align;
        }
        if o.indent.is_some() {
            self.indent = o.indent;
        }
        if o.margin_left.is_some() {
            self.margin_left = o.margin_left;
        }
        if o.margin_right.is_some() {
            self.margin_right = o.margin_right;
        }
        if o.margin_top.is_some() {
            self.margin_top = o.margin_top;
        }
        if o.margin_bottom.is_some() {
            self.margin_bottom = o.margin_bottom;
        }
        if o.bold.is_some() {
            self.bold = o.bold;
        }
        if o.italic.is_some() {
            self.italic = o.italic;
        }
        if o.font_size.is_some() {
            self.font_size = o.font_size;
        }
        if o.line_height.is_some() {
            self.line_height = o.line_height;
        }
        if o.caps.is_some() {
            self.caps = o.caps;
        }
        if o.padding_left.is_some() {
            self.padding_left = o.padding_left;
        }
        if o.padding_right.is_some() {
            self.padding_right = o.padding_right;
        }
        if o.padding_top.is_some() {
            self.padding_top = o.padding_top;
        }
        if o.padding_bottom.is_some() {
            self.padding_bottom = o.padding_bottom;
        }
        if o.border_width.is_some() {
            self.border_width = o.border_width;
        }
        if o.border_gray.is_some() {
            self.border_gray = o.border_gray;
        }
        if o.border_sides != 0 || o.border_width.is_some() {
            self.border_sides = o.border_sides;
        }
        if o.background.is_some() {
            self.background = o.background;
        }
        if o.radius.is_some() {
            self.radius = o.radius;
        }
        if o.width.is_some() {
            self.width = o.width;
        }
        self.hidden |= o.hidden;
        self.reserve_after |= o.reserve_after;
        if o.script.is_some() {
            self.script = o.script;
        }
    }

    /// Parses a list of `prop: value; …` declarations.
    pub fn parse(body: &str) -> Decl {
        let mut d = Decl::default();
        let mut inline_block = false;
        let mut has_width = false;
        for decl in body.split(';') {
            let Some((k, v)) = decl.split_once(':') else {
                continue;
            };
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_ascii_lowercase();
            let v = match v.find('!') {
                Some(i) if v[i + 1..].trim_start().starts_with("important") => {
                    v[..i].trim().to_string()
                }
                _ => v,
            };
            match k.as_str() {
                "text-align" => {
                    d.align = match v.as_str() {
                        "left" | "start" => Some(Align::Left),
                        "center" => Some(Align::Center),
                        "right" | "end" => Some(Align::Right),
                        "justify" => Some(Align::Justify),
                        _ => None,
                    }
                }
                "text-indent" => d.indent = Len::parse(&v),
                "margin-left" => d.margin_left = Len::parse(&v),
                "margin-right" => d.margin_right = Len::parse(&v),
                "margin-top" => d.margin_top = Len::parse(&v),
                "margin-bottom" => d.margin_bottom = Len::parse(&v),
                "padding-top" => d.padding_top = Len::parse(&v).or(d.padding_top),
                "padding-bottom" => d.padding_bottom = Len::parse(&v).or(d.padding_bottom),
                "padding-left" => d.padding_left = Len::parse(&v).or(d.padding_left),
                "padding-right" => {
                    if let Some(l) = Len::parse(&v) {
                        d.padding_right = Some(l);
                        if l.px(16.0, 1000.0) > 0.0 {
                            d.reserve_after = true;
                        }
                    }
                }
                "margin" | "padding" => {
                    // top right bottom left
                    let parts: Vec<&str> = v.split_whitespace().collect();
                    let (t, r, b, l) = match parts.len() {
                        1 => (parts[0], parts[0], parts[0], parts[0]),
                        2 => (parts[0], parts[1], parts[0], parts[1]),
                        3 => (parts[0], parts[1], parts[2], parts[1]),
                        4 => (parts[0], parts[1], parts[2], parts[3]),
                        _ => continue,
                    };
                    if k == "margin" {
                        d.margin_left = Len::parse(l);
                        d.margin_right = Len::parse(r);
                        d.margin_top = Len::parse(t);
                        d.margin_bottom = Len::parse(b);
                    } else {
                        d.padding_left = Len::parse(l);
                        d.padding_right = Len::parse(r);
                        d.padding_top = Len::parse(t);
                        d.padding_bottom = Len::parse(b);
                    }
                }
                "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
                    let sides = match k.as_str() {
                        "border-top" => 1,
                        "border-right" => 2,
                        "border-bottom" => 4,
                        "border-left" => 8,
                        _ => 15,
                    };
                    let (mut w, mut style_none, mut gray) = (None, false, None);
                    for tok in v.split_whitespace() {
                        match tok {
                            "none" | "hidden" => style_none = true,
                            "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge"
                            | "inset" | "outset" => {}
                            "thin" => w = Some(Len::Em(1.0 / 16.0)),
                            "medium" => w = Some(Len::Em(3.0 / 16.0)),
                            "thick" => w = Some(Len::Em(5.0 / 16.0)),
                            _ => {
                                if let Some(g) = parse_gray(tok) {
                                    gray = Some(g)
                                } else if let Some(l) = Len::parse(tok) {
                                    w = Some(l)
                                }
                            }
                        }
                    }
                    if style_none || w.map(|l| l.px(16.0, 1000.0) <= 0.0).unwrap_or(false) {
                        d.border_sides &= !sides;
                    } else {
                        d.border_sides |= sides;
                        d.border_width = w.or(d.border_width).or(Some(Len::Em(3.0 / 16.0)));
                        d.border_gray = gray.or(d.border_gray);
                    }
                }
                "border-width" => d.border_width = v.split_whitespace().next().and_then(Len::parse),
                "border-color" => d.border_gray = v.split_whitespace().next().and_then(parse_gray),
                "border-style" => {
                    if v.starts_with("none") || v.starts_with("hidden") {
                        d.border_sides = 0
                    } else if d.border_sides == 0 {
                        d.border_sides = 15
                    }
                }
                "border-radius" => d.radius = v.split_whitespace().next().and_then(Len::parse),
                "background-color" | "background" => {
                    d.background = v
                        .split_whitespace()
                        .find_map(parse_gray)
                        .filter(|&g| g < 250);
                }
                "font-weight" => {
                    d.bold = match v.as_str() {
                        "bold" | "bolder" | "600" | "700" | "800" | "900" => Some(true),
                        "normal" | "lighter" | "100" | "200" | "300" | "400" | "500" => Some(false),
                        _ => None,
                    }
                }
                "font-style" => {
                    d.italic = match v.as_str() {
                        "italic" | "oblique" => Some(true),
                        "normal" => Some(false),
                        _ => None,
                    }
                }
                "font-size" => {
                    d.font_size = match v.as_str() {
                        "smaller" | "small" => Some(0.85),
                        "x-small" | "xx-small" => Some(0.75),
                        "larger" | "large" => Some(1.2),
                        "x-large" => Some(1.4),
                        "xx-large" => Some(1.6),
                        "medium" => Some(1.0),
                        _ => Len::parse(&v).map(|l| match l {
                            Len::Em(e) => e,
                            Len::Pct(p) => p / 100.0,
                            Len::Both { em, .. } => em,
                        }),
                    }
                    .map(|s| s.clamp(0.6, 2.0));
                }
                "font-variant" | "font-variant-caps" => {
                    d.caps = if v.contains("all-small-caps") {
                        Some(Some(CapsVariant::AllSmallCaps))
                    } else if v.contains("small-caps") {
                        Some(Some(CapsVariant::SmallCaps))
                    } else if v == "normal" {
                        Some(None)
                    } else {
                        d.caps
                    };
                }
                "line-height" => {
                    d.line_height = match v.as_str() {
                        "normal" | "inherit" | "initial" => None,
                        _ => match Len::parse(&v) {
                            // "1.2" (no unit) arrives here as 1.2/16 em: we read it again.
                            Some(Len::Em(_)) if v.trim().parse::<f32>().is_ok() => {
                                v.trim().parse::<f32>().ok()
                            }
                            Some(Len::Em(e)) => Some(e),
                            Some(Len::Pct(p)) => Some(p / 100.0),
                            Some(Len::Both { em, .. }) => Some(em),
                            None => None,
                        },
                    }
                    .map(|l| l.clamp(0.8, 3.0));
                }
                "font" => {
                    if v.contains("italic") {
                        d.italic = Some(true);
                    }
                    if v.contains("bold") {
                        d.bold = Some(true);
                    }
                }
                "display" => {
                    if v == "none" {
                        d.hidden = true;
                    }
                    if v == "inline-block" {
                        inline_block = true;
                    }
                }
                "visibility" => {
                    if v == "hidden" {
                        d.hidden = true;
                    }
                }
                "vertical-align" => {
                    d.script = match v.as_str() {
                        "super" | "top" | "text-top" => Some(Script::Sup),
                        "sub" | "bottom" | "text-bottom" => Some(Script::Sub),
                        "baseline" => None,
                        _ => d.script,
                    }
                }
                "width" => {
                    d.width = Len::parse(&v);
                    has_width = d.width.map(|l| l.px(16.0, 1000.0) > 0.0).unwrap_or(false);
                }
                "min-width" => {
                    has_width = Len::parse(&v)
                        .map(|l| l.px(16.0, 1000.0) > 0.0)
                        .unwrap_or(false)
                }
                _ => {}
            }
        }
        if inline_block && has_width {
            d.reserve_after = true;
        }
        d
    }
}

/// Couleur CSS → niveau de gris (luminance) ; `currentColor` = encre noire.
pub fn parse_gray(v: &str) -> Option<u8> {
    let v = v.trim().trim_end_matches(';').to_ascii_lowercase();
    let lum = |r: f32, g: f32, b: f32| {
        (0.2126 * r + 0.7152 * g + 0.0722 * b)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    if let Some(h) = v.strip_prefix('#') {
        let hex = |s: &str| u8::from_str_radix(s, 16).ok();
        return match h.len() {
            3 => {
                let c: Vec<u8> = h
                    .chars()
                    .filter_map(|c| hex(&c.to_string()))
                    .map(|x| x * 17)
                    .collect();
                (c.len() == 3).then(|| lum(c[0] as f32, c[1] as f32, c[2] as f32))
            }
            6 => Some(lum(
                hex(&h[0..2])? as f32,
                hex(&h[2..4])? as f32,
                hex(&h[4..6])? as f32,
            )),
            _ => None,
        };
    }
    if let Some(inner) = v
        .strip_prefix("rgb(")
        .or_else(|| v.strip_prefix("rgba("))
        .and_then(|r| r.strip_suffix(')'))
    {
        let n: Vec<f32> = inner
            .split(',')
            .filter_map(|x| x.trim().trim_end_matches('%').parse().ok())
            .collect();
        return (n.len() >= 3).then(|| lum(n[0], n[1], n[2]));
    }
    match v.as_str() {
        "currentcolor" | "black" => Some(0),
        "white" => Some(255),
        "gray" | "grey" => Some(128),
        "silver" => Some(192),
        "lightgray" | "lightgrey" => Some(211),
        "darkgray" | "darkgrey" => Some(169),
        "dimgray" | "dimgrey" => Some(105),
        "gainsboro" => Some(220),
        "whitesmoke" => Some(245),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Selector {
    tag: Option<String>,
    class: Option<String>,
}

impl Selector {
    /// `p`, `.x`, `p.x`; `None` for anything we cannot evaluate
    /// (descendants, identifiers, pseudo-classes…), which is then ignored.
    fn parse(s: &str) -> Option<Selector> {
        let s = s.trim();
        if s.is_empty()
            || s.contains(|c: char| {
                c.is_whitespace()
                    || c == '>'
                    || c == '+'
                    || c == '~'
                    || c == '#'
                    || c == ':'
                    || c == '['
                    || c == '*'
            })
        {
            return None;
        }
        let (tag, class) = match s.split_once('.') {
            Some((t, c)) => (t, Some(c)),
            None => (s, None),
        };
        if let Some(c) = class {
            if c.is_empty() || c.contains('.') {
                return None;
            }
        }
        Some(Selector {
            tag: if tag.is_empty() {
                None
            } else {
                Some(tag.to_ascii_lowercase())
            },
            class: class.map(|c| c.to_string()),
        })
    }
    fn specificity(&self) -> u8 {
        (self.tag.is_some() as u8) + 2 * (self.class.is_some() as u8)
    }
    fn matches(&self, tag: &str, classes: &[&str]) -> bool {
        self.tag.as_deref().map(|t| t == tag).unwrap_or(true)
            && self
                .class
                .as_deref()
                .map(|c| classes.contains(&c))
                .unwrap_or(true)
    }
}

/// Stylesheet: rules in document order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    rules: Vec<(Selector, Decl)>,
}

impl Sheet {
    pub fn parse(src: &str) -> Sheet {
        let mut sheet = Sheet::default();
        sheet.add(src);
        sheet
    }

    /// Adds the rules of `src` (after those already present).
    pub fn add(&mut self, src: &str) {
        let src = strip_comments(src);
        let mut rest = src.as_str();
        while let Some(open) = rest.find('{') {
            // At-rules without a block ("@charset "UTF-8"; @import url(x);") precede
            // the selector in the same segment: we keep only what follows the last ";".
            let head = rest[..open].rsplit(';').next().unwrap_or("").trim();
            if head.starts_with('@') {
                // @media, @font-face, @page…: we skip the whole block, nested braces included.
                let mut depth = 0usize;
                let mut end = rest.len();
                for (k, c) in rest[open..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = open + k + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                rest = &rest[end..];
                continue;
            }
            let Some(close) = rest[open..].find('}') else {
                break;
            };
            let body = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let decl = Decl::parse(body);
            if decl == Decl::default() {
                continue;
            }
            for sel in head.split(',') {
                if let Some(s) = Selector::parse(sel) {
                    self.rules.push((s, decl.clone()));
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Adds the rules of another stylesheet (after its own).
    pub fn extend(&mut self, other: &Sheet) {
        self.rules.extend(other.rules.iter().cloned());
    }

    /// Declarations applicable to an element: rules by specificity
    /// increasing (tag < class < tag.class), then `style=""`.
    pub fn lookup(&self, tag: &str, classes: &[&str], inline: Option<&str>) -> Decl {
        let mut d = Decl::default();
        for spec in 1..=3u8 {
            for (sel, decl) in &self.rules {
                if sel.specificity() == spec && sel.matches(tag, classes) {
                    d.merge(decl);
                }
            }
        }
        if let Some(s) = inline {
            d.merge(&Decl::parse(s));
        }
        d
    }
}

fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feuille_calibre() {
        let css = r#"
            /* commentaire { piège } */
            @charset "UTF-8"; @import url("autre.css");
            @media amzn-kf8 { .a { font-style: italic } .b { font-weight: bold } .c { font-style: italic } }
            .spa { display: inline-block; text-indent: 0; white-space: pre-wrap; width: 1.2em }
            .noindent { display: block; font-family: "times1"; text-align: justify; text-indent: 0; margin: 0.5em 0 0 }
            .bm4 { text-align: justify; text-indent: -1.2em; margin: 0.3em 0 0 1.2em }
            .calibre2 { font-weight: bold }
            p, .fig2 { text-align: center }
            p.noindent { text-indent: 0 }
            body p.x { color: red; text-align: right }
            @media print { p { margin: 0 } }
        "#;
        let s = Sheet::parse(css);
        let spa = s.lookup("span", &["spa"], None);
        assert!(spa.reserve_after);
        let bm4 = s.lookup("p", &["bm4"], None);
        assert_eq!(bm4.indent, Some(Len::Em(-1.2)));
        assert_eq!(bm4.margin_left, Some(Len::Em(1.2)));
        assert_eq!(bm4.margin_right, Some(Len::Em(0.0)));
        assert_eq!(bm4.margin_top, Some(Len::Em(0.3)));
        assert_eq!(bm4.margin_bottom, Some(Len::Em(0.0)));
        assert_eq!(bm4.align, Some(Align::Justify)); // .bm4 wins over p
        assert_eq!(s.lookup("p", &[], None).align, Some(Align::Center));
        assert_eq!(s.lookup("b", &["calibre2"], None).bold, Some(true));
        assert_eq!(s.lookup("p", &["x"], None).align, Some(Align::Center)); // "body p.x" ignored
        assert_eq!(
            s.lookup("p", &["noindent"], Some("text-align: left")).align,
            Some(Align::Left)
        );
        assert_eq!(Len::parse("12px"), Some(Len::Em(0.75)));
        assert_eq!(Len::parse("10%"), Some(Len::Pct(10.0)));
        assert!(Decl::parse("display:none").hidden);
        assert_eq!(Decl::parse("font-size: 0.75em").font_size, Some(0.75));
        assert_eq!(Decl::parse("line-height: 1.2").line_height, Some(1.2));
        assert_eq!(Decl::parse("line-height: 1em").line_height, Some(1.0));
        assert_eq!(Decl::parse("line-height: 150%").line_height, Some(1.5));
        assert_eq!(Decl::parse("line-height: normal").line_height, None);
        assert_eq!(
            Decl::parse("font-variant: small-caps").caps,
            Some(Some(CapsVariant::SmallCaps))
        );
        assert_eq!(
            Decl::parse("font-variant-caps: all-small-caps").caps,
            Some(Some(CapsVariant::AllSmallCaps))
        );
        // Boxes: border, background, radius; padding no longer replaces the margin.
        let b = Decl::parse("background-color: #ECEDEE; border-radius: 1em; margin: 1em 0; padding: 0.5em 1em 1em 1.5em; border: #BCBDC0 solid 2px");
        assert_eq!(b.background, Some(237));
        assert_eq!(b.border_gray, Some(189));
        assert_eq!(b.border_sides, 15);
        assert_eq!(b.border_width, Some(Len::Em(0.125)));
        assert_eq!(b.radius, Some(Len::Em(1.0)));
        assert_eq!(
            (b.margin_left, b.padding_left),
            (Some(Len::Em(0.0)), Some(Len::Em(1.5)))
        );
        let fm = Decl::parse("margin: 25% 2em 0; padding: 0.3em; border: currentColor solid 2px");
        assert_eq!(
            (
                fm.margin_top,
                fm.margin_left,
                fm.padding_left,
                fm.border_gray
            ),
            (
                Some(Len::Pct(25.0)),
                Some(Len::Em(2.0)),
                Some(Len::Em(0.3)),
                Some(0)
            )
        );
        assert_eq!(
            Decl::parse("border-top: currentColor solid 1px").border_sides,
            1
        );
        assert_eq!(
            Len::sum(&[Some(Len::Em(1.0)), None, Some(Len::Pct(10.0))]),
            Some(Len::Both { em: 1.0, pct: 10.0 })
        );
        // Rules under @media do not leak, and block-less at-rules do not hide the next one.
        assert_eq!(s.lookup("p", &["c"], None).italic, None);
        assert!(s.lookup("span", &["spa"], None).reserve_after);
        assert_eq!(Decl::parse("FONT-WEIGHT: BOLD !IMPORTANT").bold, Some(true));
        assert_eq!(
            Decl::parse("text-align: center ! important").align,
            Some(Align::Center)
        );
    }
}
