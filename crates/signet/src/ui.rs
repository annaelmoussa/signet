//! The three mock-up screens: Home, Reading, Quick Settings.
//! Each function draws into the canvas and returns its touch zones.

use crate::draw::{self, Gray};
use crate::icons::{self, Icon};
use crate::library::BookEntry;
use crate::reader::{ItemKind, Page, PageConfig};
use crate::theme::*;
use crate::vector::{self, Path};
use signet_fb::{Canvas, Rect};
use signet_text::{Align, ParagraphStyle, Style, Typesetter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Resume,
    OpenBook(usize),
    SeeAll,
    PrevPage,
    NextPage,
    ToggleSettings,
    CloseSettings,
    SetSize(usize),
    SetMargins(usize),
    SetLeading(usize),
    SetInk(usize),
    SetLight(u8),
    Toc,
    Annotations,
    Home,
    GoToChapter(usize),
    TocPage(usize),
    Back,
    LibraryPage(usize),
    ToggleDevMode,
    /// "Lecture" title of the panel: a long press (2 s) toggles dev mode,
    /// with no visible button in reading mode; a simple tap does nothing.
    DevSecret,
    Highlight,
    CancelSelection,
    DeleteHighlight(usize),
    GoToAnnotation(usize),
    AnnotationsPage(usize),
    ToggleAutoLight,
    ToggleFastPages,
    Define,
    CloseDefinition,
    /// Link in the text (index into the chapter's links).
    FollowLink(usize),
    /// "Revenir à la page…" button after a followed link.
    LinkBack,
    CloseNote,
    /// "Aller à la note": the note in the book.
    GoToNote,
    /// Table-of-contents entry (index into the book's table of contents).
    GoToToc(usize),
    /// Places or removes the bookmark of the displayed page.
    ToggleBookmark,
    /// Night mode (white on black).
    ToggleNight,
    /// Reading font (index into `theme::FAMILIES`).
    SetFont(usize),
    /// Cross-reference touched in a definition (index into the window's cross-references).
    DictLink(usize),
    /// Page of a long definition.
    DictPage(usize),
    /// Return to the word before a cross-reference.
    DictBack,
    /// Library order (index into `LIBRARY_SORTS`).
    LibrarySort(usize),
    /// Menu of a book (index into the library).
    BookMenu(usize),
    ToggleFinished(usize),
    /// "Supprimer de la liseuse": asks for confirmation.
    AskDelete(usize),
    ConfirmDelete(usize),
    CloseBookMenu,
    /// Opens the "Aller à la page" keypad.
    GoToPage,
    /// Opens the search in the book.
    Search,
    /// Character key (search keyboard, numeric keypad).
    Key(char),
    KeyErase,
    /// "OK" / "Rechercher".
    KeyEnter,
    /// Closes the keypad or the keyboard without doing anything.
    CloseKeypad,
    /// The search field: reopens the keyboard.
    SearchField,
    SearchResult(usize),
    SearchPage(usize),
}

/// Highlight to draw: (block, from, to, gray).
pub type Mark = (usize, u32, u32, u8);

/// Action bar at the bottom of the page during a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionBar {
    /// New selection: "Surligner" / "Annuler".
    New,
    /// Existing highlight touched: "Supprimer" / "Fermer".
    Existing(usize),
}

#[derive(Clone, Debug)]
pub struct HitZone {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub hit: Hit,
}

impl HitZone {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

pub struct Status<'a> {
    pub title: &'a str,
    pub clock: &'a str,
    pub battery: Option<u8>,
    /// Power plugged in (charger or computer): a lightning bolt near the battery.
    pub charging: bool,
}

/// Common status bar: title on the left, time + battery on the right.
pub fn status_bar(c: &mut Canvas, ts: &mut Typesetter, s: &Status, small: bool) {
    let w = c.width as i32;
    let px = if small { 20.0 } else { PX_STATUS };
    let base = STATUS_H / 2 + (px * 0.35) as i32;
    // A title that leads back starts with "‹": a drawn chevron.
    let color = if small { GRAY_DARK } else { INK };
    match s.title.strip_prefix('‹') {
        Some(rest) => {
            icons::draw(
                c,
                ts,
                Icon::ChevronLeft,
                MARGIN + 8,
                STATUS_H / 2,
                px + 2.0,
                color,
            );
            ts.draw_text(
                c,
                rest.trim_start(),
                MARGIN + 26,
                base,
                Style::Regular,
                px,
                color,
            );
        }
        None => {
            ts.draw_text(c, s.title, MARGIN, base, Style::Regular, px, color);
        }
    }
    status_right(c, ts, s.clock, s.battery, s.charging, small);
    if !small {
        draw::hline(c, MARGIN, STATUS_H, w - 2 * MARGIN, GRAY_LIGHT);
    }
}

/// Time and battery, on the right of the status bar; returns the left edge of the time.
fn status_right(
    c: &mut Canvas,
    ts: &mut Typesetter,
    clock: &str,
    battery: Option<u8>,
    charging: bool,
    small: bool,
) -> i32 {
    let w = c.width as i32;
    let px = if small { 20.0 } else { PX_STATUS };
    let base = STATUS_H / 2 + (px * 0.35) as i32;
    // Battery: rounded outline, terminal, level; the bolt is inside while charging.
    let bx = w - MARGIN - icons::BATTERY_W;
    icons::battery(c, bx, STATUS_H / 2 - 8, battery, charging, INK);
    let left = bx - 12 - ts.font(Style::Regular).measure(clock, px) as i32;
    ts.draw_text(c, clock, left, base, Style::Regular, px, INK);
    left
}

/// Redraws only the time and the battery (the time has changed): erases the
/// area of the old time (`old_clock`) and of the new one, returns the rectangle
/// to refresh. The title, on the left, is not touched.
pub fn refresh_status_right(
    c: &mut Canvas,
    ts: &mut Typesetter,
    now: &Status,
    old: &Status,
    small: bool,
) -> Rect {
    let w = c.width as i32;
    let px = if small { 20.0 } else { PX_STATUS };
    let bx = w - MARGIN - icons::BATTERY_W;
    let font = ts.font(Style::Regular);
    let extent = |s: &Status| font.measure(s.clock, px) as i32;
    let left = (bx - 12 - extent(now).max(extent(old)) - 4).max(0);
    let r = Rect::new(left as u32, 0, (w - left) as u32, (STATUS_H - 2) as u32);
    draw::rect(c, r.x as i32, r.y as i32, r.w as i32, r.h as i32, WHITE);
    status_right(c, ts, now.clock, now.battery, now.charging, small);
    r
}

