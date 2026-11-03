//! Definition window: the dictionary entry, formatted (headword,
//! pronunciation, category, numbered senses, italic examples, cross-references in
//! small caps that touch), in a bottom panel as tall as
//! the entry, paginated when it is long.

use crate::icons::{self, Icon};
use crate::theme::*;
use crate::ui::{self, Hit, HitZone};
use signet_dict::{RichEntry, Run};
use signet_fb::{Canvas, Rect};
use signet_text::{Align, Caps, Fmt, Line, ParagraphStyle, Script, Style, Typesetter};

/// What the window shows.
pub struct View<'a> {
    /// Word looked up (title when nothing is found).
    pub word: &'a str,
    pub entries: &'a [RichEntry],
    /// Near-miss words, when nothing is found.
    pub suggestions: &'a [String],
    /// No dictionary on the reader.
    pub missing: bool,
    pub page: usize,
    /// Word before a followed cross-reference: "‹" goes back to it.
    pub back: Option<&'a str>,
}

/// Result of drawing: zones, rectangle to refresh, page count,
/// and the target of each `Hit::DictLink(i)`.
pub struct Drawn {
    pub zones: Vec<HitZone>,
    pub rect: Rect,
    pub pages: usize,
    pub links: Vec<String>,
}

/// Size and line spacing of the definition text.
const PX: f32 = 24.0;
const LH: f32 = 1.34;
/// Position of the sense numbers, to the left of the text.
const GUTTER: i32 = 34;
/// Height of the page buttons, at the bottom.
const FOOT: i32 = 72;

/// A laid-out paragraph: where to place it, its lines, its cross-references.
struct Laid {
    x: i32,
    number: Option<usize>,
    lines: Vec<Line>,
    st: ParagraphStyle,
    /// Tappable character ranges → word to look up.
    links: Vec<(u32, u32, String)>,
    /// Space before the paragraph (between two senses).
    gap: f32,
    /// Next entry (homograph): a rule above.
    rule: bool,
}