/// Screen 01 — Home.
pub fn home(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    current: Option<&BookEntry>,
    library: &[BookEntry],
) -> Vec<HitZone> {
    let w = c.width as i32;
    let mut zones = Vec::new();
    c.clear(WHITE);
    status_bar(c, ts, status, false);

    // --- "Current book" card ------------------------------------------
    let card_y = STATUS_H + 36;
    let cover_w = 230;
    let cover_h = 345;
    let text_x = MARGIN + cover_w + 32;
    let text_w = w - MARGIN - text_x;
    // Bottom of the card: the cover, or the button if a long title pushes it away.
    let mut card_bottom = card_y + cover_h;
    if let Some(book) = current {
        cover_box(c, book.cover.as_ref(), MARGIN, card_y, cover_w, cover_h);
        ts.draw_text(
            c,
            "Le livre en cours",
            text_x,
            card_y + 26,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        let st = ParagraphStyle {
            style: Style::Regular,
            px: PX_TITLE,
            line_height: 1.15,
            align: Align::Left,
            indent: 0.0,
        };
        let (lines, h) = ts.layout(&book.title, text_w as f32, st);
        let lines: Vec<_> = lines.into_iter().take(3).collect();
        let h = h.min(3.0 * PX_TITLE * 1.15);
        ts.draw(c, &lines, text_x, card_y + 44, st, INK);
        let author_y = card_y + 44 + h as i32 + 30;
        ts.draw_text(
            c,
            &book.author,
            text_x,
            author_y,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
        // progress
        let bar_y = author_y + 48;
        // Progress: the bar, then the page (as in the book's page footer)
        // and the percentage.
        draw::rounded(c, text_x, bar_y, text_w, 8, 4, GRAY_LIGHT);
        draw::rounded(
            c,
            text_x,
            bar_y,
            (text_w * book.percent as i32 / 100).max(8),
            8,
            4,
            INK,
        );
        let pct = format!("{} %", book.percent);
        let pw = ts.font(Style::Regular).measure(&pct, PX_SMALL) as i32;
        ts.draw_text(
            c,
            &pct,
            text_x + text_w - pw,
            bar_y + 38,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        if let Some((page, total)) = book.pages {
            ts.draw_text(
                c,
                &format!("Page {page} sur {total}"),
                text_x,
                bar_y + 38,
                Style::Regular,
                PX_SMALL,
                GRAY_DARK,
            );
        }
        // button
        let btn_y = bar_y + 62;
        let btn_h = 60;
        draw::rounded(c, text_x, btn_y, text_w, btn_h, 10, INK);
        let label = if book.started {
            "Reprendre la lecture"
        } else {
            "Commencer la lecture"
        };
        let lw = ts.font(Style::Regular).measure(label, PX_BUTTON);
        ts.draw_text(
            c,
            label,
            text_x + (text_w - lw as i32) / 2,
            btn_y + btn_h / 2 + 8,
            Style::Regular,
            PX_BUTTON,
            WHITE,
        );
        zones.push(HitZone {
            x: text_x,
            y: btn_y,
            w: text_w,
            h: btn_h,
            hit: Hit::Resume,
        });
        zones.push(HitZone {
            x: MARGIN,
            y: card_y,
            w: cover_w,
            h: cover_h,
            hit: Hit::Resume,
        });
        card_bottom = card_bottom.max(btn_y + btn_h);
    } else {
        ts.draw_text(
            c,
            "Aucun livre en cours",
            text_x,
            card_y + 60,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
        ts.draw_text(
            c,
            "Choisissez un livre ci-dessous.",
            text_x,
            card_y + 100,
            Style::Regular,
            PX_SMALL,
            GRAY,
        );
    }

    // --- Library -------------------------------------------------------
    let sec_y = card_bottom + 44;
    draw::hline(c, MARGIN, sec_y - 22, w - 2 * MARGIN, GRAY_LIGHT);
    ts.draw_text(
        c,
        "Bibliothèque",
        MARGIN,
        sec_y + 14,
        Style::Bold,
        PX_SECTION,
        INK,
    );
    let see = "Tout voir";
    let sw = ts.font(Style::Regular).measure(see, PX_SMALL) as i32;
    ts.draw_text(
        c,
        see,
        w - MARGIN - 22 - sw,
        sec_y + 12,
        Style::Regular,
        PX_SMALL,
        GRAY_DARK,
    );
    icons::draw(
        c,
        ts,
        Icon::ChevronRight,
        w - MARGIN - 8,
        sec_y + 4,
        22.0,
        GRAY_DARK,
    );
    zones.push(HitZone {
        x: w - MARGIN - sw - 42,
        y: sec_y - 20,
        w: sw + 60,
        h: 50,
        hit: Hit::SeeAll,
    });

    let row_h = 120;
    let mut y = sec_y + 40;
    let thumb_w = 68;
    let thumb_h = 98;
    for (i, book) in library.iter().enumerate() {
        if current.map(|b| b.path == book.path).unwrap_or(false) {
            continue;
        }
        if y + row_h > c.height as i32 - 10 {
            break;
        }
        cover_box(c, book.cover.as_ref(), MARGIN, y + 11, thumb_w, thumb_h);
        let tx = MARGIN + thumb_w + 24;
        let right_label = if book.finished {
            "Terminé".to_string()
        } else if !book.started {
            "Non commencé".to_string()
        } else {
            format!("{} %", book.percent)
        };
        let rw = ts.font(Style::Regular).measure(&right_label, PX_SMALL) as i32;
        let title_w = w - MARGIN - tx - rw - 24;
        let title = ellipsize(ts, &book.title, Style::Regular, PX_BODY, title_w as f32);
        ts.draw_text(c, &title, tx, y + 50, Style::Regular, PX_BODY, INK);
        let author = ellipsize(ts, &book.author, Style::Regular, PX_SMALL, title_w as f32);
        ts.draw_text(c, &author, tx, y + 82, Style::Regular, PX_SMALL, GRAY_DARK);
        ts.draw_text(
            c,
            &right_label,
            w - MARGIN - rw,
            y + 64,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        zones.push(HitZone {
            x: MARGIN,
            y,
            w: w - 2 * MARGIN,
            h: row_h,
            hit: Hit::OpenBook(i),
        });
        y += row_h;
        draw::hline(c, tx, y, w - MARGIN - tx, GRAY_FAINT);
    }
    // No book other than the current one: say so, rather than an empty section.
    if y == sec_y + 40 {
        icons::draw(c, ts, Icon::Library, MARGIN + 18, y + 54, 36.0, GRAY);
        ts.draw_text(
            c,
            "Vos autres livres apparaîtront ici.",
            MARGIN + 52,
            y + 46,
            Style::Italic,
            PX_SMALL,
            GRAY_DARK,
        );
        ts.draw_text(
            c,
            "Copiez des EPUB sur la liseuse par USB.",
            MARGIN + 52,
            y + 78,
            Style::Regular,
            20.0,
            GRAY,
        );
    }
    zones
}

fn cover_box(c: &mut Canvas, cover: Option<&Gray>, x: i32, y: i32, w: i32, h: i32) {
    match cover {
        Some(g) => {
            let g = g.fit(w as u32, h as u32);
            let ox = x + (w - g.width as i32) / 2;
            let oy = y + (h - g.height as i32) / 2;
            g.blit(c, ox, oy);
            draw::frame(c, ox, oy, g.width as i32, g.height as i32, 1, GRAY_LIGHT);
        }
        None => {
            draw::rect(c, x, y, w, h, GRAY_FAINT);
            draw::frame(c, x, y, w, h, 1, GRAY_LIGHT);
        }
    }
}

pub(crate) fn ellipsize(ts: &Typesetter, text: &str, style: Style, px: f32, max_w: f32) -> String {
    let f = ts.font(style);
    if f.measure(text, px) <= max_w {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() {
        s.pop();
        let t = format!("{}…", s.trim_end());
        if f.measure(&t, px) <= max_w {
            return t;
        }
    }
    "…".into()
}

/// Screen 02 — Reading: a composed page.
/// What surrounds the page: its number, the return button after a followed
/// link, the bookmark.
#[derive(Clone, Copy, Default)]
pub struct Chrome<'a> {
    pub label: &'a str,
    pub back: Option<&'a str>,
    pub bookmarked: bool,
    /// Night mode: the page in white on black, images included as they are.
    pub night: bool,
}

/// Callout background dark enough for its text to be written in white.
fn dark_background(g: u8) -> bool {
    g < 128
}

/// Bookmark ribbon: rectangle with a V-shaped notch.
fn ribbon(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, gray: u8) {
    vector::fill(
        c,
        &Path::new().polygon(
            &[
                (x, y),
                (x + w, y),
                (x + w, y + h),
                (x + w / 2.0, y + h - w * 0.45),
                (x, y + h),
            ],
            false,
        ),
        gray,
    );
}

/// Top-right corner of the page that places or removes the bookmark (as on Kobo).
pub const BOOKMARK_CORNER: i32 = 100;

#[allow(clippy::too_many_arguments)]
pub fn reader(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    page: &Page,
    cfg: &PageConfig,
    chrome: &Chrome,
    marks: &[Mark],
    bar: Option<SelectionBar>,
) -> Vec<HitZone> {
    let (page_label, back) = (chrome.label, chrome.back);
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    status_bar(c, ts, status, true);
    let x0 = cfg.margin_x;
    let y0 = cfg.margin_top;
    // Callouts: background and border, under everything else (edges clipped at the bottom of the text area).
    let text_bottom = y0 + cfg.text_height() as i32;
    for f in &page.frames {
        let fy0 = y0 + f.y0.round() as i32;
        let fy1 = (y0 + f.y1.round() as i32).min(text_bottom + 4);
        draw::frame_box(
            c,
            x0 + f.x0.round() as i32,
            fy0,
            (f.x1 - f.x0).round() as i32,
            fy1 - fy0,
            f.border,
            f.border_gray,
            f.sides,
            f.background,
            f.radius,
            !f.open_top,
            !f.open_bottom,
        );
    }
    // Highlight backgrounds, under the text.
    for item in &page.items {
        if item.kind != ItemKind::Text {
            continue;
        }
        for &(block, from, to, gray) in marks {
            if block != item.block {
                continue;
            }
            let asc = (item.style.px * 0.78) as i32;
            let desc = (item.style.px * 0.26) as i32;
            for line in &item.lines {
                let mut run: Option<(i32, i32, i32)> = None; // (x start, x end, baseline)
                for g in &line.glyphs {
                    let inside = g.ch >= from && g.ch < to;
                    let gx = x0 + item.x as i32 + g.x.round() as i32;
                    let gw = g.w.round() as i32;
                    let by = y0 + item.y as i32 + g.y.round() as i32;
                    match (inside, run) {
                        (true, None) => run = Some((gx, gx + gw, by)),
                        (true, Some((a, _, b))) => run = Some((a, gx + gw, b)),
                        (false, Some((a, bx, by))) => {
                            draw::rect(c, a, by - asc, bx - a, asc + desc, gray);
                            run = None;
                        }
                        _ => {}
                    }
                }
                if let Some((a, bx, by)) = run {
                    draw::rect(c, a, by - asc, bx - a, asc + desc, gray);
                }
            }
        }
    }
    // The book's text in its font (the interface keeps its own).
    let ui_family = ts.set_family(cfg.family);
    for item in &page.items {
        match &item.kind {
            ItemKind::Separator => {
                let cx = w / 2;
                draw::hline(
                    c,
                    cx - 40,
                    y0 + item.y as i32 + (item.style.px * 0.5) as i32,
                    80,
                    GRAY,
                );
            }
            ItemKind::ImagePlaceholder { .. } => {
                ts.draw(
                    c,
                    &item.lines,
                    x0 + item.x as i32,
                    y0 + item.y as i32,
                    item.style,
                    GRAY_DARK,
                );
            }
            ItemKind::Image(img) => {
                // Within the block's width (the column, or the inside of a callout):
                // left or right if its paragraph asks for it, centered otherwise.
                let free = item.width as i32 - img.width as i32;
                let dx = match item.style.align {
                    Align::Left => 0,
                    Align::Right => free,
                    _ => free / 2,
                };
                if let Some(g) = img.pixels() {
                    g.blit(c, x0 + item.x as i32 + dx, y0 + item.y as i32);
                }
            }
            ItemKind::Text => {
                // In a callout with a dark background (the book's "white on black"), the text is white.
                let dark = page.frames.iter().any(|f| {
                    f.background.map(dark_background).unwrap_or(false)
                        && item.y >= f.y0 - 1.0
                        && item.y < f.y1
                        && item.x + 1.0 >= f.x0
                });
                ts.draw(
                    c,
                    &item.lines,
                    x0 + item.x as i32,
                    y0 + item.y as i32,
                    item.style,
                    if dark { WHITE } else { BLACK },
                );
            }
        }
    }
    ts.set_family(ui_family);
    // Discreet pagination, or the return button after a followed link (in its place,
    // as in Apple Books).
    let mut back_zone = None;
    match back {
        Some(label) => {
            let lw = ts.font(Style::Regular).measure(label, PX_SMALL) as i32;
            let (bw, bh) = (lw + 24 + 10 + 40, 44);
            let (bx, by) = ((w - bw) / 2, h - bh - 10);
            let (fx, fy) = (bx as f32, by as f32);
            vector::fill(
                c,
                &Path::new().rounded_rect(fx, fy, bw as f32, bh as f32, bh as f32 / 2.0, false),
                WHITE,
            );
            vector::fill(
                c,
                &Path::new().rounded_frame(fx, fy, bw as f32, bh as f32, bh as f32 / 2.0, 2.0),
                INK,
            );
            icons::draw(
                c,
                ts,
                Icon::ChevronLeft,
                bx + 20 + 12,
                by + bh / 2,
                24.0,
                INK,
            );
            ts.draw_text(
                c,
                label,
                bx + 20 + 24 + 10,
                by + bh / 2 + 8,
                Style::Regular,
                PX_SMALL,
                INK,
            );
            back_zone = Some(HitZone {
                x: bx - 20,
                y: by - 14,
                w: bw + 40,
                h: bh + 24,
                hit: Hit::LinkBack,
            });
        }
        None => {
            let lw = ts.font(Style::Regular).measure(page_label, 20.0) as i32;
            ts.draw_text(
                c,
                page_label,
                (w - lw) / 2,
                h - 28,
                Style::Regular,
                20.0,
                GRAY_DARK,
            );
        }
    }
    // Bookmark: a ribbon in the right margin, under the status bar (whose
    // time corner is redrawn alone every minute, without it).
    if chrome.bookmarked {
        ribbon(c, (w - 36) as f32, (STATUS_H - 2) as f32, 22.0, 46.0, INK);
    }
    // Night mode: the whole screen inverted, then the images flipped back the right way.
    if chrome.night {
        c.invert_rect(signet_fb::Rect::new(0, 0, w as u32, h as u32));
        for item in &page.items {
            if let ItemKind::Image(img) = &item.kind {
                let free = item.width as i32 - img.width as i32;
                let dx = match item.style.align {
                    Align::Left => 0,
                    Align::Right => free,
                    _ => free / 2,
                };
                let (ix, iy) = (x0 + item.x as i32 + dx, y0 + item.y as i32);
                c.invert_rect(signet_fb::Rect::new(
                    ix.max(0) as u32,
                    iy.max(0) as u32,
                    img.width,
                    img.height,
                ));
            }
        }
    }

    if let Some(bar) = bar {
        // Action bar: two buttons at the bottom of the page, over the pagination.
        let bh = 64;
        let by = h - bh - 8;
        let (bxf, byf, bwf) = (MARGIN as f32, by as f32, (w - 2 * MARGIN) as f32);
        vector::fill(
            c,
            &Path::new().rounded_rect(bxf, byf, bwf, bh as f32, 14.0, false),
            WHITE,
        );
        vector::fill(
            c,
            &Path::new().rounded_frame(bxf, byf, bwf, bh as f32, 14.0, 2.0),
            INK,
        );
        let buttons: Vec<(Icon, &str, Style, Hit)> = match bar {
            SelectionBar::New => vec![
                (Icon::Highlighter, "Surligner", Style::Bold, Hit::Highlight),
                (Icon::Dictionary, "Définition", Style::Regular, Hit::Define),
                (Icon::Close, "Annuler", Style::Regular, Hit::CancelSelection),
            ],
            SelectionBar::Existing(i) => vec![
                (
                    Icon::Trash,
                    "Supprimer",
                    Style::Bold,
                    Hit::DeleteHighlight(i),
                ),
                (Icon::Dictionary, "Définition", Style::Regular, Hit::Define),
                (Icon::Close, "Fermer", Style::Regular, Hit::CancelSelection),
            ],
        };
        let n = buttons.len() as i32;
        let cell = (w - 2 * MARGIN) / n;
        let mut zones = Vec::new();
        for (k, (icon, label, style, hit)) in buttons.into_iter().enumerate() {
            let bx = MARGIN + k as i32 * cell;
            if k > 0 {
                draw::rect(c, bx, by + 12, 1, bh - 24, GRAY_LIGHT);
            }
            // Icon then label, centered together in the cell.
            let lw = ts.font(style).measure(label, PX_SMALL) as i32;
            let gw = 24 + 10 + lw;
            let gx = bx + (cell - gw) / 2;
            icons::draw(c, ts, icon, gx + 12, by + bh / 2, 24.0, INK);
            ts.draw_text(c, label, gx + 34, by + bh / 2 + 8, style, PX_SMALL, INK);
            zones.push(HitZone {
                x: bx,
                y: by,
                w: cell,
                h: bh,
                hit,
            });
        }
        zones.push(HitZone {
            x: 0,
            y: 0,
            w,
            h: STATUS_H,
            hit: Hit::CancelSelection,
        });
        return zones;
    }

    back_zone
        .into_iter()
        .chain([
            HitZone {
                x: w - BOOKMARK_CORNER,
                y: STATUS_H,
                w: BOOKMARK_CORNER,
                h: BOOKMARK_CORNER,
                hit: Hit::ToggleBookmark,
            },
            HitZone {
                x: 0,
                y: STATUS_H,
                w: w / 3,
                h: h - STATUS_H,
                hit: Hit::PrevPage,
            },
            HitZone {
                x: 2 * w / 3,
                y: STATUS_H,
                w: w / 3,
                h: h - STATUS_H,
                hit: Hit::NextPage,
            },
            HitZone {
                x: w / 3,
                y: STATUS_H,
                w: w / 3,
                h: h - STATUS_H,
                hit: Hit::ToggleSettings,
            },
            HitZone {
                x: 0,
                y: 0,
                w,
                h: STATUS_H,
                hit: Hit::Home,
            },
        ])
        .collect()
}

/// Key of the keyboard or keypad: rounded background, gray border on a white background.
pub(crate) fn key_box(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, fill: u8) {
    let (fx, fy, fw, fh) = (x as f32, y as f32, w as f32, h as f32);
    vector::fill(
        c,
        &Path::new().rounded_rect(fx, fy, fw, fh, 10.0, false),
        fill,
    );
    if fill == WHITE {
        vector::fill(
            c,
            &Path::new().rounded_frame(fx, fy, fw, fh, 10.0, 1.5),
            GRAY,
        );
    }
}

/// Label centered in a key.
#[allow(clippy::too_many_arguments)]
pub(crate) fn key_text(
    c: &mut Canvas,
    ts: &mut Typesetter,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &str,
    style: Style,
    px: f32,
    gray: u8,
) {
    let tw = ts.font(style).measure(text, px) as i32;
    ts.draw_text(
        c,
        text,
        x + (w - tw) / 2,
        y + h / 2 + (px * 0.36) as i32,
        style,
        px,
        gray,
    );
}

/// Bottom panel common to the keypad and the note: overlay on the page, white
/// rounded background, handle, title, close cross (zone returned).
pub(crate) fn sheet(
    c: &mut Canvas,
    ts: &mut Typesetter,
    title: &str,
    panel_h: i32,
    close: Hit,
) -> (i32, Vec<HitZone>) {
    let w = c.width as i32;
    let h = c.height as i32;
    let py = h - panel_h;
    draw::dim(c, 0, 0, w, py, 90);
    draw::rounded(c, 0, py, w, panel_h + 24, 24, WHITE);
    draw::hline(c, 0, py, w, GRAY_LIGHT);
    draw::rounded(c, w / 2 - 28, py + 12, 56, 6, 3, GRAY_LIGHT);
    ts.draw_text(c, title, MARGIN, py + 62, Style::Bold, PX_SECTION, INK);
    icons::draw(c, ts, Icon::Close, w - MARGIN - 14, py + 50, 30.0, INK);
    let zones = vec![
        HitZone {
            x: w - MARGIN - 60,
            y: py + 10,
            w: 70,
            h: 80,
            hit: close,
        },
        HitZone {
            x: 0,
            y: 0,
            w,
            h: py,
            hit: close,
        },
    ];
    (py, zones)
}

/// "Aller à la page": the input (the current page in gray as long as nothing is
/// typed), the page count, a numeric keypad. Returns the zones, the panel and
/// the field (the only part refreshed at each digit).
pub fn goto_page(
    c: &mut Canvas,
    ts: &mut Typesetter,
    input: &str,
    current: usize,
    total: usize,
) -> (Vec<HitZone>, signet_fb::Rect, signet_fb::Rect) {
    let w = c.width as i32;
    let h = c.height as i32;
    let (_, mut zones) = sheet(c, ts, "Aller à la page", GOTO_H, Hit::CloseKeypad);
    let (x, right) = (MARGIN, w - MARGIN);
    let field = goto_field(c, ts, input, current, total);
    let (fy, fh) = (field.y as i32, field.h as i32);
    let (gap, kh) = (12, 76);
    let kw = (right - x - 2 * gap) / 3;
    let ky = fy + fh + 24;
    for (i, k) in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "⌫", "0", "OK"]
        .iter()
        .enumerate()
    {
        let bx = x + (i % 3) as i32 * (kw + gap);
        let by = ky + (i / 3) as i32 * (kh + gap);
        let hit = match *k {
            "⌫" => {
                key_box(c, bx, by, kw, kh, GRAY_FAINT);
                icons::draw(c, ts, Icon::Backspace, bx + kw / 2, by + kh / 2, 30.0, INK);
                Hit::KeyErase
            }
            "OK" => {
                key_box(c, bx, by, kw, kh, INK);
                key_text(c, ts, bx, by, kw, kh, "OK", Style::Bold, PX_BODY, WHITE);
                Hit::KeyEnter
            }
            d => {
                key_box(c, bx, by, kw, kh, WHITE);
                key_text(c, ts, bx, by, kw, kh, d, Style::Regular, PX_TITLE, INK);
                Hit::Key(d.chars().next().unwrap())
            }
        };
        zones.push(HitZone {
            x: bx - gap / 2,
            y: by - gap / 2,
            w: kw + gap,
            h: kh + gap,
            hit,
        });
    }
    (zones, signet_fb::Rect::new(0, 0, w as u32, h as u32), field)
}

const GOTO_H: i32 = 600;

/// Field of the "Aller à la page" keypad (in the already drawn panel): returns its rectangle.
pub fn goto_field(
    c: &mut Canvas,
    ts: &mut Typesetter,
    input: &str,
    current: usize,
    total: usize,
) -> signet_fb::Rect {
    let w = c.width as i32;
    let h = c.height as i32;
    let (x, right) = (MARGIN, w - MARGIN);
    let (fy, fh) = (h - GOTO_H + 92, 80);
    draw::rect(c, x, fy, right - x, fh, WHITE);
    vector::fill(
        c,
        &Path::new().rounded_frame(
            x as f32,
            fy as f32,
            (right - x) as f32,
            fh as f32,
            12.0,
            2.0,
        ),
        INK,
    );
    let shown = if input.is_empty() {
        current.to_string()
    } else {
        input.to_string()
    };
    ts.draw_text(
        c,
        &shown,
        x + 24,
        fy + 54,
        Style::Bold,
        PX_TITLE,
        if input.is_empty() { GRAY } else { INK },
    );
    let of = format!("sur {total}");
    let ow = ts.font(Style::Regular).measure(&of, PX_BODY) as i32;
    ts.draw_text(
        c,
        &of,
        right - 24 - ow,
        fy + 52,
        Style::Regular,
        PX_BODY,
        GRAY_DARK,
    );
    signet_fb::Rect::new(x as u32, fy as u32, (right - x) as u32, fh as u32)
}

/// Height of the search keyboard.
const KEYBOARD_H: i32 = 5 * 78 + 12;

/// AZERTY keyboard, digits on top, apostrophe and hyphen for French
/// words (no accents: the search ignores them).
fn keyboard(c: &mut Canvas, ts: &mut Typesetter, y0: i32, enter: &str) -> Vec<HitZone> {
    let w = c.width as i32;
    let unit = (w - 12) / 10;
    let (kh, gap) = (70, 8);
    let mut zones = Vec::new();
    for (r, row) in ["1234567890", "azertyuiop", "qsdfghjklm", "wxcvbn'-"]
        .iter()
        .enumerate()
    {
        let y = y0 + r as i32 * (kh + gap);
        for (k, ch) in row.chars().enumerate() {
            let x = 6 + k as i32 * unit;
            key_box(c, x + 3, y, unit - 6, kh, WHITE);
            key_text(
                c,
                ts,
                x + 3,
                y,
                unit - 6,
                kh,
                &ch.to_string(),
                Style::Regular,
                28.0,
                INK,
            );
            zones.push(HitZone {
                x,
                y: y - gap / 2,
                w: unit,
                h: kh + gap,
                hit: Hit::Key(ch),
            });
        }
    }
    let y = y0 + 3 * (kh + gap);
    let x = 6 + 8 * unit;
    key_box(c, x + 3, y, 2 * unit - 6, kh, GRAY_FAINT);
    icons::draw(c, ts, Icon::Backspace, x + unit, y + kh / 2, 30.0, INK);
    zones.push(HitZone {
        x,
        y: y - gap / 2,
        w: 2 * unit,
        h: kh + gap,
        hit: Hit::KeyErase,
    });
    let y = y0 + 4 * (kh + gap);
    key_box(c, 9, y, 6 * unit - 6, kh, WHITE);
    key_text(
        c,
        ts,
        9,
        y,
        6 * unit - 6,
        kh,
        "espace",
        Style::Regular,
        PX_SMALL,
        GRAY_DARK,
    );
    zones.push(HitZone {
        x: 6,
        y: y - gap / 2,
        w: 6 * unit,
        h: kh + gap,
        hit: Hit::Key(' '),
    });
    let ex = 6 + 6 * unit;
    key_box(c, ex + 3, y, 4 * unit - 6, kh, INK);
    key_text(
        c,
        ts,
        ex + 3,
        y,
        4 * unit - 6,
        kh,
        enter,
        Style::Bold,
        PX_SMALL,
        WHITE,
    );
    zones.push(HitZone {
        x: ex,
        y: y - gap / 2,
        w: 4 * unit,
        h: kh + gap,
        hit: Hit::KeyEnter,
    });
    zones
}

/// Search screen: what it shows.
pub struct SearchView<'a> {
    pub query: &'a str,
    pub keyboard: bool,
    /// Results of the page: (chapter title, passage).
    pub results: &'a [(String, crate::search::Found)],
    /// Index of the page's first result in the full list.
    pub first: usize,
    /// Number of results (none: not searched yet) and whether it was capped.
    pub total: Option<(usize, bool)>,
    pub page: usize,
    pub pages: usize,
    pub busy: bool,
}

/// Search in the book: input field, then the keyboard or the results
/// (excerpts, found word in bold). Returns the zones and the field's rectangle.
pub fn search(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    v: &SearchView,
) -> (Vec<HitZone>, signet_fb::Rect) {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    status_bar(c, ts, status, true);
    let mut zones = vec![HitZone {
        x: 0,
        y: 0,
        w,
        h: STATUS_H,
        hit: Hit::Back,
    }];
    let (x, right) = (MARGIN, w - MARGIN);
    let (fy, fh) = (STATUS_H + 16, 68);
    let field = signet_fb::Rect::new(x as u32, fy as u32, (right - x) as u32, fh as u32);
    vector::fill(
        c,
        &Path::new().rounded_frame(
            x as f32,
            fy as f32,
            (right - x) as f32,
            fh as f32,
            14.0,
            2.0,
        ),
        INK,
    );
    icons::draw(c, ts, Icon::Search, x + 32, fy + fh / 2, 26.0, GRAY_DARK);
    let tx = x + 60;
    if v.query.is_empty() {
        ts.draw_text(
            c,
            "Rechercher dans le livre",
            tx,
            fy + 44,
            Style::Regular,
            PX_BODY,
            GRAY,
        );
    } else {
        let shown = ellipsize(
            ts,
            v.query,
            Style::Regular,
            PX_BODY,
            (right - tx - 30) as f32,
        );
        let qw = ts.draw_text(c, &shown, tx, fy + 44, Style::Regular, PX_BODY, INK) as i32;
        if v.keyboard {
            draw::rect(c, tx + qw + 3, fy + 16, 2, fh - 32, INK);
        }
    }
    if v.query.is_empty() && v.keyboard {
        draw::rect(c, tx - 4, fy + 16, 2, fh - 32, INK);
    }
    zones.push(HitZone {
        x,
        y: fy,
        w: right - x,
        h: fh,
        hit: Hit::SearchField,
    });
    let y0 = fy + fh + 44;
    if v.keyboard {
        ts.draw_text(
            c,
            "Un mot ou une expression : les accents et les",
            x,
            y0,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        ts.draw_text(
            c,
            "majuscules ne comptent pas.",
            x,
            y0 + 32,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        zones.extend(keyboard(c, ts, h - KEYBOARD_H, "Rechercher"));
        return (zones, field);
    }
    if v.busy {
        ts.draw_text(
            c,
            "Recherche dans tout le livre…",
            x,
            y0,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
        return (zones, field);
    }
    match v.total {
        None => {}
        Some((0, _)) => {
            ts.draw_text(
                c,
                &format!("Aucun résultat pour « {} ».", v.query),
                x,
                y0,
                Style::Italic,
                PX_BODY,
                GRAY_DARK,
            );
        }
        Some((n, capped)) => {
            let head = format!(
                "{n}{} résultat{}",
                if capped { "+" } else { "" },
                if n > 1 { "s" } else { "" }
            );
            ts.draw_text(c, &head, x, y0, Style::Bold, PX_SMALL, GRAY_DARK);
            let row_h = SEARCH_ROW_H;
            let st = ParagraphStyle {
                style: Style::Regular,
                px: PX_SMALL,
                line_height: 1.3,
                align: Align::Left,
                indent: 0.0,
            };
            let mut y = y0 + 18;
            for (k, (chapter, f)) in v.results.iter().enumerate() {
                let chapter = ellipsize(ts, chapter, Style::Bold, 20.0, (right - x) as f32);
                ts.draw_text(c, &chapter, x, y + 30, Style::Bold, 20.0, GRAY_DARK);
                let spans = [
                    (f.before.as_str(), Style::Regular),
                    (f.matched.as_str(), Style::Bold),
                    (f.after.as_str(), Style::Regular),
                ];
                let (lines, _) = ts.layout_rich(&spans, (right - x) as f32, st);
                let lines: Vec<_> = lines.into_iter().take(2).collect();
                ts.draw(c, &lines, x, y + 42, st, INK);
                zones.push(HitZone {
                    x,
                    y,
                    w: right - x,
                    h: row_h,
                    hit: Hit::SearchResult(v.first + k),
                });
                y += row_h;
                draw::hline(c, x, y - 6, right - x, GRAY_FAINT);
            }
            let label = format!("{} / {}", v.page + 1, v.pages.max(1));
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
            pager(c, ts, v.page, v.pages, &mut zones, Hit::SearchPage);
        }
    }
    (zones, field)
}

const SEARCH_ROW_H: i32 = 118;

/// Search results per screen page.
pub fn search_per_page(h: i32) -> usize {
    ((h - (STATUS_H + 16 + 68 + 44 + 18) - 64) / SEARCH_ROW_H).max(1) as usize
}

/// Note over the page: "Note 1", its text (the book's italics
/// kept), "Aller à la note". The panel takes the height of the note; a
/// note that is too long is cut off, the button leads to the full text.
pub fn note(
    c: &mut Canvas,
    ts: &mut Typesetter,
    title: &str,
    blocks: &[signet_epub::Block],
    px: f32,
    family: u8,
) -> (Vec<HitZone>, signet_fb::Rect) {
    let w = c.width as i32;
    let h = c.height as i32;
    let x = MARGIN;
    let right = w - MARGIN;
    let gap = (px * 0.5) as i32;
    // The note in the book's font; the title and the button in the interface's.
    let ui_family = ts.set_family(family);
    let laid: Vec<(ParagraphStyle, Vec<signet_text::Line>)> = blocks
        .iter()
        .map(|b| crate::reader::layout_note(b, ts, (right - x) as f32, px))
        .filter(|(_, l)| !l.is_empty())
        .collect();
    ts.set_family(ui_family);
    let text_h: i32 = laid
        .iter()
        .map(|(st, l)| (l.len() as f32 * st.px * st.line_height) as i32 + gap)
        .sum::<i32>()
        - gap;
    // Header (title, close), text, button.
    let (head, button_h, foot) = (96, 60, 28);
    let panel_h = (head + text_h.max(0) + 36 + button_h + foot).clamp(h * 30 / 100, h * 72 / 100);
    let (py, mut zones) = sheet(c, ts, title, panel_h, Hit::CloseNote);
    // Text: whole lines as long as there is room above the button.
    let by = h - foot - button_h;
    let limit = by - 30;
    let mut y = py + head;
    ts.set_family(family);
    'blocks: for (st, lines) in &laid {
        let lh = st.px * st.line_height;
        let fit = (((limit - y) as f32) / lh).floor().max(0.0) as usize;
        if fit < lines.len() {
            ts.draw(c, &lines[..fit], x, y, *st, INK);
            let ey = y + (fit as f32 * lh) as i32 + (lh * 0.6) as i32;
            ts.draw_text(
                c,
                "…",
                x,
                ey.min(limit + 10),
                Style::Regular,
                st.px,
                GRAY_DARK,
            );
            break 'blocks;
        }
        ts.draw(c, lines, x, y, *st, INK);
        y += (lines.len() as f32 * lh) as i32 + gap;
    }
    ts.set_family(ui_family);
    // "Aller à la note": bordered button, full width.
    let (fx, fy, fw) = (x as f32, by as f32, (right - x) as f32);
    vector::fill(
        c,
        &Path::new().rounded_frame(fx, fy, fw, button_h as f32, 14.0, 2.0),
        INK,
    );
    let label = "Aller à la note";
    let lw = ts.font(Style::Regular).measure(label, PX_BUTTON) as i32;
    let gx = x + ((right - x) - (lw + 10 + 24)) / 2;
    ts.draw_text(
        c,
        label,
        gx,
        by + button_h / 2 + 8,
        Style::Regular,
        PX_BUTTON,
        INK,
    );
    icons::draw(
        c,
        ts,
        Icon::ChevronRight,
        gx + lw + 10 + 12,
        by + button_h / 2,
        24.0,
        INK,
    );
    zones.push(HitZone {
        x,
        y: by - 8,
        w: right - x,
        h: button_h + 16,
        hit: Hit::GoToNote,
    });
    (zones, signet_fb::Rect::new(0, 0, w as u32, h as u32))
}

/// The text link under the finger: index into `links`. Note calls are
/// tiny glyphs ("¹"): a finger that lands next to them still reaches them
/// (24 px); other links are touched on their text. External addresses
/// lead nowhere on the reader: the page turns.
pub fn link_at(
    page: &Page,
    cfg: &PageConfig,
    links: &[signet_epub::Link],
    tx: i32,
    ty: i32,
) -> Option<usize> {
    let x0 = cfg.margin_x;
    let y0 = cfg.margin_top;
    let mut best: Option<(i32, usize)> = None;
    for item in &page.items {
        if item.kind != ItemKind::Text {
            continue;
        }
        let here: Vec<usize> = (0..links.len())
            .filter(|&i| links[i].block == item.block && !links[i].external)
            .collect();
        if here.is_empty() {
            continue;
        }
        let asc = (item.style.px * 0.8) as i32;
        let desc = (item.style.px * 0.3) as i32;
        for line in &item.lines {
            for g in &line.glyphs {
                let Some(&i) = here
                    .iter()
                    .find(|&&i| g.ch >= links[i].from && g.ch < links[i].to)
                else {
                    continue;
                };
                let reach = if links[i].note { 24 } else { 6 };
                let gx = x0 + item.x as i32 + g.x.round() as i32;
                let gw = (g.w.round() as i32).max(1);
                let by = y0 + item.y as i32 + g.y.round() as i32;
                let dx = if tx < gx {
                    gx - tx
                } else {
                    (tx - gx - gw).max(0)
                };
                let dy = if ty < by - asc {
                    by - asc - ty
                } else {
                    (ty - by - desc).max(0)
                };
                let d = dx.max(dy);
                if d <= reach && best.map(|(bd, _)| d < bd).unwrap_or(true) {
                    best = Some((d, i));
                }
            }
        }
    }
    best.map(|(_, i)| i)
}

/// Screen rectangles covered by the range [from, to) of the block (one per line).
pub fn range_rects(
    page: &Page,
    cfg: &PageConfig,
    block: usize,
    from: u32,
    to: u32,
) -> Vec<signet_fb::Rect> {
    let x0 = cfg.margin_x;
    let y0 = cfg.margin_top;
    let mut out = Vec::new();
    for item in &page.items {
        if item.kind != ItemKind::Text || item.block != block {
            continue;
        }
        let asc = (item.style.px * 0.78) as i32;
        let desc = (item.style.px * 0.26) as i32;
        for line in &item.lines {
            let mut run: Option<(i32, i32, i32)> = None;
            for g in &line.glyphs {
                let inside = g.ch >= from && g.ch < to;
                let gx = x0 + item.x as i32 + g.x.round() as i32;
                let gw = g.w.round() as i32;
                let by = y0 + item.y as i32 + g.y.round() as i32;
                match (inside, run) {
                    (true, None) => run = Some((gx, gx + gw, by)),
                    (true, Some((a, _, b))) => run = Some((a, gx + gw, b)),
                    (false, Some((a, bx, by))) => {
                        out.push(signet_fb::Rect::new(
                            a.max(0) as u32,
                            (by - asc).max(0) as u32,
                            (bx - a).max(0) as u32,
                            (asc + desc) as u32,
                        ));
                        run = None;
                    }
                    _ => {}
                }
            }
            if let Some((a, bx, by)) = run {
                out.push(signet_fb::Rect::new(
                    a.max(0) as u32,
                    (by - asc).max(0) as u32,
                    (bx - a).max(0) as u32,
                    (asc + desc) as u32,
                ));
            }
        }
    }
    out
}

/// The glyph under (tx, ty): (block, character index), or nothing.
pub fn glyph_at(page: &Page, cfg: &PageConfig, tx: i32, ty: i32) -> Option<(usize, u32)> {
    let x0 = cfg.margin_x;
    let y0 = cfg.margin_top;
    for item in &page.items {
        if item.kind != ItemKind::Text {
            continue;
        }
        // Generous line box: the whole line spacing is tappable.
        let asc = (item.style.px * 0.9) as i32;
        let desc = (item.style.px * (item.style.line_height - 0.9)).max(item.style.px * 0.3) as i32;
        for line in &item.lines {
            let Some(first) = line.glyphs.first() else {
                continue;
            };
            let by = y0 + item.y as i32 + first.y.round() as i32;
            if ty < by - asc || ty > by + desc {
                continue;
            }
            for g in &line.glyphs {
                let gx = x0 + item.x as i32 + g.x.round() as i32;
                if tx >= gx - 2 && tx < gx + g.w.round() as i32 + 2 {
                    return Some((item.block, g.ch));
                }
            }
        }
    }
    None
}

/// Annotations screen: the book's highlights and bookmarks, in text
/// order, paginated. `items`: (chapter title, excerpt, bookmark).
pub fn annotations(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    items: &[(String, String, bool)],
    page: usize,
) -> Vec<HitZone> {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    status_bar(c, ts, status, true);
    let mut zones = vec![HitZone {
        x: 0,
        y: 0,
        w,
        h: STATUS_H,
        hit: Hit::Back,
    }];
    ts.draw_text(
        c,
        "Annotations",
        MARGIN,
        STATUS_H + 50,
        Style::Bold,
        PX_SECTION,
        INK,
    );
    let marks = items.iter().filter(|i| i.2).count();
    let highlights = items.len() - marks;
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n > 1 { many } else { one });
    let count = match (highlights, marks) {
        (h, 0) => plural(h, "surlignage", "surlignages"),
        (0, m) => plural(m, "marque-page", "marque-pages"),
        (h, m) => format!(
            "{} · {}",
            plural(h, "surlignage", "surlignages"),
            plural(m, "marque-page", "marque-pages")
        ),
    };
    let cw = ts.font(Style::Regular).measure(&count, PX_SMALL) as i32;
    ts.draw_text(
        c,
        &count,
        w - MARGIN - cw,
        STATUS_H + 48,
        Style::Regular,
        PX_SMALL,
        GRAY_DARK,
    );
    let row_h = 112;
    let top = STATUS_H + 84;
    let per_page = ((h - top - 80) / row_h).max(1) as usize;
    let pages = (items.len() + per_page - 1) / per_page.max(1);
    let start = page * per_page;
    let mut y = top;
    let st = ParagraphStyle {
        style: Style::Regular,
        px: PX_SMALL,
        line_height: 1.3,
        align: Align::Left,
        indent: 0.0,
    };
    for (i, (chapter, text, bookmark)) in items.iter().enumerate().skip(start).take(per_page) {
        let title_w = (w - 2 * MARGIN - 24) as f32;
        if *bookmark {
            // Bookmark: the ribbon, and the first words of the marked page.
            ribbon(c, MARGIN as f32 - 2.0, (y + 12) as f32, 14.0, 24.0, INK);
            let title = ellipsize(
                ts,
                &format!("Marque-page · {chapter}"),
                Style::Bold,
                20.0,
                title_w,
            );
            ts.draw_text(c, &title, MARGIN + 24, y + 30, Style::Bold, 20.0, GRAY_DARK);
            let (lines, _) = ts.layout(text, title_w, st);
            let lines: Vec<_> = lines.into_iter().take(2).collect();
            ts.draw(c, &lines, MARGIN + 24, y + 42, st, GRAY_DARK);
        } else {
            draw::rect(c, MARGIN, y + 10, 6, row_h - 28, GRAY_LIGHT);
            let title = ellipsize(ts, chapter, Style::Bold, 20.0, title_w);
            ts.draw_text(c, &title, MARGIN + 24, y + 30, Style::Bold, 20.0, GRAY_DARK);
            let (lines, _) = ts.layout(&format!("« {text} »"), title_w, st);
            let lines: Vec<_> = lines.into_iter().take(2).collect();
            ts.draw(c, &lines, MARGIN + 24, y + 42, st, INK);
        }
        zones.push(HitZone {
            x: MARGIN,
            y,
            w: w - 2 * MARGIN,
            h: row_h,
            hit: Hit::GoToAnnotation(i),
        });
        y += row_h;
        draw::hline(c, MARGIN + 24, y - 6, w - 2 * MARGIN - 24, GRAY_FAINT);
    }
    if items.is_empty() {
        icons::draw(c, ts, Icon::Highlighter, MARGIN + 20, top + 40, 40.0, GRAY);
        ts.draw_text(
            c,
            "Aucune annotation.",
            MARGIN + 56,
            top + 50,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
        ts.draw_text(
            c,
            "Dans une page : appui long sur un mot, puis appuyez sur",
            MARGIN,
            top + 100,
            Style::Regular,
            PX_SMALL,
            GRAY,
        );
        ts.draw_text(
            c,
            "le dernier mot du passage, et « Surligner ».",
            MARGIN,
            top + 132,
            Style::Regular,
            PX_SMALL,
            GRAY,
        );
        ts.draw_text(
            c,
            "Un appui dans le coin haut-droit de la page la marque.",
            MARGIN,
            top + 180,
            Style::Regular,
            PX_SMALL,
            GRAY,
        );
    }
    let label = format!("{} / {}", page + 1, pages.max(1));
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
    pager(c, ts, page, pages, &mut zones, Hit::AnnotationsPage);
    zones
}

/// Pagination at the bottom of a list: "Précédent" and "Suivant", with their chevrons.
pub(crate) fn pager(
    c: &mut Canvas,
    ts: &mut Typesetter,
    page: usize,
    pages: usize,
    zones: &mut Vec<HitZone>,
    to: impl Fn(usize) -> Hit,
) {
    let (w, h) = (c.width as i32, c.height as i32);
    if page > 0 {
        icons::draw(c, ts, Icon::ChevronLeft, MARGIN + 8, h - 35, 22.0, INK);
        ts.draw_text(
            c,
            "Précédent",
            MARGIN + 24,
            h - 28,
            Style::Regular,
            PX_SMALL,
            INK,
        );
        zones.push(HitZone {
            x: 0,
            y: h - 70,
            w: w / 3,
            h: 70,
            hit: to(page - 1),
        });
    }
    if page + 1 < pages {
        let t = "Suivant";
        let tw = ts.font(Style::Regular).measure(t, PX_SMALL) as i32;
        ts.draw_text(
            c,
            t,
            w - MARGIN - 24 - tw,
            h - 28,
            Style::Regular,
            PX_SMALL,
            INK,
        );
        icons::draw(c, ts, Icon::ChevronRight, w - MARGIN - 8, h - 35, 22.0, INK);
        zones.push(HitZone {
            x: 2 * w / 3,
            y: h - 70,
            w: w / 3,
            h: 70,
            hit: to(page + 1),
        });
    }
}

pub struct SettingsView {
    pub size: usize,
    pub margins: usize,
    pub leading: usize,
    pub light: u8,
    pub dev_mode: bool,
    pub auto_light: bool,
    pub fast_pages: bool,
    pub ink: usize,
    /// Reader's address on the network (dev mode), for SSH.
    pub dev_ip: Option<String>,
    /// The displayed page carries a bookmark.
    pub bookmarked: bool,
    /// "Passer en lecture" touched once: the button asks for confirmation.
    pub dev_confirm: bool,
    pub night: bool,
    /// Reading font (index into `theme::FAMILIES`).
    pub font: usize,
}

/// Screen 03 — Quick Settings: panel at the bottom, page visible behind.
pub fn settings_overlay(
    c: &mut Canvas,
    ts: &mut Typesetter,
    s: &SettingsView,
) -> (Vec<HitZone>, signet_fb::Rect) {
    let w = c.width as i32;
    let h = c.height as i32;
    // In reading mode, no technical line at the bottom: a shorter panel.
    let panel_h = if s.dev_mode { 764 } else { 692 };
    let py = h - panel_h;
    draw::dim(c, 0, 0, w, py, 90);
    draw::rounded(c, 0, py, w, panel_h + 24, 24, WHITE);
    draw::hline(c, 0, py, w, GRAY_LIGHT);
    // handle
    draw::rounded(c, w / 2 - 28, py + 12, 56, 6, 3, GRAY_LIGHT);
    let mut zones = vec![HitZone {
        x: 0,
        y: 0,
        w,
        h: py,
        hit: Hit::CloseSettings,
    }];

    let x = MARGIN;
    let right = w - MARGIN;
    ts.draw_text(c, "Lecture", x, py + 56, Style::Bold, PX_BODY, INK);
    zones.push(HitZone {
        x,
        y: py + 16,
        w: 220,
        h: 64,
        hit: Hit::DevSecret,
    });
    icons::draw(c, ts, Icon::Close, right - 14, py + 46, 30.0, INK);
    zones.push(HitZone {
        x: right - 60,
        y: py + 10,
        w: 70,
        h: 70,
        hit: Hit::CloseSettings,
    });

    let row = |i: i32| py + 110 + i * 78;
    let label_x = x + 44;
    let ctrl_x = right - 3 * 64;

    // Text size: A A A
    icons::draw(c, ts, Icon::TextSize, x + 14, row(0) + 2, 28.0, INK);
    ts.draw_text(
        c,
        "Taille du texte",
        label_x,
        row(0) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    for (i, px) in [20.0f32, 26.0, 32.0].iter().enumerate() {
        let bx = ctrl_x + i as i32 * 64;
        toggle_box(c, bx, row(0) - 26, 54, 54, s.size == i);
        let aw = ts.font(Style::Regular).measure("A", *px) as i32;
        ts.draw_text(
            c,
            "A",
            bx + (54 - aw) / 2,
            row(0) + 12,
            Style::Regular,
            *px,
            INK,
        );
        zones.push(HitZone {
            x: bx,
            y: row(0) - 26,
            w: 54,
            h: 54,
            hit: Hit::SetSize(i),
        });
    }

    // Font: each name drawn in its own font.
    icons::draw(c, ts, Icon::Type, x + 14, row(1) + 2, 26.0, INK);
    ts.draw_text(
        c,
        "Police",
        label_x,
        row(1) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    let fw = 128;
    for (i, name) in crate::theme::FAMILIES.iter().enumerate() {
        let bx = right - (crate::theme::FAMILIES.len() - i) as i32 * (fw + 10) + 10;
        toggle_box(c, bx, row(1) - 26, fw, 54, s.font == i);
        let ui_family = ts.set_family(i as u8);
        let nw = ts.font(Style::Regular).measure(name, 21.0) as i32;
        ts.draw_text(
            c,
            name,
            bx + (fw - nw) / 2,
            row(1) + 8,
            Style::Regular,
            21.0,
            INK,
        );
        ts.set_family(ui_family);
        zones.push(HitZone {
            x: bx,
            y: row(1) - 26,
            w: fw,
            h: 54,
            hit: Hit::SetFont(i),
        });
    }

    // Brightness: slider, "Auto" box (time-of-day adaptation), moon (night mode)
    icons::draw(c, ts, Icon::Sun, x + 14, row(2) + 2, 28.0, INK);
    ts.draw_text(
        c,
        "Luminosité",
        label_x,
        row(2) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    let moon_w = 54;
    toggle_box(c, right - moon_w, row(2) - 22, moon_w, 44, s.night);
    icons::draw(c, ts, Icon::Moon, right - moon_w / 2, row(2), 24.0, INK);
    zones.push(HitZone {
        x: right - moon_w - 4,
        y: row(2) - 28,
        w: moon_w + 14,
        h: 56,
        hit: Hit::ToggleNight,
    });
    let auto_w = 70;
    let ax = right - moon_w - 10 - auto_w;
    toggle_box(c, ax, row(2) - 22, auto_w, 44, s.auto_light);
    let aw = ts.font(Style::Regular).measure("Auto", 20.0) as i32;
    ts.draw_text(
        c,
        "Auto",
        ax + (auto_w - aw) / 2,
        row(2) + 7,
        Style::Regular,
        20.0,
        INK,
    );
    zones.push(HitZone {
        x: ax - 6,
        y: row(2) - 28,
        w: auto_w + 10,
        h: 56,
        hit: Hit::ToggleAutoLight,
    });
    let sx = label_x + ts.font(Style::Regular).measure("Luminosité", PX_SMALL) as i32 + 36;
    let sw = ax - 22 - sx;
    // Smoothed slider: track, set portion, round knob.
    let (tx, ty) = (sx as f32, row(2) as f32 - 2.5);
    vector::fill(
        c,
        &Path::new().rounded_rect(tx, ty, sw as f32, 5.0, 2.5, false),
        GRAY_LIGHT,
    );
    let knob = sx as f32 + sw as f32 * s.light as f32 / 100.0;
    vector::fill(
        c,
        &Path::new().rounded_rect(tx, ty, (knob - tx).max(5.0), 5.0, 2.5, false),
        INK,
    );
    vector::fill(
        c,
        &Path::new().circle(knob, row(2) as f32, 12.0, false),
        INK,
    );
    vector::fill(
        c,
        &Path::new().circle(knob, row(2) as f32, 4.5, false),
        WHITE,
    );
    for step in 0..10u8 {
        let zx = sx + sw * step as i32 / 10;
        zones.push(HitZone {
            x: zx,
            y: row(2) - 30,
            w: sw / 10 + 1,
            h: 60,
            hit: Hit::SetLight(step * 10 + 10),
        });
    }

    // Margins: 3 pictograms
    icons::margins(c, x + 14, row(3) + 1, 1, INK);
    ts.draw_text(
        c,
        "Marges",
        label_x,
        row(3) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    for i in 0..3 {
        let bx = ctrl_x + i as i32 * 64;
        toggle_box(c, bx, row(3) - 26, 54, 54, s.margins == i);
        icons::margins(c, bx + 27, row(3) + 1, i, INK);
        zones.push(HitZone {
            x: bx,
            y: row(3) - 26,
            w: 54,
            h: 54,
            hit: Hit::SetMargins(i),
        });
    }

    // Line spacing: 3 pictograms
    icons::leading(c, x + 14, row(4) + 1, 1, INK);
    ts.draw_text(
        c,
        "Interligne",
        label_x,
        row(4) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    for i in 0..3 {
        let bx = ctrl_x + i as i32 * 64;
        toggle_box(c, bx, row(4) - 26, 54, 54, s.leading == i);
        icons::leading(c, bx + 27, row(4) + 1, i, INK);
        zones.push(HitZone {
            x: bx,
            y: row(4) - 26,
            w: 54,
            h: 54,
            hit: Hit::SetLeading(i),
        });
    }

    // Ink: anti-aliasing contrast and weight (thin / sharp / bold)
    icons::draw(c, ts, Icon::Droplet, x + 14, row(5) + 2, 28.0, INK);
    ts.draw_text(
        c,
        "Encre",
        label_x,
        row(5) + 10,
        Style::Regular,
        PX_SMALL,
        INK,
    );
    for (i, name) in ["Fin", "Net", "Gras"].iter().enumerate() {
        let bx = ctrl_x + i as i32 * 64;
        toggle_box(c, bx, row(5) - 26, 54, 54, s.ink == i);
        let (gamma, emb) = crate::theme::INKS[i];
        ts.set_ink(gamma, emb);
        let nw = ts.font(Style::Regular).measure(name, 17.0) as i32;
        ts.draw_text(
            c,
            name,
            bx + (54 - nw) / 2,
            row(5) + 7,
            Style::Regular,
            17.0,
            INK,
        );
        zones.push(HitZone {
            x: bx,
            y: row(5) - 26,
            w: 54,
            h: 54,
            hit: Hit::SetInk(i),
        });
    }
    let (gamma, emb) = crate::theme::INKS[s.ink.min(2)];
    ts.set_ink(gamma, emb);

    // Fast page turning (black and white text, DU refresh)
    let fy = row(5) + 54;
    // The technical detail only in dev mode.
    let fl = if s.dev_mode {
        "Pages rapides (noir et blanc, 375 ms au lieu de 620)"
    } else {
        "Tourner les pages plus vite"
    };
    ts.draw_text(c, fl, label_x, fy + 6, Style::Regular, 20.0, GRAY_DARK);
    toggle_box(c, right - 78, fy - 18, 78, 40, s.fast_pages);
    let onoff = if s.fast_pages { "Oui" } else { "Non" };
    let ow = ts.font(Style::Regular).measure(onoff, 20.0) as i32;
    ts.draw_text(
        c,
        onoff,
        right - 78 + (78 - ow) / 2,
        fy + 7,
        Style::Regular,
        20.0,
        INK,
    );
    zones.push(HitZone {
        x: right - 90,
        y: fy - 26,
        w: 100,
        h: 56,
        hit: Hit::ToggleFastPages,
    });

    // Table of contents, annotations, search, go to a page, bookmark: a
    // row of buttons, the icon above the label.
    let ly = py + 592;
    draw::hline(c, x, ly - 4, right - x, GRAY_LIGHT);
    let items: [(Option<Icon>, &str, Hit); 5] = [
        (Some(Icon::Contents), "Sommaire", Hit::Toc),
        (Some(Icon::Highlighter), "Annotations", Hit::Annotations),
        (Some(Icon::Search), "Rechercher", Hit::Search),
        (Some(Icon::Hash), "Aller à…", Hit::GoToPage),
        (None, "Marque-page", Hit::ToggleBookmark),
    ];
    let cell = (right - x) / items.len() as i32;
    for (k, (icon, label, hit)) in items.iter().enumerate() {
        let cx = x + k as i32 * cell + cell / 2;
        match icon {
            Some(i) => icons::draw(c, ts, *i, cx, ly + 32, 28.0, INK),
            // The bookmark: the solid ribbon when the page is marked.
            None if s.bookmarked => ribbon(c, cx as f32 - 9.0, (ly + 17) as f32, 18.0, 30.0, INK),
            None => icons::draw(c, ts, Icon::Bookmark, cx, ly + 32, 28.0, INK),
        }
        let lw = ts.font(Style::Regular).measure(label, 18.0) as i32;
        ts.draw_text(c, label, cx - lw / 2, ly + 74, Style::Regular, 18.0, INK);
        zones.push(HitZone {
            x: x + k as i32 * cell,
            y: ly,
            w: cell,
            h: 92,
            hit: *hit,
        });
    }

    // Dev mode only: the SSH address and the return to reading mode. In reading
    // mode, nothing technical (a long press on "Lecture" reopens dev mode).
    if s.dev_mode {
        // Mode: reading (no Wi-Fi, sleep) or development (Wi-Fi + SSH, no sleep).
        let my = py + 722;
        draw::hline(c, x, my - 36, right - x, GRAY_LIGHT);
        let btn = if s.dev_confirm {
            "Confirmer : sans Wi-Fi"
        } else if s.dev_mode {
            "Passer en lecture"
        } else {
            "Passer en dev"
        };
        let bw = ts.font(Style::Regular).measure(btn, 20.0) as i32 + 28;
        let label = match (s.dev_mode, &s.dev_ip) {
            (true, Some(ip)) => format!("Mode dev : SSH sur {ip}, pas de veille"),
            (true, None) => "Mode dev : Wi-Fi en connexion, pas de veille".to_string(),
            (false, _) => "Mode lecture : veille, Wi-Fi coupé".to_string(),
        };
        icons::draw(
            c,
            ts,
            if s.dev_mode {
                Icon::Terminal
            } else {
                Icon::BookOpen
            },
            x + 11,
            my - 1,
            22.0,
            GRAY_DARK,
        );
        let label = ellipsize(
            ts,
            &label,
            Style::Regular,
            20.0,
            (right - bw - 16 - x - 32) as f32,
        );
        ts.draw_text(c, &label, x + 32, my + 6, Style::Regular, 20.0, GRAY_DARK);
        // Second tap expected: the button turns black.
        let (fill, frame, ink) = if s.dev_confirm {
            (INK, INK, WHITE)
        } else {
            (GRAY_FAINT, GRAY_LIGHT, INK)
        };
        vector::fill(
            c,
            &Path::new().rounded_rect(
                (right - bw) as f32,
                (my - 20) as f32,
                bw as f32,
                40.0,
                9.0,
                false,
            ),
            fill,
        );
        vector::fill(
            c,
            &Path::new().rounded_frame(
                (right - bw) as f32,
                (my - 20) as f32,
                bw as f32,
                40.0,
                9.0,
                1.2,
            ),
            frame,
        );
        ts.draw_text(c, btn, right - bw + 14, my + 6, Style::Regular, 20.0, ink);
        zones.push(HitZone {
            x: right - bw - 10,
            y: my - 30,
            w: bw + 20,
            h: 60,
            hit: Hit::ToggleDevMode,
        });
    }
    (zones, signet_fb::Rect::new(0, 0, w as u32, h as u32))
}

/// Option box, rounded corners and smoothed edges: chosen, light gray background and
/// black border; otherwise, a thin gray border.
fn toggle_box(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, on: bool) {
    let (x, y, w, h) = (x as f32, y as f32, w as f32, h as f32);
    if on {
        vector::fill(
            c,
            &Path::new().rounded_rect(x, y, w, h, 10.0, false),
            GRAY_FAINT,
        );
        vector::fill(c, &Path::new().rounded_frame(x, y, w, h, 10.0, 2.0), INK);
    } else {
        vector::fill(
            c,
            &Path::new().rounded_frame(x, y, w, h, 10.0, 1.2),
            GRAY_LIGHT,
        );
    }
}

/// "Tout voir" screen: the whole library, paginated.
/// Orders offered in the library.
pub const LIBRARY_SORTS: [&str; 3] = ["Récents", "Titre", "Auteur"];

/// Full library, paginated, in the chosen order. `books`: (index into
/// the library, book). Each row has its menu (⋮): finished, delete.
pub fn library(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    books: &[(usize, &BookEntry)],
    page: usize,
    sort: usize,
) -> Vec<HitZone> {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    status_bar(c, ts, status, false);
    let mut zones = vec![HitZone {
        x: 0,
        y: 0,
        w: w / 2,
        h: STATUS_H,
        hit: Hit::Back,
    }];
    ts.draw_text(
        c,
        "Bibliothèque",
        MARGIN,
        STATUS_H + 50,
        Style::Bold,
        PX_SECTION,
        INK,
    );
    // Sort: three boxes to the right of the title.
    let (sw, gap) = (104, 8);
    let sx0 = w - MARGIN - 3 * sw - 2 * gap;
    for (k, name) in LIBRARY_SORTS.iter().enumerate() {
        let bx = sx0 + k as i32 * (sw + gap);
        toggle_box(c, bx, STATUS_H + 18, sw, 46, sort == k);
        let nw = ts.font(Style::Regular).measure(name, 20.0) as i32;
        ts.draw_text(
            c,
            name,
            bx + (sw - nw) / 2,
            STATUS_H + 48,
            Style::Regular,
            20.0,
            INK,
        );
        zones.push(HitZone {
            x: bx,
            y: STATUS_H + 10,
            w: sw,
            h: 62,
            hit: Hit::LibrarySort(k),
        });
    }
    let row_h = 120;
    let top = STATUS_H + 84;
    let per_page = ((h - top - 80) / row_h).max(1) as usize;
    let pages = (books.len() + per_page - 1) / per_page.max(1);
    let start = page * per_page;
    let thumb_w = 68;
    let thumb_h = 98;
    let more_w = 44;
    let mut y = top;
    for (i, book) in books.iter().skip(start).take(per_page) {
        cover_box(c, book.cover.as_ref(), MARGIN, y + 11, thumb_w, thumb_h);
        let tx = MARGIN + thumb_w + 24;
        let right_label = if book.finished {
            "Terminé".to_string()
        } else if !book.started {
            "Non commencé".to_string()
        } else {
            format!("{} %", book.percent)
        };
        let rw = ts.font(Style::Regular).measure(&right_label, PX_SMALL) as i32;
        let lx = w - MARGIN - more_w - rw;
        let title_w = lx - tx - 20;
        let title = ellipsize(ts, &book.title, Style::Regular, PX_BODY, title_w as f32);
        ts.draw_text(c, &title, tx, y + 50, Style::Regular, PX_BODY, INK);
        let author = ellipsize(ts, &book.author, Style::Regular, PX_SMALL, title_w as f32);
        ts.draw_text(c, &author, tx, y + 82, Style::Regular, PX_SMALL, GRAY_DARK);
        ts.draw_text(
            c,
            &right_label,
            lx,
            y + 64,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        icons::draw(
            c,
            ts,
            Icon::More,
            w - MARGIN - more_w / 2 + 6,
            y + row_h / 2,
            26.0,
            INK,
        );
        // The menu first: its zone takes precedence over the book's.
        zones.push(HitZone {
            x: w - MARGIN - more_w - 8,
            y,
            w: more_w + 8 + MARGIN,
            h: row_h,
            hit: Hit::BookMenu(*i),
        });
        zones.push(HitZone {
            x: MARGIN,
            y,
            w: w - 2 * MARGIN - more_w,
            h: row_h,
            hit: Hit::OpenBook(*i),
        });
        y += row_h;
        draw::hline(c, tx, y, w - MARGIN - tx, GRAY_FAINT);
    }
    if books.is_empty() {
        icons::draw(c, ts, Icon::Library, MARGIN + 20, top + 50, 40.0, GRAY);
        ts.draw_text(
            c,
            "Aucun livre : copiez des EPUB par USB.",
            MARGIN + 56,
            top + 60,
            Style::Italic,
            PX_BODY,
            GRAY_DARK,
        );
    }
    let label = format!(
        "{} livre{} · {} / {}",
        books.len(),
        if books.len() > 1 { "s" } else { "" },
        page + 1,
        pages.max(1)
    );
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
    pager(c, ts, page, pages, &mut zones, Hit::LibraryPage);
    zones
}

/// Menu of a library book: finished or not, delete. `confirm`:
/// deletion requested, to be confirmed (the file leaves the reader).
pub fn book_menu(
    c: &mut Canvas,
    ts: &mut Typesetter,
    book: &BookEntry,
    index: usize,
    confirm: bool,
) -> (Vec<HitZone>, signet_fb::Rect) {
    let w = c.width as i32;
    let h = c.height as i32;
    let (x, right) = (MARGIN, w - MARGIN);
    let title = ellipsize(
        ts,
        &book.title,
        Style::Bold,
        PX_SECTION,
        (right - x - 60) as f32,
    );
    let (py, mut zones) = sheet(
        c,
        ts,
        &title,
        if confirm { 380 } else { 330 },
        Hit::CloseBookMenu,
    );
    let author = ellipsize(
        ts,
        &book.author,
        Style::Italic,
        PX_SMALL,
        (right - x) as f32,
    );
    ts.draw_text(c, &author, x, py + 98, Style::Italic, PX_SMALL, GRAY_DARK);
    if !confirm {
        let rows: [(Icon, &str, Hit); 2] = [
            (
                Icon::BookCheck,
                if book.finished {
                    "Marquer comme non terminé"
                } else {
                    "Marquer comme terminé"
                },
                Hit::ToggleFinished(index),
            ),
            (
                Icon::Trash,
                "Supprimer de la liseuse",
                Hit::AskDelete(index),
            ),
        ];
        let mut y = py + 128;
        for (icon, label, hit) in rows {
            draw::hline(c, x, y, right - x, GRAY_FAINT);
            icons::draw(c, ts, icon, x + 16, y + 40, 28.0, INK);
            ts.draw_text(c, label, x + 52, y + 49, Style::Regular, PX_BODY, INK);
            zones.push(HitZone {
                x,
                y,
                w: right - x,
                h: 80,
                hit,
            });
            y += 80;
        }
    } else {
        ts.draw_text(
            c,
            "Supprimer ce livre de la liseuse ?",
            x,
            py + 150,
            Style::Bold,
            PX_BODY,
            INK,
        );
        ts.draw_text(
            c,
            "Le fichier EPUB, la position de lecture et les",
            x,
            py + 190,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        ts.draw_text(
            c,
            "annotations seront effacés.",
            x,
            py + 220,
            Style::Regular,
            PX_SMALL,
            GRAY_DARK,
        );
        let (by, bh, gap) = (py + 262, 64, 16);
        let bw = (right - x - gap) / 2;
        key_box(c, x, by, bw, bh, WHITE);
        key_text(
            c,
            ts,
            x,
            by,
            bw,
            bh,
            "Annuler",
            Style::Regular,
            PX_BODY,
            INK,
        );
        zones.push(HitZone {
            x,
            y: by - 8,
            w: bw,
            h: bh + 16,
            hit: Hit::CloseBookMenu,
        });
        key_box(c, x + bw + gap, by, bw, bh, INK);
        key_text(
            c,
            ts,
            x + bw + gap,
            by,
            bw,
            bh,
            "Supprimer",
            Style::Bold,
            PX_BODY,
            WHITE,
        );
        zones.push(HitZone {
            x: x + bw + gap,
            y: by - 8,
            w: bw,
            h: bh + 16,
            hit: Hit::ConfirmDelete(index),
        });
    }
    (zones, signet_fb::Rect::new(0, 0, w as u32, h as u32))
}

/// Table-of-contents screen: the table of contents, paginated.
const TOC_ROW_H: i32 = 72;
const TOC_TOP: i32 = STATUS_H + 90;

/// Table-of-contents entries per page.
pub fn toc_per_page(h: i32) -> usize {
    ((h - TOC_TOP - 80) / TOC_ROW_H).max(1) as usize
}

/// Table of contents: (label, depth, action); `current`: the entry of the passage read.
pub fn toc(
    c: &mut Canvas,
    ts: &mut Typesetter,
    status: &Status,
    entries: &[(String, u8, Hit)],
    page: usize,
    current: Option<usize>,
) -> Vec<HitZone> {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    status_bar(c, ts, status, true);
    let mut zones = vec![HitZone {
        x: 0,
        y: 0,
        w,
        h: STATUS_H,
        hit: Hit::Back,
    }];
    ts.draw_text(
        c,
        "Sommaire",
        MARGIN,
        STATUS_H + 50,
        Style::Bold,
        PX_SECTION,
        INK,
    );
    let row_h = TOC_ROW_H;
    let per_page = toc_per_page(h);
    let pages = (entries.len() + per_page - 1) / per_page.max(1);
    let start = page * per_page;
    let mut y = TOC_TOP;
    for (k, (label, depth, hit)) in entries.iter().enumerate().skip(start).take(per_page) {
        let x = MARGIN + *depth as i32 * 28;
        let is_current = Some(k) == current;
        if is_current {
            draw::rounded(
                c,
                MARGIN - 12,
                y,
                w - 2 * MARGIN + 24,
                row_h - 8,
                8,
                GRAY_FAINT,
            );
        }
        let maxw = (w - MARGIN - x) as f32;
        let text = ellipsize(
            ts,
            label,
            Style::Regular,
            if *depth == 0 { PX_BODY } else { PX_SMALL },
            maxw,
        );
        ts.draw_text(
            c,
            &text,
            x,
            y + 44,
            Style::Regular,
            if *depth == 0 { PX_BODY } else { PX_SMALL },
            if *depth == 0 { INK } else { GRAY_DARK },
        );
        zones.push(HitZone {
            x: MARGIN,
            y,
            w: w - 2 * MARGIN,
            h: row_h,
            hit: *hit,
        });
        y += row_h;
        draw::hline(c, x, y - 4, w - MARGIN - x, GRAY_FAINT);
    }
    // bottom-of-page navigation
    let label = format!("{} / {}", page + 1, pages.max(1));
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
    pager(c, ts, page, pages, &mut zones, Hit::TocPage);
    zones
}

/// Sleep screen: cover of the current book, otherwise a word.
pub fn sleep_screen(
    c: &mut Canvas,
    ts: &mut Typesetter,
    book: Option<&BookEntry>,
    caption: &str,
    icon: Icon,
) {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    match book.and_then(|b| b.cover.as_ref()) {
        Some(g) => {
            let g = g.fit((w - 2 * MARGIN) as u32, (h - 260) as u32);
            let x = (w - g.width as i32) / 2;
            let y = 90;
            g.blit(c, x, y);
            draw::frame(c, x, y, g.width as i32, g.height as i32, 1, GRAY_LIGHT);
            if let Some(b) = book {
                let t = ellipsize(
                    ts,
                    &b.title,
                    Style::Regular,
                    PX_BODY,
                    (w - 2 * MARGIN) as f32,
                );
                let tw = ts.font(Style::Regular).measure(&t, PX_BODY) as i32;
                ts.draw_text(
                    c,
                    &t,
                    (w - tw) / 2,
                    y + g.height as i32 + 50,
                    Style::Regular,
                    PX_BODY,
                    INK,
                );
                let a = ellipsize(
                    ts,
                    &b.author,
                    Style::Italic,
                    PX_SMALL,
                    (w - 2 * MARGIN) as f32,
                );
                let aw = ts.font(Style::Italic).measure(&a, PX_SMALL) as i32;
                ts.draw_text(
                    c,
                    &a,
                    (w - aw) / 2,
                    y + g.height as i32 + 86,
                    Style::Italic,
                    PX_SMALL,
                    GRAY_DARK,
                );
            }
        }
        None => {
            let t = "Signet";
            let tw = ts.font(Style::Bold).measure(t, 48.0) as i32;
            ts.draw_text(c, t, (w - tw) / 2, h / 2, Style::Bold, 48.0, INK);
        }
    }
    let zw = ts.font(Style::Regular).measure(caption, PX_SMALL) as i32;
    let gx = (w - zw - 30) / 2;
    icons::draw(c, ts, icon, gx + 11, h - 67, 22.0, GRAY);
    ts.draw_text(c, caption, gx + 30, h - 60, Style::Regular, PX_SMALL, GRAY);
}

/// "Connected over USB" screen.
pub fn usb_screen(c: &mut Canvas, ts: &mut Typesetter) {
    let w = c.width as i32;
    let h = c.height as i32;
    c.clear(WHITE);
    let cx = w / 2;
    let cy = h / 2 - 120;
    icons::draw(c, ts, Icon::Usb, cx, cy + 40, 120.0, INK);
    let t = "Connectée à l'ordinateur";
    let tw = ts.font(Style::Bold).measure(t, PX_SECTION) as i32;
    ts.draw_text(c, t, (w - tw) / 2, cy + 280, Style::Bold, PX_SECTION, INK);
    let m = "La bibliothèque est accessible en USB.";
    let mw = ts.font(Style::Regular).measure(m, PX_BODY) as i32;
    ts.draw_text(
        c,
        m,
        (w - mw) / 2,
        cy + 330,
        Style::Regular,
        PX_BODY,
        GRAY_DARK,
    );
    let m2 = "Éjectez le disque pour reprendre la lecture.";
    let m2w = ts.font(Style::Italic).measure(m2, PX_SMALL) as i32;
    ts.draw_text(
        c,
        m2,
        (w - m2w) / 2,
        cy + 370,
        Style::Italic,
        PX_SMALL,
        GRAY,
    );
}

/// Touch calibration screen: a target, a message.
pub fn calibration(c: &mut Canvas, ts: &mut Typesetter, target: (i32, i32), step: u8) {
    let w = c.width as i32;
    c.clear(WHITE);
    let (tx, ty) = target;
    draw::rounded(c, tx - 26, ty - 26, 52, 52, 26, INK);
    draw::rounded(c, tx - 18, ty - 18, 36, 36, 18, WHITE);
    draw::rounded(c, tx - 6, ty - 6, 12, 12, 6, INK);
    let title = "Calibration de l'écran tactile";
    let tw = ts.font(Style::Bold).measure(title, PX_SECTION) as i32;
    ts.draw_text(c, title, (w - tw) / 2, 430, Style::Bold, PX_SECTION, INK);
    let msg = if step == 0 {
        "Touchez la cible en haut à gauche."
    } else {
        "Touchez la cible en bas à droite."
    };
    let mw = ts.font(Style::Regular).measure(msg, PX_BODY) as i32;
    ts.draw_text(
        c,
        msg,
        (w - mw) / 2,
        490,
        Style::Regular,
        PX_BODY,
        GRAY_DARK,
    );
    let hint = "Une seule fois : Signet mémorise le résultat.";
    let hw = ts.font(Style::Italic).measure(hint, PX_SMALL) as i32;
    ts.draw_text(c, hint, (w - hw) / 2, 540, Style::Italic, PX_SMALL, GRAY);
}

#[cfg(test)]
mod tests_barre {
    use super::*;

    fn bar(
        ts: &mut Typesetter,
        clock: &str,
        battery: Option<u8>,
        charging: bool,
        small: bool,
    ) -> Canvas {
        let mut c = Canvas::new(758, 80);
        c.clear(WHITE);
        status_bar(
            &mut c,
            ts,
            &Status {
                title: "Jeudi 22 octobre",
                clock,
                battery,
                charging,
            },
            small,
        );
        c
    }

    /// The corner of the bar redrawn alone (the time passes, the cable is plugged or
    /// unplugged) gives exactly the bar drawn all at once; the bolt is only there
    /// while charging.
    #[test]
    fn status_corner_refresh_matches_full_draw() {
        let mut ts = crate::typesetter();
        for small in [true, false] {
            for (old, new) in [
                (("12:22", false), ("12:23", true)),
                (("12:23", true), ("12:24", false)),
                (("9:59", false), ("10:00", false)),
            ] {
                let mut c = bar(&mut ts, old.0, Some(80), old.1, small);
                let r = refresh_status_right(
                    &mut c,
                    &mut ts,
                    &Status {
                        title: "",
                        clock: new.0,
                        battery: Some(80),
                        charging: new.1,
                    },
                    &Status {
                        title: "",
                        clock: old.0,
                        battery: Some(80),
                        charging: old.1,
                    },
                    small,
                );
                let full = bar(&mut ts, new.0, Some(80), new.1, small);
                assert_eq!(
                    c.data, full.data,
                    "{old:?} → {new:?} (petite : {small}), zone {r:?}"
                );
            }
            // The bolt is inside the battery: the battery changes while charging, and nothing
            // is drawn between the time and the battery.
            let bx = 758 - MARGIN - icons::BATTERY_W;
            let region = |c: &Canvas, x0: i32, x1: i32| {
                (x0..x1)
                    .flat_map(|x| (8..56).map(move |y| (x, y)))
                    .map(|(x, y)| c.data[(y * 758 + x) as usize] as u32)
                    .collect::<Vec<_>>()
            };
            let (on, off) = (
                bar(&mut ts, "12:23", Some(80), true, small),
                bar(&mut ts, "12:23", Some(80), false, small),
            );
            assert_ne!(
                region(&on, bx, bx + icons::BATTERY_W),
                region(&off, bx, bx + icons::BATTERY_W)
            );
            assert!(
                region(&on, bx - 11, bx - 1).iter().all(|&v| v == 255),
                "encre entre l'heure et la batterie"
            );
            if let Ok(dir) = std::env::var("SIGNET_UI_DUMP") {
                let mut both = Canvas::new(758, 160);
                both.data[..758 * 80]
                    .copy_from_slice(&bar(&mut ts, "12:23", Some(80), false, small).data);
                both.data[758 * 80..]
                    .copy_from_slice(&bar(&mut ts, "12:23", Some(80), true, small).data);
                both.save_pgm(&format!("{dir}/barre-{small}.pgm")).unwrap();
            }
        }
    }

    /// Preview of the screens with icons (selection bar, definition, USB, sleep):
    /// SIGNET_UI_DUMP=folder.
    #[test]
    fn preview_screens() {
        let Ok(dir) = std::env::var("SIGNET_UI_DUMP") else {
            return;
        };
        let mut ts = crate::typesetter();
        let cfg = crate::reader::PageConfig {
            px: 30.0,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        let status = Status {
            title: "La méthode Make Time",
            clock: "14:40",
            battery: Some(62),
            charging: true,
        };
        let mut c = Canvas::new(758, 1024);
        let page = crate::reader::Page::default();
        let chrome = Chrome {
            label: "27 / 515",
            ..Default::default()
        };
        reader(
            &mut c,
            &mut ts,
            &status,
            &page,
            &cfg,
            &chrome,
            &[],
            Some(SelectionBar::New),
        );
        c.save_pgm(&format!("{dir}/apercu-selection.pgm")).unwrap();
        reader(
            &mut c,
            &mut ts,
            &status,
            &page,
            &cfg,
            &chrome,
            &[],
            Some(SelectionBar::Existing(0)),
        );
        c.save_pgm(&format!("{dir}/apercu-selection2.pgm")).unwrap();
        usb_screen(&mut c, &mut ts);
        c.save_pgm(&format!("{dir}/apercu-usb.pgm")).unwrap();
        sleep_screen(&mut c, &mut ts, None, "En veille", Icon::Moon);
        c.save_pgm(&format!("{dir}/apercu-veille.pgm")).unwrap();
    }
}

#[cfg(test)]
mod tests_liens {
    use super::*;
    use crate::reader::{paginate, PageConfig};

    /// On a real book (SIGNET_EPUB_SAMPLE): the first note call is touched
    /// on its glyph and a little beside it, not far from it; its note is composed in the
    /// window. Previews (page with return button, note): SIGNET_UI_DUMP=folder.
    #[test]
    fn note_call_hit_and_popup() {
        let Ok(path) = std::env::var("SIGNET_EPUB_SAMPLE") else {
            return;
        };
        let mut book = signet_epub::Book::open(&path).unwrap();
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
        let found = (0..book.spine.len()).find_map(|ci| {
            let ch = book.chapter_full(ci).ok()?;
            let li = ch.links.iter().position(|l| l.note && l.from > 0)?;
            Some((ci, ch, li))
        });
        let (ci, ch, li) = found.expect("un appel de note dans le livre");
        let link = &ch.links[li];
        let pages = paginate(&ch.blocks, &ts, &cfg, &mut |_: &str| None);
        let page = pages
            .iter()
            .find(|p| !range_rects(p, &cfg, link.block, link.from, link.to).is_empty())
            .expect("page de l'appel");
        let r = range_rects(page, &cfg, link.block, link.from, link.to)[0];
        let (cx, cy) = (r.x as i32 + r.w as i32 / 2, r.y as i32 + r.h as i32 / 2);
        assert_eq!(
            link_at(page, &cfg, &ch.links, cx, cy),
            Some(li),
            "sur le glyphe"
        );
        assert_eq!(
            link_at(page, &cfg, &ch.links, r.x as i32 + r.w as i32 + 18, cy),
            Some(li),
            "un peu à droite"
        );
        assert_eq!(
            link_at(page, &cfg, &ch.links, cx, cy + 12),
            Some(li),
            "un peu dessous"
        );
        assert_eq!(
            link_at(page, &cfg, &ch.links, r.x as i32 + r.w as i32 + 60, cy),
            None,
            "trop loin"
        );
        // The note: the target element, at the start of its block.
        let a = ch
            .anchors
            .get(link.fragment.as_deref().unwrap())
            .copied()
            .expect("ancre de la note");
        assert_eq!(a.ch, 0);
        let note_block = crate::reader::strip_note_number(&ch.blocks[a.block], "1");
        let text = note_block.text();
        assert!(!text.starts_with('1'), "numéro retiré : {text:?}");
        let mut ts = ts;
        if let Ok(dir) = std::env::var("SIGNET_UI_DUMP") {
            let status = Status {
                title: "La méthode Make Time",
                clock: "14:40",
                battery: Some(62),
                charging: false,
            };
            let mut c = Canvas::new(758, 1024);
            reader(
                &mut c,
                &mut ts,
                &status,
                page,
                &cfg,
                &Chrome {
                    label: "27 / 515",
                    ..Default::default()
                },
                &[],
                None,
            );
            note(&mut c, &mut ts, "Note 1", &[note_block], 30.0, 0);
            c.save_pgm(&format!("{dir}/apercu-note.pgm")).unwrap();
            reader(
                &mut c,
                &mut ts,
                &status,
                page,
                &cfg,
                &Chrome {
                    label: "27 / 515",
                    back: Some("Revenir à la page 12"),
                    bookmarked: true,
                    night: false,
                },
                &[],
                None,
            );
            c.save_pgm(&format!("{dir}/apercu-retour.pgm")).unwrap();
        }
        eprintln!(
            "chapitre {ci} : appel « {} » → {:?}, note : {text:?}",
            link.label, link.fragment
        );
    }
}

#[cfg(test)]
mod tests_encadres {
    use super::*;
    use crate::reader::{paginate, PageConfig};

    /// The book's "white on black" callout: dark background painted, text written in
    /// white (before: background ignored, black text on white).
    #[test]
    fn dark_frame_text_is_white() {
        let src = r##"<html xmlns="http://www.w3.org/1999/xhtml"><head><style>.dark { background-color: #222222; color: #ffffff; padding: 1em }</style></head>
<body><p>Avant l'encadré.</p><div class="dark"><p>Texte blanc sur fond noir, assez long pour remplir une ligne entière de la colonne.</p></div><p>Après.</p></body></html>"##;
        let blocks = signet_epub::parse_xhtml(src, "x").unwrap();
        let mut ts = crate::typesetter();
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
        let pages = paginate(&blocks, &ts, &cfg, &mut |_: &str| None);
        let f = pages[0].frames.first().expect("encadré").clone();
        assert_eq!(f.background, Some(0x22));
        let mut c = Canvas::new(758, 1024);
        let status = Status {
            title: "",
            clock: "",
            battery: None,
            charging: false,
        };
        reader(
            &mut c,
            &mut ts,
            &status,
            &pages[0],
            &cfg,
            &Chrome::default(),
            &[],
            None,
        );
        // In the callout: mostly dark background, and white pixels (the text).
        let (x0, x1) = (
            cfg.margin_x + f.x0 as i32 + 10,
            cfg.margin_x + f.x1 as i32 - 10,
        );
        let (y0, y1) = (
            cfg.margin_top + f.y0 as i32 + 10,
            cfg.margin_top + f.y1 as i32 - 10,
        );
        let (mut dark, mut white, mut n) = (0, 0, 0);
        for y in y0..y1 {
            for x in x0..x1 {
                let v = c.data[(y * 758 + x) as usize];
                n += 1;
                if v < 0x40 {
                    dark += 1
                }
                if v > 0xD0 {
                    white += 1
                }
            }
        }
        assert!(dark * 2 > n, "fond sombre : {dark}/{n}");
        assert!(white > 200, "texte blanc : {white} pixels");
    }
}