fn fmt_of(r: &Run) -> Fmt {
    let style = match (r.bold, r.italic) {
        (true, true) => Style::BoldItalic,
        (true, false) => Style::Bold,
        (false, true) => Style::Italic,
        (false, false) => Style::Regular,
    };
    let script = if r.sup {
        Script::Sup
    } else if r.sub {
        Script::Sub
    } else {
        Script::Normal
    };
    Fmt {
        style,
        script,
        scale: 1.0,
        caps: if r.caps {
            Caps::SmallCaps
        } else {
            Caps::Normal
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn lay(
    ts: &Typesetter,
    runs: &[Run],
    number: Option<usize>,
    indent: i32,
    x0: i32,
    width: i32,
    gap: f32,
    rule: bool,
) -> Laid {
    let st = ParagraphStyle {
        style: Style::Regular,
        px: PX,
        line_height: LH,
        align: Align::Left,
        indent: 0.0,
    };
    let spans: Vec<(&str, Fmt)> = runs.iter().map(|r| (r.text.as_str(), fmt_of(r))).collect();
    let (lines, _) = ts.layout_formatted(&spans, (width - indent) as f32, st);
    let mut links = Vec::new();
    let mut ch = 0u32;
    for r in runs {
        let n = r.text.chars().count() as u32;
        if let Some(l) = &r.link {
            links.push((ch, ch + n, l.clone()));
        }
        ch += n;
    }
    Laid {
        x: x0 + indent,
        number,
        lines,
        st,
        links,
        gap,
        rule,
    }
}

/// Paragraphs of all entries: numbered senses indented (the number
/// in the margin), continuations at the same indent, divisions ("V. intr.") at the edge;
/// each homograph after the first opens with its word and category.
fn lay_entries(ts: &Typesetter, entries: &[RichEntry], x0: i32, width: i32) -> Vec<Laid> {
    let mut out = Vec::new();
    for (k, e) in entries.iter().enumerate() {
        if k > 0 {
            let mut runs = vec![Run {
                text: e.headword.clone(),
                bold: true,
                ..Default::default()
            }];
            if let Some(c) = &e.category {
                runs.push(Run {
                    text: format!("  {c}"),
                    italic: true,
                    ..Default::default()
                });
            }
            out.push(lay(ts, &runs, None, 0, x0, width, PX * 1.2, true));
        }
        for p in &e.paras {
            let indent = if p.number.is_some() || p.continued {
                GUTTER
            } else {
                0
            };
            let gap = if p.number.is_some() {
                PX * 0.5
            } else if p.continued {
                PX * 0.15
            } else {
                PX * 0.5
            };
            out.push(lay(ts, &p.runs, p.number, indent, x0, width, gap, false));
        }
    }
    out
}

/// Piece of a paragraph placed on a page: lines [from, to) at ordinate y.
struct Placed {
    item: usize,
    from: usize,
    to: usize,
    y: f32,
}

fn paginate(items: &[Laid], avail: f32) -> Vec<Vec<Placed>> {
    let mut pages: Vec<Vec<Placed>> = vec![Vec::new()];
    let mut y = 0.0f32;
    for (k, it) in items.iter().enumerate() {
        let lh = it.st.px * it.st.line_height;
        if y > 0.0 {
            y += it.gap;
        }
        let mut i = 0;
        while i < it.lines.len() {
            let fit = ((avail - y) / lh).floor().max(0.0) as usize;
            // No lone sense number at the bottom of a page: it goes with its continuation.
            let fit = if fit == 1 && i == 0 && it.lines.len() > 1 {
                0
            } else {
                fit
            };
            if fit == 0 {
                if pages.last().map(|p| p.is_empty()).unwrap_or(true) {
                    // Empty page and nothing fits: place one line anyway.
                    pages.last_mut().unwrap().push(Placed {
                        item: k,
                        from: i,
                        to: i + 1,
                        y,
                    });
                    i += 1;
                }
                pages.push(Vec::new());
                y = 0.0;
                continue;
            }
            let n = fit.min(it.lines.len() - i);
            pages.last_mut().unwrap().push(Placed {
                item: k,
                from: i,
                to: i + n,
                y,
            });
            y += n as f32 * lh;
            i += n;
            if i < it.lines.len() {
                pages.push(Vec::new());
                y = 0.0;
            }
        }
    }
    if pages.len() > 1 && pages.last().map(|p| p.is_empty()).unwrap_or(false) {
        pages.pop();
    }
    pages
}

/// Total height of a run of paragraphs laid out as one block.
fn total_height(items: &[Laid]) -> f32 {
    items
        .iter()
        .enumerate()
        .map(|(k, it)| {
            it.lines.len() as f32 * it.st.px * it.st.line_height + if k > 0 { it.gap } else { 0.0 }
        })
        .sum()
}

fn baseline(l: &Line) -> f32 {
    l.glyphs.first().map(|g| g.y).unwrap_or(0.0)
}

/// Draws the definition window over the page.
pub fn draw(c: &mut Canvas, ts: &mut Typesetter, v: &View) -> Drawn {
    let w = c.width as i32;
    let h = c.height as i32;
    let (x, right) = (MARGIN, w - MARGIN);
    let mut links: Vec<String> = Vec::new();
    let max_h = h * 80 / 100;
    let min_h = h * 34 / 100;

    let head = v.entries.first();
    let title = head.map(|e| e.headword.as_str()).unwrap_or(v.word);
    let has_sub = head
        .map(|e| e.pronunciation.is_some() || e.category.is_some())
        .unwrap_or(false);
    let content_off = if has_sub { 126 } else { 100 };
    let items = lay_entries(ts, v.entries, x, right - x);

    // Panel height: that of the entry, up to 80% of the screen; beyond that, pages.
    let whole = total_height(&items);
    let single = content_off as f32 + whole + 28.0 <= max_h as f32;
    let panel_h = if v.entries.is_empty() {
        (content_off
            + 70
            + if v.suggestions.is_empty() {
                0
            } else {
                60 + 64 * v.suggestions.len().div_ceil(3) as i32
            }
            + 40)
            .clamp(min_h, max_h)
    } else if single {
        (content_off + whole.ceil() as i32 + 28).clamp(min_h, max_h)
    } else {
        max_h
    };
    let (py, mut zones) = ui::sheet(c, ts, "", panel_h, Hit::CloseDefinition);

    // Header: "‹" after a followed cross-reference, the word, then pronunciation and category.
    let mut tx = x;
    if v.back.is_some() {
        icons::draw(c, ts, Icon::ChevronLeft, x + 10, py + 52, 28.0, INK);
        tx = x + 36;
    }
    let shown = ui::ellipsize(ts, title, Style::Bold, PX_SECTION, (right - tx - 60) as f32);
    let tw = ts.draw_text(c, &shown, tx, py + 62, Style::Bold, PX_SECTION, INK) as i32;
    if v.back.is_some() {
        zones.push(HitZone {
            x: 0,
            y: py + 10,
            w: tx + tw + 20,
            h: 80,
            hit: Hit::DictBack,
        });
    }
    if let Some(e) = head {
        let mut sx = tx;
        if let Some(p) = &e.pronunciation {
            // The phonetic alphabet (ʀ, ɔ, ʃ…) is missing from Literata: Source Sans has it.
            let ui_family = ts.set_family(1);
            sx += ts.draw_text(
                c,
                &format!("[{p}]"),
                sx,
                py + 98,
                Style::Regular,
                PX_SMALL,
                GRAY_DARK,
            ) as i32
                + 14;
            ts.set_family(ui_family);
        }
        if let Some(cat) = &e.category {
            ts.draw_text(c, cat, sx, py + 98, Style::Italic, PX_SMALL, GRAY_DARK);
        }
    }

    let top = (py + content_off) as f32;
    if v.entries.is_empty() {
        let msg = if v.missing {
            "Aucun dictionnaire sur la liseuse."
        } else {
            "Pas d'entrée dans le dictionnaire."
        };
        ts.draw_text(
            c,
            msg,
            x,
            top as i32 + 10,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
        if v.missing {
            ts.draw_text(
                c,
                "Copiez dicthtml-fr.zip dans .kobo/dict par USB.",
                x,
                top as i32 + 50,
                Style::Regular,
                PX_SMALL,
                GRAY_DARK,
            );
        }
        if !v.suggestions.is_empty() {
            ts.draw_text(
                c,
                "Mots proches",
                x,
                top as i32 + 64,
                Style::Bold,
                PX_SMALL,
                GRAY_DARK,
            );
            // Tap targets, at most three per row, as wide as their word.
            let (mut cx, mut cy) = (x, top as i32 + 84);
            for s in v.suggestions {
                let sw = ts.font(Style::Regular).measure(s, PX_BODY) as i32 + 36;
                if cx + sw > right {
                    cx = x;
                    cy += 64;
                }
                ui::key_box(c, cx, cy, sw, 50, WHITE);
                ui::key_text(c, ts, cx, cy, sw, 50, s, Style::Regular, PX_BODY, INK);
                zones.push(HitZone {
                    x: cx,
                    y: cy - 6,
                    w: sw,
                    h: 62,
                    hit: Hit::DictLink(links.len()),
                });
                links.push(s.clone());
                cx += sw + 12;
            }
        }
        return Drawn {
            zones,
            rect: Rect::new(0, 0, w as u32, h as u32),
            pages: 1,
            links,
        };
    }

    let avail = (h - FOOT) as f32 - top;
    let pages = if single {
        vec![(0..items.len())
            .map(|k| Placed {
                item: k,
                from: 0,
                to: items[k].lines.len(),
                y: 0.0,
            })
            .collect::<Vec<_>>()]
    } else {
        paginate(&items, avail)
    };
    // Ordinates of the paragraphs of a single-block page.
    let pages = if single {
        let mut y = 0.0;
        let placed: Vec<Placed> = items
            .iter()
            .enumerate()
            .map(|(k, it)| {
                if k > 0 {
                    y += it.gap;
                }
                let p = Placed {
                    item: k,
                    from: 0,
                    to: it.lines.len(),
                    y,
                };
                y += it.lines.len() as f32 * it.st.px * it.st.line_height;
                p
            })
            .collect();
        vec![placed]
    } else {
        pages
    };
    let page = v.page.min(pages.len() - 1);
    let mut link_zones = Vec::new();
    for p in &pages[page] {
        let it = &items[p.item];
        let t = top + p.y;
        if it.rule && p.from == 0 {
            ui_rule(c, x, (t - it.gap * 0.5) as i32, right - x);
        }
        let Some(first) = it.lines.get(p.from) else {
            continue;
        };
        // Line `from` placed at the top of the piece, like the first line of the paragraph.
        let y0 = t + baseline(&it.lines[0]) - baseline(first);
        ts.draw(
            c,
            &it.lines[p.from..p.to],
            it.x,
            y0.round() as i32,
            it.st,
            INK,
        );
        if p.from == 0 {
            if let Some(n) = it.number {
                ts.draw_text(
                    c,
                    &n.to_string(),
                    x,
                    (t + baseline(&it.lines[0])).round() as i32,
                    Style::Bold,
                    PX,
                    INK,
                );
            }
        }
        // Cross-references of this piece: one rectangle per line.
        for (from, to, target) in &it.links {
            for line in &it.lines[p.from..p.to] {
                let gs: Vec<_> = line
                    .glyphs
                    .iter()
                    .filter(|g| g.ch >= *from && g.ch < *to)
                    .collect();
                let (Some(a), Some(b)) = (gs.first(), gs.last()) else {
                    continue;
                };
                let by = (y0 + b.y).round() as i32;
                let (gx0, gx1) = (it.x + a.x.round() as i32, it.x + (b.x + b.w).round() as i32);
                link_zones.push(HitZone {
                    x: gx0 - 6,
                    y: by - (PX * 0.85) as i32,
                    w: gx1 - gx0 + 12,
                    h: (PX * 1.25) as i32,
                    hit: Hit::DictLink(links.len()),
                });
            }
            links.push(target.clone());
        }
    }
    // Cross-references first, then the halves of the text that turn pages.
    zones.extend(link_zones);
    if pages.len() > 1 {
        let label = format!("{} / {}", page + 1, pages.len());
        let lw = ts.font(Style::Regular).measure(&label, 20.0) as i32;
        ts.draw_text(
            c,
            &label,
            (w - lw) / 2,
            h - 28,
            Style::Regular,
            20.0,
            GRAY_DARK,
        );
        ui::pager(c, ts, page, pages.len(), &mut zones, Hit::DictPage);
        let (zy, zh) = (top as i32, h - FOOT - top as i32);
        if page > 0 {
            zones.push(HitZone {
                x: 0,
                y: zy,
                w: w / 2,
                h: zh,
                hit: Hit::DictPage(page - 1),
            });
        }
        if page + 1 < pages.len() {
            zones.push(HitZone {
                x: w / 2,
                y: zy,
                w: w / 2,
                h: zh,
                hit: Hit::DictPage(page + 1),
            });
        }
    }
    Drawn {
        zones,
        rect: Rect::new(0, 0, w as u32, h as u32),
        pages: pages.len(),
        links,
    }
}

/// Rule between two homographs.
fn ui_rule(c: &mut Canvas, x: i32, y: i32, w: i32) {
    crate::draw::hline(c, x, y, w, GRAY_LIGHT);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On the real dictionary (SIGNET_DICT): a short entry fits in one block,
    /// a long one is paginated, cross-references touch, a misspelled word suggests
    /// near-miss words. Previews: SIGNET_UI_DUMP=directory.
    #[test]
    fn real_entries() {
        let Ok(path) = std::env::var("SIGNET_DICT") else {
            return;
        };
        let mut d = signet_dict::Dictionary::open(&path).unwrap();
        let mut ts = crate::typesetter();
        let dump = std::env::var("SIGNET_UI_DUMP").ok();
        for (word, page, back) in [
            ("concentration", 0, None),
            ("temps", 0, None),
            ("temps", 1, None),
            ("lente", 0, Some("lent")),
            ("aide", 0, None),
            ("concentation", 0, None),
        ] {
            let entries = d.lookup_rich(word, 4);
            let sugg = if entries.is_empty() {
                d.suggestions(word, 6)
            } else {
                Vec::new()
            };
            let mut c = Canvas::new(758, 1024);
            c.clear(WHITE);
            let v = View {
                word,
                entries: &entries,
                suggestions: &sugg,
                missing: false,
                page,
                back,
            };
            let drawn = draw(&mut c, &mut ts, &v);
            match word {
                "concentration" => {
                    assert!(
                        drawn.links.iter().any(|l| l == "rassemblement"),
                        "{:?}",
                        drawn.links
                    );
                    assert!(drawn
                        .zones
                        .iter()
                        .any(|z| matches!(z.hit, Hit::DictLink(_))));
                }
                "temps" => assert!(drawn.pages > 1, "« temps » tient sur plusieurs pages"),
                "aide" => assert!(entries.len() >= 2, "homographes"),
                "concentation" => assert!(
                    drawn.links.iter().any(|l| l == "concentration"),
                    "{:?}",
                    drawn.links
                ),
                _ => {}
            }
            assert_eq!(ts.family(), 0, "l'interface garde sa police");
            if let Some(dir) = &dump {
                c.save_pgm(&format!("{dir}/dico-{word}-{page}.pgm"))
                    .unwrap();
            }
        }
    }
}
