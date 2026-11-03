//! The application state machine: Home ⇄ Reading (+ Settings).

use crate::annotations::{Annotation, Annotations};
use crate::backend::{Input, Screen};
use crate::definition;
use crate::icons::Icon;
use crate::library::{BookEntry, Library};
use crate::reader::{paginate, Page, PageConfig};
use crate::state::{Position, State};
use crate::theme::*;
use crate::ui::{self, Hit, HitZone, SelectionBar, SettingsView, Status};
use signet_epub::Book;
use signet_fb::{Canvas, Rect, Waveform};
use signet_input::{Event, Phase};
use signet_text::Typesetter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Home,
    Reader,
    Settings,
    Toc,
    Usb,
    Library,
    Annotations,
    Definition,
    Note,
    GoToPage,
    Search,
    BookMenu,
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Selection in progress on the page: a character range within a block.
#[derive(Clone, Copy, Debug)]
struct Selection {
    block: usize,
    from: u32,
    to: u32,
    /// An existing highlight that was touched (index), or a new selection.
    existing: Option<usize>,
}

struct Open {
    path: PathBuf,
    book: Book,
    chapter: usize,
    pages: Vec<Page>,
    page: usize,
    cache_cfg: Option<PageConfig>,
    /// Character count per chapter (to estimate the pages of chapters not yet
    /// counted). Empty while not needed: once every page of the book is counted,
    /// opening the book does not have to read each chapter.
    chars: Vec<usize>,
    /// Texts of the current chapter's blocks (anchoring of highlights).
    block_texts: Vec<String>,
    /// Links and identified elements of the current chapter.
    links: Vec<signet_epub::Link>,
    anchors: HashMap<String, signet_epub::Anchor>,
    /// Next chapter pre-composed: (index, configuration, composition).
    next: Option<(usize, PageConfig, Composed)>,
    annotations: Annotations,
    /// Book images: display sizes and decoded pixels.
    images: crate::reader::BookImages,
    /// Exact page count of each chapter for `counts_cfg`, counted as reading goes
    /// and during idle time, kept in `.signet/covers/<book>.pages`.
    counts: Vec<Option<usize>>,
    counts_cfg: Option<PageConfig>,
    counts_saved: bool,
    /// Passage to find again at the next layout of the chapter (saved position,
    /// or the current page before a setting change).
    pending_anchor: Option<(usize, u32)>,
    /// Passage actually read, as long as no page has been turned since it was found
    /// again: the displayed page starts a little before it, and successive size changes
    /// would otherwise move the reading back one page each time.
    anchor_hold: Option<(usize, u32)>,
    /// Target element in another chapter (link, table-of-contents entry): its passage
    /// is only known once that chapter has been read.
    pending_fragment: Option<String>,
    /// After a followed link: where to go back to (chapter, passage) and the button label.
    link_back: Option<(usize, (usize, u32), String)>,
    /// Block texts and anchors of each chapter, read at the first search.
    search_texts: Option<Vec<Vec<String>>>,
    all_anchors: Option<Vec<HashMap<String, signet_epub::Anchor>>>,
    /// Passage found by the search, marked on its page: (chapter, block, from, to).
    search_mark: Option<(usize, usize, u32, u32)>,
}

/// Search in the book: input, keyboard shown, results.
#[derive(Default)]
struct SearchState {
    query: String,
    keyboard: bool,
    results: Vec<crate::search::Found>,
    /// The results match the input (otherwise: not searched yet).
    done: bool,
    page: usize,
    /// Search in progress ("Recherche…" displayed).
    busy: bool,
}

/// Cap on the number of results of a search.
const SEARCH_MAX: usize = 200;

/// Refresh of a screen already drawn in the canvas.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Refresh {
    Full,
    Soft,
    Field,
}

/// A chapter composed ahead of time: pages, block texts, links, anchors.
struct Composed {
    pages: Vec<Page>,
    texts: Vec<String>,
    links: Vec<signet_epub::Link>,
    anchors: HashMap<String, signet_epub::Anchor>,
}

/// Displayed definition: the word, its entries (or nearby words), the page,
/// the targets of the window's cross-references and the previous words (followed references).
struct DictState {
    word: String,
    entries: Vec<signet_dict::RichEntry>,
    suggestions: Vec<String>,
    missing: bool,
    page: usize,
    links: Vec<String>,
    back: Vec<String>,
}

/// Note opened over the page: its number, its blocks, and where it is.
struct NoteView {
    title: String,
    blocks: Vec<signet_epub::Block>,
    chapter: usize,
    fragment: Option<String>,
}

/// Characters of a full page, to estimate the chapters not yet counted:
/// half an em per character on average, lines three-quarters full
/// (paragraph ends, headings, images).
fn default_chars_per_page(cfg: &PageConfig) -> f32 {
    let per_line = cfg.text_width() / (cfg.px * 0.5);
    let lines = cfg.text_height() / (cfg.px * cfg.line_height);
    (per_line * lines * 0.78).max(200.0)
}

/// Start of a page: (block, character) of the first element.
fn page_anchor(page: &Page) -> Option<(usize, u32)> {
    let it = page.items.first()?;
    let ch = it
        .lines
        .first()
        .and_then(|l| l.glyphs.first())
        .map(|g| g.ch)
        .unwrap_or(0);
    Some((it.block, ch))
}

/// The page that contains the passage `anchor`: the last one that starts before it.
fn page_for_anchor(pages: &[Page], anchor: (usize, u32)) -> usize {
    pages
        .iter()
        .enumerate()
        .filter(|(_, p)| page_anchor(p).map(|a| a <= anchor).unwrap_or(false))
        .map(|(i, _)| i)
        .next_back()
        .unwrap_or(0)
}

/// Number of a note call: "¹²" → "12", "[3]" → "3".
fn note_number(label: &str) -> String {
    label
        .trim()
        .chars()
        .filter_map(|c| match c {
            '⁰' => Some('0'),
            '¹' => Some('1'),
            '²' => Some('2'),
            '³' => Some('3'),
            '⁴'..='⁹' => char::from_digit(c as u32 - '⁴' as u32 + 4, 10),
            '[' | ']' | '(' | ')' => None,
            c => Some(c),
        })
        .collect()
}

/// Fingerprint of what determines pagination: page configuration and composer
/// version (a new binary may break lines differently).
fn pages_fingerprint(cfg: &PageConfig) -> String {
    format!(
        "{:?} {}",
        cfg,
        option_env!("SIGNET_BUILD").unwrap_or(env!("CARGO_PKG_VERSION"))
    )
}

impl Open {
    /// Chapter read: the element targeted by a link or the table of contents becomes the passage to display.
    fn resolve_fragment(&mut self) {
        if let Some(f) = self.pending_fragment.take() {
            if let Some(a) = self.anchors.get(&f) {
                self.pending_anchor = Some((a.block, a.ch));
                self.anchor_hold = Some((a.block, a.ch));
            }
        }
    }

    /// Counts the characters of each chapter, if not already done.
    fn ensure_chars(&mut self) {
        if self.chars.len() != self.book.spine.len() {
            let book = &mut self.book;
            self.chars = (0..book.spine.len())
                .map(|i| {
                    book.chapter(i)
                        .map(|b| b.iter().map(|b| b.text().chars().count()).sum())
                        .unwrap_or(0)
                })
                .collect();
        }
    }

    /// Records the page count of a chapter (and forgets the counts of another configuration).
    fn record_count(&mut self, cfg: PageConfig, chapter: usize, n: usize) {
        if self.counts_cfg != Some(cfg) {
            self.counts = vec![None; self.book.spine.len()];
            self.counts_cfg = Some(cfg);
            self.counts_saved = false;
        }
        if let Some(c) = self.counts.get_mut(chapter) {
            if *c != Some(n) {
                *c = Some(n);
                self.counts_saved = false;
            }
        }
    }
}

pub struct App<S: Screen, I: Input> {
    screen: S,
    input: I,
    canvas: Canvas,
    pub ts: Typesetter,
    state: State,
    state_path: String,
    library: Library,
    library_root: PathBuf,
    cache_dir: PathBuf,
    view: View,
    zones: Vec<HitZone>,
    open: Option<Open>,
    /// Counts partial refreshes since the last full one: a GC16 is
    /// forced every N pages to clear ghosting.
    partial_since_full: u32,
    /// Time, battery and power currently displayed in the status bar.
    shown_status: Option<(String, Option<u8>, bool)>,
    /// Date displayed on the home screen or the library.
    shown_date: Option<String>,
    /// Position turned to but not yet written to the partition.
    position_dirty: bool,
    /// USB probe in progress (gadget loaded without backing media): deadline.
    usb_probe: Option<Instant>,
    /// State of the power indicator (on = cable plugged in).
    led_on: Option<bool>,
    touch_down: Option<(i32, i32, Instant)>,
    /// The finger has moved since the touch-down (drag candidate, not a long press).
    touch_moved: bool,
    /// Selection in progress by finger (press held, then drag).
    drag: Option<(u32, u32)>,
    /// Last "live" refresh and touched area (to clean it up).
    live_at: Option<Instant>,
    live_rect: Option<Rect>,
    /// Page without selection, rendered once at the start of the gesture.
    live_base: Option<Vec<u8>>,
    /// The selection has changed since the last live render.
    live_dirty: bool,
    power_down: Option<Instant>,
    pub exit_code: i32,
    toc_page: usize,
    library_page: usize,
    usb_plugged: bool,
    last_hook: Option<Instant>,
    shape_saved: usize,
    quit: bool,
    selection: Option<Selection>,
    annotations_page: usize,
    last_activity: Instant,
    /// Dictionary opened on demand: (language code, dictionary).
    dict: Option<(String, Option<signet_dict::Dictionary>)>,
    note: Option<NoteView>,
    dict_state: Option<DictState>,
    /// Digits typed in "Aller à la page".
    goto_input: String,
    /// First tap on "Passer en lecture" (the second one, within 5 s, confirms).
    dev_confirm: Option<Instant>,
    /// Menu of a library book: (index, deletion awaiting confirmation).
    book_menu: Option<(usize, bool)>,
    search: SearchState,
    applied_light: Option<u8>,
    manual_light_at: Option<Instant>,
    last_light_check: Option<Instant>,
    /// True while the partition is exported over USB (onboard unmounted).
    usb_exported: bool,
    log: Journal,
}

/// Log: kept in memory and written immediately to stderr (which the daemon
/// redirects to `.signet/signet.log`), so that it can be read while Signet
/// is running.
struct Journal(Vec<String>);
impl Journal {
    fn push(&mut self, s: String) {
        eprintln!("{s}");
        self.0.push(s);
    }
}

const FULL_EVERY: u32 = 6;
const FULL_EVERY_NIGHT: u32 = 4;

impl<S: Screen, I: Input> App<S, I> {
    pub fn new(screen: S, input: I, ts: Typesetter, library_root: &Path, state_dir: &Path) -> Self {
        let (w, h) = screen.size();
        let ts_shaped_at_start = ts.shaped_len();
        let state_path = state_dir.join("state.txt").to_string_lossy().into_owned();
        let state = State::load(&state_path);
        let cache_dir = state_dir.join("covers");
        let library = Library::scan(library_root, &cache_dir, 230, 345);
        let mut app = App {
            screen,
            input,
            canvas: Canvas::new(w, h),
            ts,
            state,
            state_path,
            library,
            library_root: library_root.to_path_buf(),
            cache_dir,
            view: View::Home,
            zones: Vec::new(),
            open: None,
            partial_since_full: 0,
            shown_status: None,
            shown_date: None,
            position_dirty: false,
            usb_probe: None,
            led_on: None,
            touch_down: None,
            touch_moved: false,
            drag: None,
            live_at: None,
            live_rect: None,
            live_base: None,
            live_dirty: false,
            power_down: None,
            exit_code: 0,
            toc_page: 0,
            library_page: 0,
            usb_plugged: crate::device::usb_plugged(),
            last_hook: None,
            // The cache has just been reloaded: nothing to rewrite as long as no new word
            // has been shaped (2.3 MB on the partition at every return to the home screen).
            shape_saved: ts_shaped_at_start,
            quit: false,
            selection: None,
            annotations_page: 0,
            last_activity: Instant::now(),
            dict: None,
            note: None,
            dict_state: None,
            goto_input: String::new(),
            dev_confirm: None,
            book_menu: None,
            search: SearchState::default(),
            applied_light: None,
            manual_light_at: None,
            last_light_check: None,
            usb_exported: false,
            log: Journal(Vec::new()),
        };
        app.sort_recent();
        app.refresh_progress();
        app
    }

    /// Library in "recents" order: books that have been opened, from last read
    /// to oldest, then the others from most recently copied to oldest.
    /// The home screen shows it in this order.
    fn sort_recent(&mut self) {
        let seen = &self.state.seen;
        self.library.books.sort_by_key(|b| {
            (
                std::cmp::Reverse(
                    seen.get(b.path.to_string_lossy().as_ref())
                        .copied()
                        .unwrap_or(0),
                ),
                std::cmp::Reverse(b.mtime),
            )
        });
    }

    /// Display order of the full library: index into `library.books`.
    fn library_order(&self) -> Vec<usize> {
        let books = &self.library.books;
        let fold = |s: &str| s.chars().map(crate::search::fold).collect::<String>();
        // Author: by the first author's family name ("Jake Knapp" → "knapp jake").
        let author_key = |a: &str| {
            let first = a
                .split([',', '&', ';'])
                .next()
                .unwrap_or("")
                .split(" et ")
                .next()
                .unwrap_or("")
                .split(" and ")
                .next()
                .unwrap_or("")
                .trim();
            let mut words: Vec<&str> = first.split_whitespace().collect();
            if let Some(last) = words.pop() {
                words.insert(0, last);
            }
            fold(&words.join(" "))
        };
        let mut idx: Vec<usize> = (0..books.len()).collect();
        match self.state.settings.library_sort {
            1 => idx.sort_by_cached_key(|&i| fold(&books[i].title)),
            2 => idx.sort_by_cached_key(|&i| (author_key(&books[i].author), fold(&books[i].title))),
            _ => {}
        }
        idx
    }

    /// Deletes a book from the reader: the file, its caches (cover,
    /// metadata, pages), its annotations, its position.
    fn delete_book(&mut self, i: usize) {
        let Some(book) = self.library.books.get(i).cloned() else {
            return;
        };
        let key = book.path.to_string_lossy().into_owned();
        if self
            .open
            .as_ref()
            .map(|o| o.path == book.path)
            .unwrap_or(false)
        {
            self.open = None;
        }
        let cache = crate::library::cache_key(&book.path);
        let annotations = Path::new(&self.state_path)
            .with_file_name("annotations")
            .join(format!("{cache}.txt"));
        let cache_dir = self.cache_dir.clone();
        let removed = crate::device::with_rw(|| {
            let r = std::fs::remove_file(&book.path);
            if let Ok(rd) = std::fs::read_dir(&cache_dir) {
                for e in rd.filter_map(|e| e.ok()) {
                    if e.file_name().to_string_lossy().starts_with(&cache) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let _ = std::fs::remove_file(&annotations);
            r
        });
        match removed {
            Ok(()) => {
                self.library.books.remove(i);
                self.state.positions.remove(&key);
                self.state.finished.remove(&key);
                self.state.seen.remove(&key);
                if self.state.last_book.as_deref() == Some(key.as_str()) {
                    self.state.last_book = None;
                }
                self.state.save();
                self.log
                    .push(format!("livre supprimé : {}", book.path.display()));
            }
            Err(e) => self.log.push(format!(
                "suppression impossible de {} : {e}",
                book.path.display()
            )),
        }
    }

    fn render_book_menu(&mut self) {
        let Some((i, confirm)) = self.book_menu else {
            return;
        };
        let Some(book) = self.library.books.get(i).cloned() else {
            return;
        };
        let (zones, rect) = ui::book_menu(&mut self.canvas, &mut self.ts, &book, i, confirm);
        self.zones = zones;
        self.view = View::BookMenu;
        self.screen.snapshot(&self.canvas, "13-menu-livre");
        self.present_soft(rect);
    }

    /// Time and battery of the status bar, remembered as displayed (the render
    /// that follows draws them).
    fn status(&mut self, title: &str) -> (String, Option<u8>, bool) {
        let s = self.status_now(title);
        self.shown_status = Some(s.clone());
        s
    }

    /// Time, battery, power plugged in. The dev-mode IP address is no longer in
    /// the status bar (which is for reading): it is in the reading panel.
    fn status_now(&self, title: &str) -> (String, Option<u8>, bool) {
        let _ = title;
        let clock = crate::device::clock(
            self.state.settings.utc_offset_minutes,
            self.state.settings.eu_dst,
        );
        (
            clock,
            crate::device::battery_percent(),
            crate::device::power_online(),
        )
    }

    /// Today's date, for the home screen's status bar: "Jeudi 22 octobre".
    fn today(&self) -> String {
        let d = crate::device::local_date(
            self.state.settings.utc_offset_minutes,
            self.state.settings.eu_dst,
        );
        let mut c = d.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    }

    /// The displayed time (or battery) has changed since the last render: only
    /// this corner of the status bar is redrawn, with a flashless partial refresh.
    /// Nothing during a selection, a touch, under a menu or an overlay.
    fn tick_status(&mut self) {
        // The indicator follows the cable, whatever the screen.
        let powered = crate::device::power_online();
        if self.led_on != Some(powered) {
            crate::device::set_power_led(powered);
            self.led_on = Some(powered);
            self.log.push(format!(
                "alimentation {} : voyant {}",
                if powered { "branchée" } else { "débranchée" },
                if powered { "allumé" } else { "éteint" }
            ));
        }
        let small = match self.view {
            View::Reader | View::Toc | View::Annotations => true,
            View::Home | View::Library => false,
            _ => return,
        };
        if self.selection.is_some()
            || self.drag.is_some()
            || self.touch_down.is_some()
            || self.usb_exported
        {
            return;
        }
        // Midnight on the home screen or library: the date changes, the screen is redrawn.
        if matches!(self.view, View::Home | View::Library)
            && self.shown_date.as_deref() != Some(self.today().as_str())
        {
            if self.view == View::Home {
                self.render_home()
            } else {
                self.render_library()
            }
            return;
        }
        let Some(old) = self.shown_status.clone() else {
            return;
        };
        let now = self.status_now("");
        if now == old {
            return;
        }
        let st = |s: &(String, Option<u8>, bool)| (s.0.clone(), s.1, s.2);
        let ((nc, nb, nch), (oc, ob, och)) = (st(&now), st(&old));
        let r = ui::refresh_status_right(
            &mut self.canvas,
            &mut self.ts,
            &Status {
                title: "",
                clock: &nc,
                battery: nb,
                charging: nch,
            },
            &Status {
                title: "",
                clock: &oc,
                battery: ob,
                charging: och,
            },
            small,
        );
        // Page in night mode: the status bar is inverted there, and so is its corner.
        if self.view == View::Reader && self.state.settings.night {
            self.canvas.invert_rect(r);
        }
        self.present(r, Waveform::Gl16, false);
        self.shown_status = Some(now);
    }

    fn page_config(&self) -> PageConfig {
        let s = &self.state.settings;
        PageConfig {
            px: READER_SIZES[s.size.min(2)],
            line_height: READER_LEADINGS[s.leading.min(2)],
            margin_x: READER_MARGINS[s.margins.min(2)],
            margin_top: STATUS_H + 28,
            margin_bottom: 64,
            screen_w: self.canvas.width as i32,
            screen_h: self.canvas.height as i32,
            family: (s.font.min(self.ts.families().saturating_sub(1))) as u8,
        }
    }

    fn current_entry(&self) -> Option<&BookEntry> {
        let last = self.state.last_book.as_deref()?;
        self.library
            .books
            .iter()
            .find(|b| b.path.to_string_lossy() == last)
    }

    // ------------------------------------------------------------ render ----

    fn present(&mut self, rect: Rect, wfm: Waveform, full: bool) {
        self.screen.present(&self.canvas, rect, wfm, full);
    }

    /// Update of a screen already displayed (setting, next page of a list,
    /// panel opening over the page): partial and flashless, with the full cleanup
    /// coming in its turn, as for pages.
    fn present_soft(&mut self, r: Rect) {
        let full = self.partial_since_full >= FULL_EVERY;
        self.present(r, if full { Waveform::Gc16 } else { Waveform::Gl16 }, full);
        self.partial_since_full = if full { 0 } else { self.partial_since_full + 1 };
    }

    fn full_rect(&self) -> Rect {
        Rect::new(0, 0, self.canvas.width, self.canvas.height)
    }

    pub fn render_home(&mut self) {
        let (clock, battery, charging) = self.status("Accueil");
        // The home screen does not need to say it is the home screen: today's date.
        let today = self.today();
        self.shown_date = Some(today.clone());
        let status = Status {
            title: &today,
            clock: &clock,
            battery,
            charging,
        };
        let current_idx = self.current_entry().map(|b| b.path.clone());
        let current = current_idx
            .as_ref()
            .and_then(|p| self.library.books.iter().find(|b| &b.path == p));
        let zones = ui::home(
            &mut self.canvas,
            &mut self.ts,
            &status,
            current,
            &self.library.books,
        );
        self.zones = zones;
        self.view = View::Home;
        self.screen.snapshot(&self.canvas, "01-accueil");
        let r = self.full_rect();
        self.present(r, Waveform::Gc16, true);
        self.partial_since_full = 0;
    }

    fn ensure_pages(&mut self) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_mut() else { return };
        // Pages not all counted for this configuration: the estimates need
        // the chapter sizes.
        if o.chars.is_empty() && (o.counts_cfg != Some(cfg) || o.counts.iter().any(|c| c.is_none()))
        {
            let t0 = Instant::now();
            o.ensure_chars();
            self.log.push(format!(
                "tailles des chapitres lues en {:.0} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            ));
        }
        if o.cache_cfg == Some(cfg) && !o.pages.is_empty() {
            return;
        }
        if let Some((c, cfg2, _)) = o.next.as_ref() {
            if *c == o.chapter && *cfg2 == cfg {
                let (_, _, comp) = o.next.take().unwrap();
                o.pages = comp.pages;
                o.block_texts = comp.texts;
                o.links = comp.links;
                o.anchors = comp.anchors;
                o.resolve_fragment();
                if o.pages.is_empty() {
                    o.pages.push(Page::default());
                }
                let (ch, n) = (o.chapter, o.pages.len());
                o.record_count(cfg, ch, n);
                o.cache_cfg = Some(cfg);
                o.page = match o.pending_anchor.take() {
                    Some(a) => page_for_anchor(&o.pages, a),
                    None => o.page.min(o.pages.len() - 1),
                };
                self.log
                    .push(format!("chapitre {} : pages précomposées, 0 ms", o.chapter));
                return;
            }
        }
        let t0 = Instant::now();
        let (s0, h0, k0) = (
            self.ts.t_shape.get(),
            self.ts.t_hyph.get(),
            self.ts.t_kp.get(),
        );
        let chapter = o.book.chapter_full(o.chapter).unwrap_or_default();
        let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let blocks = chapter.blocks;
        o.block_texts = blocks.iter().map(|b| b.text()).collect();
        o.links = chapter.links;
        o.anchors = chapter.anchors;
        o.resolve_fragment();
        let book = &mut o.book;
        let images = &mut o.images;
        let t_img0 = images.t_sizes;
        let mut image = |href: &str| images.size(book, href, &cfg);
        o.pages = paginate(&blocks, &self.ts, &cfg, &mut image);
        let t_img = o.images.t_sizes.saturating_sub(t_img0);
        if o.pages.is_empty() {
            o.pages.push(Page::default());
        }
        let (ch, n) = (o.chapter, o.pages.len());
        o.record_count(cfg, ch, n);
        o.cache_cfg = Some(cfg);
        o.page = match o.pending_anchor.take() {
            Some(a) => page_for_anchor(&o.pages, a),
            None => o.page.min(o.pages.len() - 1),
        };
        let (hits, misses) = (self.ts.shape_hits.get(), self.ts.shape_misses.get());
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
        self.log.push(format!(
            "chapitre {} : {} blocs → {} pages en {:.1} ms (parse {parse_ms:.1} ms, tailles d'images {:.0} ms, façonnage {} mots dont {} en cache, {:.0} ms, césure {:.0} ms, Knuth-Plass {:.0} ms)",
            o.chapter, blocks.len(), o.pages.len(), ms(t0.elapsed()), ms(t_img), hits + misses, hits,
            ms(self.ts.t_shape.get().saturating_sub(s0)), ms(self.ts.t_hyph.get().saturating_sub(h0)), ms(self.ts.t_kp.get().saturating_sub(k0))
        ));
        self.ts.shape_hits.set(0);
        self.ts.shape_misses.set(0);
    }

    /// Pixels of the current page's images (decoded once, at their size).
    fn resolve_page_images(&mut self) {
        let Some(o) = self.open.as_mut() else { return };
        let Some(page) = o.pages.get(o.page) else {
            return;
        };
        let (book, images) = (&mut o.book, &mut o.images);
        let t0 = images.t_decode;
        crate::reader::resolve_images(page, &mut |href, w, h| images.pixels(book, href, w, h));
        let dt = images.t_decode.saturating_sub(t0);
        if !dt.is_zero() {
            self.log.push(format!(
                "images de la page décodées en {:.0} ms",
                dt.as_secs_f64() * 1000.0
            ));
        }
    }

    /// Bookmarks placed on the displayed page (index into the annotations).
    fn page_bookmarks(o: &Open) -> Vec<usize> {
        match o.pages.get(o.page).and_then(page_anchor) {
            Some(start) => o.annotations.bookmarks_between(
                o.chapter,
                start,
                o.pages.get(o.page + 1).and_then(page_anchor),
            ),
            None => Vec::new(),
        }
    }

    /// Draws the current page into the canvas (zones, view), without displaying it.
    /// Returns: whether the page has an image, drawing time.
    fn draw_reader(&mut self) -> Option<(bool, f64)> {
        self.ensure_pages();
        self.resolve_page_images();
        let cfg = self.page_config();
        let (clock, battery, charging) = self.status("");
        let o = self.open.as_ref()?;
        let title = o.book.metadata.title.clone();
        let status = Status {
            title: &title,
            clock: &clock,
            battery,
            charging,
        };
        let label = Self::book_page_label(o);
        let page = o.pages[o.page].clone();
        let mut marks: Vec<ui::Mark> = o
            .annotations
            .in_chapter(o.chapter)
            .into_iter()
            .map(|(b, f, t, _)| (b, f, t, GRAY_LIGHT))
            .collect();
        // Passage found by the search: marked like a selection.
        if let Some((_, b, f, t)) = o.search_mark.filter(|m| m.0 == o.chapter) {
            marks.push((b, f, t, GRAY));
        }
        let bar = self.selection.map(|sel| {
            marks.push((sel.block, sel.from, sel.to, GRAY));
            match sel.existing {
                Some(i) => SelectionBar::Existing(i),
                None => SelectionBar::New,
            }
        });
        let back = o.link_back.as_ref().map(|(_, _, l)| l.clone());
        let chrome = ui::Chrome {
            label: &label,
            back: back.as_deref(),
            bookmarked: !Self::page_bookmarks(o).is_empty(),
            night: self.state.settings.night,
        };
        let t0 = Instant::now();
        let zones = ui::reader(
            &mut self.canvas,
            &mut self.ts,
            &status,
            &page,
            &cfg,
            &chrome,
            &marks,
            bar,
        );
        let draw_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.zones = zones;
        self.view = View::Reader;
        self.screen.snapshot(&self.canvas, "02-lecture");
        Some((
            page.items
                .iter()
                .any(|it| matches!(it.kind, crate::reader::ItemKind::Image(_))),
            draw_ms,
        ))
    }

    pub fn render_reader(&mut self, wfm: Waveform) {
        let Some((has_image, draw_ms)) = self.draw_reader() else {
            return;
        };
        let wfm = if wfm == Waveform::Du && has_image {
            Waveform::Glr16
        } else {
            wfm
        };
        // White on black leaves more trails: more frequent full cleanup.
        let every = if self.state.settings.night {
            FULL_EVERY_NIGHT
        } else {
            FULL_EVERY
        };
        let full = wfm == Waveform::Gc16 || self.partial_since_full >= every;
        let r = self.full_rect();
        self.present(r, if full { Waveform::Gc16 } else { wfm }, full);
        if full {
            self.partial_since_full = 0;
        } else {
            self.partial_since_full += 1;
        }
        self.log.push(format!(
            "page dessinée en {draw_ms:.1} ms ({})",
            if full { "GC16 plein" } else { wfm.name() }
        ));
    }

    /// "72 / 400" at book scale: we calibrate "characters per page"
    /// on the current chapter (actually paginated) and extrapolate to the others.
    fn book_page_label(o: &Open) -> String {
        let (page, total, _) = Self::page_numbers(o);
        format!("{page} / {total}")
    }

    /// (page read, pages in the book, exact): exact when the whole book is counted.
    fn page_numbers(o: &Open) -> (usize, usize, bool) {
        let (pages, exact) = Self::chapter_pages(o);
        let before: usize = pages[..o.chapter.min(pages.len())].iter().sum();
        let total: usize = pages.iter().sum::<usize>().max(1);
        ((before + o.page + 1).min(total), total, exact)
    }

    /// Pages of each chapter, and whether all are counted.
    fn chapter_pages(o: &Open) -> (Vec<usize>, bool) {
        // Known pages: counted for the current configuration (the open chapter
        // always is). The others are estimated at the average rate of the counted
        // chapters, not of the current chapter alone (a table of contents or an
        // image chapter skewed the whole book).
        let known = |i: usize| -> Option<usize> {
            if i == o.chapter {
                Some(o.pages.len().max(1))
            } else if o.counts_cfg == o.cache_cfg {
                o.counts.get(i).copied().flatten()
            } else {
                None
            }
        };
        // Chapter sizes are only read if pages remain to be estimated.
        let n_ch = o.book.spine.len();
        let chars = |i: usize| o.chars.get(i).copied().unwrap_or(0);
        let (kc, kp) = (0..n_ch)
            .filter_map(|i| known(i).map(|n| (chars(i), n)))
            .fold((0usize, 0usize), |(c, p), (cc, pp)| (c + cc, p + pp));
        // Calibrate on enough text only: at first opening, the only counted
        // chapter is often the cover (one page, almost no text), which gave
        // "1 / 1712" for a 515-page book. Otherwise, the full page.
        let cpp = if kp > 0 && kc >= 4000 {
            (kc as f32 / kp as f32).max(200.0)
        } else {
            o.cache_cfg
                .map(|c| default_chars_per_page(&c))
                .unwrap_or(650.0)
        };
        let pages_of =
            |i: usize| known(i).unwrap_or_else(|| ((chars(i) as f32 / cpp).ceil() as usize).max(1));
        (
            (0..n_ch).map(pages_of).collect(),
            (0..n_ch).all(|i| known(i).is_some()),
        )
    }

    /// Progress of each library book, in pages when the book is
    /// counted (the same as the page footer), otherwise by chapters.
    fn refresh_progress(&mut self) {
        let cfg = self.page_config();
        for i in 0..self.library.books.len() {
            let path = self.library.books[i].path.clone();
            let key = path.to_string_lossy().into_owned();
            let finished = self.state.finished.get(&key).copied().unwrap_or(false);
            let pos = self.state.positions.get(&key).copied();
            let (percent, pages) = match self
                .open
                .as_ref()
                .filter(|o| o.path == path && !o.pages.is_empty())
            {
                Some(o) => {
                    let (page, total, exact) = Self::page_numbers(o);
                    ((page * 100 / total) as u8, exact.then_some((page, total)))
                }
                None => match (pos, self.saved_counts(&path, &cfg)) {
                    (Some(p), Some((counts, exact))) if p.chapter < counts.len() => {
                        let total: usize = counts.iter().sum::<usize>().max(1);
                        let page = (counts[..p.chapter].iter().sum::<usize>()
                            + p.page.min(counts[p.chapter].saturating_sub(1))
                            + 1)
                        .min(total);
                        ((page * 100 / total) as u8, exact.then_some((page, total)))
                    }
                    (Some(p), _) => (
                        ((p.chapter as f32 / self.library.books[i].chapters.max(1) as f32) * 100.0)
                            .round() as u8,
                        None,
                    ),
                    (None, _) => (0, None),
                },
            };
            // Started: a position beyond the very first page. The rounded percentage
            // is not enough (page 3 of 515: 0 %).
            let started = match self.open.as_ref().filter(|o| o.path == path) {
                Some(o) => o.chapter > 0 || o.page > 0,
                None => pos.map(|p| p.chapter > 0 || p.page > 0).unwrap_or(false),
            };
            let b = &mut self.library.books[i];
            b.finished = finished;
            b.started = started || finished;
            b.percent = if finished {
                100
            } else if started {
                percent.max(1)
            } else {
                percent
            };
            b.pages = if finished { None } else { pages };
        }
    }

    /// Page count of each chapter of a book fully counted for this
    /// configuration (`.pages` file on the partition), and whether the count is exact:
    /// made by another composer version, it remains a good estimate.
    fn saved_counts(&self, path: &Path, cfg: &PageConfig) -> Option<(Vec<usize>, bool)> {
        let txt = std::fs::read_to_string(
            self.cache_dir
                .join(format!("{}.pages", crate::library::cache_key(path))),
        )
        .ok()?;
        let mut lines = txt.lines();
        let head = lines.next()?;
        let exact = head == pages_fingerprint(cfg);
        if !exact && !head.starts_with(&format!("{cfg:?} ")) {
            return None;
        }
        let counts: Option<Vec<usize>> = lines.map(|l| l.trim().parse().ok()).collect();
        counts.map(|c| (c, exact))
    }

    /// The cable has just been plugged in: we export the partition.
    /// Before a restart: if a book is on screen, the new instance will return
    /// to it (`/tmp` is cleared at boot: a real reboot starts at the home screen).
    fn mark_resume(&self) {
        if self.open.is_some()
            && matches!(
                self.view,
                View::Reader
                    | View::Settings
                    | View::Toc
                    | View::Annotations
                    | View::Definition
                    | View::Note
                    | View::GoToPage
                    | View::Search
            )
        {
            let _ = std::fs::write("/tmp/signet-reprise", "");
        }
    }

    /// Cable plugged in: computer or charger? The storage gadget is loaded
    /// without backing media; a computer enumerates it within a second at most, a charger
    /// never does. Reading continues in the meantime.
    fn usb_probe_start(&mut self) {
        let (fw, sn) = crate::device::kobo_version();
        match crate::device::usb_gadget_load(&fw, &sn) {
            Ok(steps) => {
                for l in steps {
                    self.log.push(format!("usb : {l}"));
                }
                self.usb_probe = Some(Instant::now() + Self::USB_PROBE);
            }
            Err((steps, e)) => {
                for l in steps {
                    self.log.push(format!("usb : {l}"));
                }
                self.log
                    .push(format!("usb : gadget impossible à charger : {e}"));
                for l in crate::device::usb_gadget_unload() {
                    self.log.push(format!("usb : {l}"));
                }
            }
        }
    }

    /// A computer has talked to the USB controller: the bus reset
    /// ("default") is its first move, then address and configuration; a
    /// charger produces none of these (and the bus suspend, "suspended",
    /// presupposes a reset before it).
    fn host_seen(state: &str) -> bool {
        matches!(state, "default" | "addressed" | "configured" | "suspended")
    }

    /// Probe in progress: a computer → export; deadline passed → charger.
    fn usb_probe_poll(&mut self) {
        let Some(deadline) = self.usb_probe else {
            return;
        };
        let state = crate::device::udc_state();
        if Self::host_seen(&state) {
            self.usb_probe = None;
            self.log.push(format!(
                "usb : ordinateur détecté (état « {state} »), export de la bibliothèque"
            ));
            self.usb_connect();
        } else if Instant::now() >= deadline {
            self.usb_probe = None;
            self.log.push(format!(
                "usb : aucun ordinateur (état « {state} ») : chargeur, la lecture continue"
            ));
            for l in crate::device::usb_gadget_unload() {
                self.log.push(format!("usb : {l}"));
            }
        }
    }

    /// Asleep: plugging in wakes the reader only if it is a
    /// computer (blocking probe, the screen is the sleep one).
    fn usb_probe_blocking(&mut self) -> bool {
        self.usb_probe_start();
        while self.usb_probe.is_some() {
            if Self::host_seen(&crate::device::udc_state()) {
                // The gadget stays loaded: the main loop will do the export.
                self.usb_probe = Some(Instant::now());
                return true;
            }
            if self.usb_probe.map(|d| Instant::now() >= d).unwrap_or(true) {
                self.usb_probe_poll();
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        false
    }

    fn usb_connect(&mut self) {
        self.save_position();
        self.save_shape_cache();
        self.log
            .push("export USB : journal recopié sur la partition avant démontage".into());
        let _ = crate::device::flush_log();
        // Release the open book: its files live on the exported partition.
        self.open = None;
        ui::usb_screen(&mut self.canvas, &mut self.ts);
        self.screen.snapshot(&self.canvas, "06-usb");
        let r = self.full_rect();
        self.present(r, Waveform::Gc16, true);
        match crate::device::usb_insert_onboard() {
            Ok(steps) => {
                for l in steps {
                    self.log.push(format!("usb : {l}"));
                }
                self.usb_exported = true;
                self.view = View::Usb;
            }
            Err((steps, e)) => {
                for l in steps {
                    self.log.push(format!("usb : {l}"));
                }
                self.log.push(format!("usb : export impossible : {e}"));
                // Remount only if we had unmounted; otherwise, unload the gadget.
                let steps = if !crate::device::onboard_mounted() {
                    crate::device::usb_unexport()
                } else {
                    crate::device::usb_gadget_unload()
                };
                for l in steps {
                    self.log.push(format!("usb : {l}"));
                }
                // We will retry on the next iteration as long as the cable is plugged in.
                self.usb_plugged = false;
                self.render_home();
            }
        }
    }

    /// The cable is unplugged: we take the partition back and reload everything.
    fn usb_disconnect(&mut self) {
        for l in crate::device::usb_unexport() {
            self.log.push(format!("usb : {l}"));
        }
        self.usb_exported = false;
        let _ = crate::device::flush_log();
        self.reload();
        self.render_home();
    }

    /// Reloads state and library from the partition (after a USB export).
    fn reload(&mut self) {
        let path = self.state_path.clone();
        self.state = State::load(&path);
        self.library = Library::scan(&self.library_root, &self.cache_dir, 230, 345);
        self.sort_recent();
        self.refresh_progress();
        self.log.push(format!(
            "bibliothèque rechargée : {} livres",
            self.library.books.len()
        ));
    }

    /// Sleep: sleep screen, suspend, then redraw on wake.
    ///
    /// Kobos often wake for no reason (a few seconds after
    /// falling asleep, or on an internal event). A wake-up is only
    /// "real" if followed by a button press, a cover opening
    /// or a USB plug-in; otherwise we go back to sleep.
    fn sleep(&mut self) {
        self.save_position();
        let _ = crate::device::flush_log();
        let current = self.current_entry().cloned();
        ui::sleep_screen(
            &mut self.canvas,
            &mut self.ts,
            current.as_ref(),
            "En veille",
            crate::icons::Icon::Moon,
        );
        self.screen.snapshot(&self.canvas, "05-veille");
        let r = self.full_rect();
        self.present(r, Waveform::Gc16, true);
        let light = self.state.settings.light;
        let _ = crate::device::set_frontlight(0);
        for l in crate::device::wifi_off() {
            self.log.push(format!("wifi : {l}"));
        }
        // Automatic power-off: an RTC alarm will wake us to power off.
        // A .signet/poweroff-test file (seconds) shortens the delay for tests.
        let poweroff_after: u64 =
            std::fs::read_to_string(Path::new(&self.state_path).with_file_name("poweroff-test"))
                .ok()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(self.state.settings.poweroff_hours as u64 * 3600);
        let mut poweroff_at: Option<i64> = None;
        if poweroff_after > 0 {
            match crate::device::set_rtc_alarm(poweroff_after as u32) {
                Ok(at) => {
                    poweroff_at = Some(at);
                    self.log.push(format!(
                        "extinction automatique programmée dans {poweroff_after} s (epoch {at})"
                    ));
                }
                Err(e) => self.log.push(format!("alarme RTC impossible : {e}")),
            }
        }
        let mut cycles = 0u32;
        let wake_reason = loop {
            cycles += 1;
            let rep = crate::device::suspend();
            for l in rep.steps {
                self.log.push(format!("veille #{cycles} : {l}"));
            }
            if cycles <= 2 {
                // Diagnostics of the first cycles: entering sleep and the wake reason.
                for l in crate::device::dmesg_tail(45) {
                    if l.contains("PM:")
                        || l.contains("wake")
                        || l.contains("Wake")
                        || l.contains("alarm")
                        || l.contains("rtc")
                        || l.contains("ntx")
                        || l.contains("suspend")
                    {
                        self.log.push(format!("dmesg: {l}"));
                    }
                }
            }
            if rep.slept < Duration::from_millis(150) {
                // Did not sleep at all: the kernel refused.
                break "refus du noyau";
            }
            // Woken up: who woke us? We listen for a few seconds.
            let deadline = Instant::now() + Duration::from_secs(4);
            let mut reason: Option<&str> = None;
            while Instant::now() < deadline {
                let plugged = crate::device::usb_plugged();
                if plugged != self.usb_plugged {
                    self.usb_plugged = plugged;
                    if !plugged {
                        self.log
                            .push("câble USB débranché pendant la veille".into());
                    } else if self.usb_probe_blocking() {
                        reason = Some("ordinateur");
                        break;
                    } else {
                        // A charger does not wake the reader (neither the screen nor the light).
                        self.log
                            .push("chargeur branché pendant la veille : on reste endormi".into());
                    }
                }
                // The indicator follows the cable during sleep too.
                let powered = crate::device::power_online();
                if self.led_on != Some(powered) {
                    crate::device::set_power_led(powered);
                    self.led_on = Some(powered);
                }
                let Some(ev) = self.input.next(Duration::from_millis(500)) else {
                    continue;
                };
                match ev {
                    Event::Key {
                        code: signet_input::KEY_POWER,
                        ..
                    } => {
                        reason = Some("bouton");
                        break;
                    }
                    Event::Cover(false) => {
                        reason = Some("capot ouvert");
                        break;
                    }
                    Event::Touch { .. } => {} // touch alone does not wake (pocket, bag)
                    _ => {}
                }
            }
            match reason {
                Some(r) => break r,
                None => {
                    // Is it the power-off alarm?
                    if let (Some(at), Some(now)) = (poweroff_at, crate::device::rtc_now()) {
                        if now >= at - 5 {
                            self.log.push(format!(
                                "réveil par l'alarme d'extinction (epoch {now}) : extinction"
                            ));
                            self.save_position();
                            self.save_shape_cache();
                            let _ = crate::device::flush_log();
                            ui::sleep_screen(
                                &mut self.canvas,
                                &mut self.ts,
                                current.as_ref(),
                                "Éteinte : appuyez sur le bouton pour rallumer",
                                crate::icons::Icon::Power,
                            );
                            let r = self.full_rect();
                            self.present(r, Waveform::Gc16, true);
                            crate::device::clear_rtc_alarm();
                            crate::device::poweroff();
                            std::thread::sleep(Duration::from_secs(30));
                            break "extinction échouée";
                        }
                    }
                    self.log.push(format!(
                        "veille #{cycles} : réveil parasite après {:.1} s, on se rendort",
                        rep.slept.as_secs_f64()
                    ));
                    if cycles >= 200 {
                        break "trop de cycles";
                    }
                }
            }
        };
        crate::device::clear_rtc_alarm();
        self.log
            .push(format!("réveil : {wake_reason} (après {cycles} cycle(s))"));
        self.power_down = None;
        self.last_activity = Instant::now();
        let _ = light;
        self.applied_light = None;
        self.apply_light("réveil");
        if crate::device::dev_mode() {
            // Bring Wi-Fi and sshd back up (the driver was unloaded so that the device could sleep).
            let _ = std::fs::remove_file("/tmp/signet-ip");
            let _ = std::process::Command::new("/bin/sh")
                .arg("/mnt/onboard/.signet/dev.sh")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            self.log
                .push("mode dev : dev.sh relancé pour le Wi-Fi".into());
        }
        // On wake: redraw the current view.
        match self.view {
            View::Home => self.render_home(),
            View::Reader => self.render_reader(Waveform::Gc16),
            View::Settings => {
                self.render_reader(Waveform::Gc16);
                self.render_settings();
            }
            View::Toc => self.render_toc(),
            View::Library | View::BookMenu => {
                self.book_menu = None;
                self.render_library();
            }
            View::Annotations => self.render_annotations(),
            View::Definition | View::Note | View::GoToPage => self.render_reader(Waveform::Gc16),
            View::Search => self.render_search(Refresh::Full),
            View::Usb => {}
        }
    }

    pub fn render_settings(&mut self) {
        let s = &self.state.settings;
        let bookmarked = self
            .open
            .as_ref()
            .map(|o| !Self::page_bookmarks(o).is_empty())
            .unwrap_or(false);
        let sv = SettingsView {
            size: s.size,
            margins: s.margins,
            leading: s.leading,
            light: s.light,
            dev_mode: crate::device::dev_mode(),
            dev_ip: std::fs::read_to_string("/tmp/signet-ip")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            auto_light: s.auto_light,
            fast_pages: s.fast_pages,
            ink: s.ink,
            bookmarked,
            dev_confirm: self
                .dev_confirm
                .map(|t| t.elapsed() < Duration::from_secs(5))
                .unwrap_or(false),
            night: s.night,
            font: s.font.min(FAMILIES.len() - 1),
        };
        let (zones, rect) = ui::settings_overlay(&mut self.canvas, &mut self.ts, &sv);
        // The page zones stay inactive under the overlay: only the panel's zones count.
        self.zones = zones;
        self.view = View::Settings;
        self.screen.snapshot(&self.canvas, "03-reglages");
        self.present_soft(rect);
    }

    pub fn render_library(&mut self) {
        self.render_library_with(false);
    }

    /// `soft`: same screen (other sort, menu closed), without flash.
    fn render_library_with(&mut self, soft: bool) {
        let (clock, battery, charging) = self.status("");
        // The bar's title leads back to the home screen: it says so.
        self.shown_date = Some(self.today());
        let status = Status {
            title: "‹  Accueil",
            clock: &clock,
            battery,
            charging,
        };
        let page = self.library_page;
        let order = self.library_order();
        let books: Vec<(usize, &BookEntry)> =
            order.iter().map(|&i| (i, &self.library.books[i])).collect();
        let zones = ui::library(
            &mut self.canvas,
            &mut self.ts,
            &status,
            &books,
            page,
            self.state.settings.library_sort,
        );
        self.zones = zones;
        self.view = View::Library;
        self.screen.snapshot(&self.canvas, "07-bibliotheque");
        let r = self.full_rect();
        if soft {
            self.present_soft(r);
        } else {
            self.present(r, Waveform::Gc16, true);
            self.partial_since_full = 0;
        }
    }

    pub fn render_annotations(&mut self) {
        self.render_annotations_with(false);
    }

    fn render_annotations_with(&mut self, soft: bool) {
        let (clock, battery, charging) = self.status("");
        let Some(o) = self.open.as_ref() else { return };
        // The bar's title leads back to the page: it says so.
        let title = format!("‹  {}", o.book.metadata.title);
        let items: Vec<(String, String, bool)> = o
            .annotations
            .items
            .iter()
            .map(|a| {
                (
                    self.section_title(a.chapter, (a.block, a.from)),
                    a.text.clone(),
                    a.is_bookmark(),
                )
            })
            .collect();
        let status = Status {
            title: &title,
            clock: &clock,
            battery,
            charging,
        };
        let page = self.annotations_page;
        let zones = ui::annotations(&mut self.canvas, &mut self.ts, &status, &items, page);
        self.zones = zones;
        self.view = View::Annotations;
        self.screen.snapshot(&self.canvas, "08-annotations");
        let r = self.full_rect();
        if soft {
            self.present_soft(r);
        } else {
            self.present(r, Waveform::Gc16, true);
            self.partial_since_full = 0;
        }
    }

    /// Opens (once) the dictionary for the book's language, looked up in
    /// `.signet/dict/` then `.kobo/dict/`; "fr" by default.
    fn dictionary(&mut self) -> Option<&mut signet_dict::Dictionary> {
        let lang = self
            .open
            .as_ref()
            .map(|o| {
                o.book
                    .metadata
                    .language
                    .chars()
                    .take(2)
                    .collect::<String>()
                    .to_lowercase()
            })
            .filter(|l| l.len() == 2)
            .unwrap_or_else(|| "fr".into());
        if self.dict.as_ref().map(|(l, _)| l != &lang).unwrap_or(true) {
            let base = Path::new(&self.state_path)
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let root = base.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let candidates = [
                base.join("dict").join(format!("dicthtml-{lang}.zip")),
                root.join(".kobo")
                    .join("dict")
                    .join(format!("dicthtml-{lang}.zip")),
                base.join("dict").join("dicthtml-fr.zip"),
            ];
            let found = candidates.iter().find(|p| p.exists()).and_then(|p| {
                let t0 = Instant::now();
                let d = signet_dict::Dictionary::open(p).ok();
                self.log.push(format!(
                    "dictionnaire {} : {} ({:.0} ms)",
                    p.display(),
                    if d.is_some() { "ouvert" } else { "illisible" },
                    t0.elapsed().as_secs_f64() * 1000.0
                ));
                d
            });
            if found.is_none() {
                self.log.push(format!("dictionnaire {lang} introuvable"));
            }
            self.dict = Some((lang, found));
        }
        self.dict.as_mut().and_then(|(_, d)| d.as_mut())
    }

    /// Displays the definition of the selected word.
    fn render_definition(&mut self) {
        let Some(sel) = self.selection else { return };
        let word: String = self
            .open
            .as_ref()
            .and_then(|o| o.block_texts.get(sel.block))
            .map(|t| {
                t.chars()
                    .skip(sel.from as usize)
                    .take((sel.to - sel.from) as usize)
                    .collect()
            })
            .unwrap_or_default();
        let word = signet_dict::normalize(&word);
        let state = self.look_up(&word, Vec::new());
        self.dict_state = Some(state);
        self.render_dict();
    }

    /// Looks up `word` in the dictionary for the book's language; nothing found:
    /// nearby words. `back`: the previous words (followed references).
    fn look_up(&mut self, word: &str, back: Vec<String>) -> DictState {
        let t0 = Instant::now();
        // Several words selected: the phrase if the dictionary has it
        // ("à peu près"), otherwise the first word.
        let first = word
            .split_whitespace()
            .next()
            .map(signet_dict::normalize)
            .filter(|f| f != word);
        let (word, entries, suggestions, missing) = match self.dictionary() {
            Some(d) => {
                let mut w = word.to_string();
                let mut e = d.lookup_rich(word, 4);
                if e.is_empty() {
                    if let Some(f) = first {
                        e = d.lookup_rich(&f, 4);
                        w = f;
                    }
                }
                let s = if e.is_empty() {
                    d.suggestions(&w, 6)
                } else {
                    Vec::new()
                };
                (w, e, s, false)
            }
            None => (word.to_string(), Vec::new(), Vec::new(), true),
        };
        let word = word.as_str();
        self.log.push(format!(
            "définition « {word} » : {} entrée(s), {} mot(s) proche(s), en {:.0} ms",
            entries.len(),
            suggestions.len(),
            t0.elapsed().as_secs_f64() * 1000.0
        ));
        DictState {
            word: word.to_string(),
            entries,
            suggestions,
            missing,
            page: 0,
            links: Vec::new(),
            back,
        }
    }

    /// The page (with the selected word), then the definition window over it.
    fn render_dict(&mut self) {
        self.draw_reader();
        let Some(st) = self.dict_state.as_mut() else {
            return;
        };
        let v = definition::View {
            word: &st.word,
            entries: &st.entries,
            suggestions: &st.suggestions,
            missing: st.missing,
            page: st.page,
            back: st.back.last().map(|s| s.as_str()),
        };
        let drawn = definition::draw(&mut self.canvas, &mut self.ts, &v);
        st.page = st.page.min(drawn.pages.saturating_sub(1));
        st.links = drawn.links;
        self.zones = drawn.zones;
        self.view = View::Definition;
        self.screen.snapshot(&self.canvas, "09-definition");
        self.present_soft(drawn.rect);
    }

    /// Tap on a link in the text. A note call opens the note over the
    /// page: the target element starts a block that is not a heading (a note's
    /// back-reference to its call targets the middle of a paragraph, and goes there). The
    /// other links (page references, the book's table of contents) lead to their target, and
    /// a button at the bottom of the page leads back to the passage that was left.
    fn follow_link(&mut self, i: usize) {
        let Some(o) = self.open.as_mut() else { return };
        let Some(link) = o.links.get(i).cloned() else {
            return;
        };
        let Some(target) = o.book.spine_index(&link.href) else {
            self.log
                .push(format!("lien vers {} : hors du livre", link.href));
            return;
        };
        if link.note {
            let t0 = Instant::now();
            let chapter = o.book.chapter_full(target).unwrap_or_default();
            let anchor = link
                .fragment
                .as_ref()
                .and_then(|f| chapter.anchors.get(f))
                .copied();
            if let Some(a) = anchor.filter(|a| {
                a.ch == 0
                    && a.block < chapter.blocks.len()
                    && !matches!(chapter.blocks[a.block], signet_epub::Block::Heading { .. })
            }) {
                let number = note_number(&link.label);
                let mut end = a
                    .end
                    .max(a.block + 1)
                    .min(chapter.blocks.len())
                    .min(a.block + 6);
                let mut blocks: Vec<signet_epub::Block> = chapter.blocks[a.block..end].to_vec();
                if let Some(first) = blocks.first_mut() {
                    *first = crate::reader::strip_note_number(first, &number);
                }
                // Note reduced to its number ("<p>1</p><p>Le texte</p>"): the next block.
                if blocks
                    .iter()
                    .map(|b| b.text().trim().chars().count())
                    .sum::<usize>()
                    < 30
                    && end < chapter.blocks.len()
                {
                    blocks.push(chapter.blocks[end].clone());
                    end += 1;
                }
                self.log.push(format!(
                    "note {number} : chapitre {target}, blocs {}..{end} ({:.0} ms)",
                    a.block,
                    t0.elapsed().as_secs_f64() * 1000.0
                ));
                self.note = Some(NoteView {
                    title: if number.is_empty() {
                        "Note".into()
                    } else {
                        format!("Note {number}")
                    },
                    blocks,
                    chapter: target,
                    fragment: link.fragment.clone(),
                });
                self.render_note();
                return;
            }
        }
        let w = self.page_wfm();
        self.jump_to(target, link.fragment, true, w);
    }

    /// Remembers the displayed passage for the "Revenir à la page…" button.
    fn remember_back(o: &mut Open) {
        if let Some(a) = o
            .anchor_hold
            .or_else(|| o.pages.get(o.page).and_then(page_anchor))
        {
            let (page, _, _) = Self::page_numbers(o);
            o.link_back = Some((o.chapter, a, format!("Revenir à la page {page}")));
        }
    }

    /// Goes to chapter `target`, to element `fragment` if given. `back`:
    /// remember the passage that was left for the "Revenir à la page…" button.
    fn jump_to(&mut self, target: usize, fragment: Option<String>, back: bool, wfm: Waveform) {
        let Some(o) = self.open.as_mut() else { return };
        if back {
            Self::remember_back(o);
        } else {
            o.link_back = None;
        }
        o.anchor_hold = None;
        if target == o.chapter && !o.pages.is_empty() {
            let anchor = fragment
                .as_ref()
                .and_then(|f| o.anchors.get(f))
                .map(|a| (a.block, a.ch));
            o.page = anchor.map(|a| page_for_anchor(&o.pages, a)).unwrap_or(0);
            o.anchor_hold = anchor;
        } else {
            o.chapter = target;
            o.pages.clear();
            o.page = 0;
            o.pending_fragment = fragment;
        }
        self.render_reader(wfm);
        self.note_position();
    }

    /// Title of the section that contains the passage: the last table-of-contents
    /// entry that starts before it. Anchors are only known for the
    /// open chapter (and for all after a search): elsewhere, only the
    /// first entry of a chapter counts. Otherwise, "Chapitre N".
    fn section_title(&self, chapter: usize, at: (usize, u32)) -> String {
        let Some(o) = self.open.as_ref() else {
            return String::new();
        };
        let anchors_of = |c: usize| {
            if c == o.chapter && o.cache_cfg.is_some() {
                Some(&o.anchors)
            } else {
                o.all_anchors.as_ref().and_then(|v| v.get(c))
            }
        };
        let mut best: Option<(usize, (usize, u32), &str)> = None;
        let mut seen = std::collections::HashSet::new();
        for e in &o.book.toc {
            let Some(c) = o.book.spine_index(&e.href) else {
                continue;
            };
            let first = seen.insert(c);
            let start = match (&e.fragment, anchors_of(c)) {
                (None, _) => Some((0, 0)),
                (Some(f), Some(m)) => m.get(f).map(|a| (a.block, a.ch)),
                (Some(_), None) => first.then_some((0, 0)),
            };
            if let Some(s) = start {
                if (c, s) <= (chapter, at)
                    && best.map(|(bc, bs, _)| (bc, bs) <= (c, s)).unwrap_or(true)
                {
                    best = Some((c, s, e.label.as_str()));
                }
            }
        }
        best.map(|b| b.2.to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| format!("Chapitre {}", chapter + 1))
    }

    /// Places or removes the bookmark of the displayed page. From the page, only
    /// the ribbon corner is refreshed; from the panel, it is redrawn.
    fn toggle_bookmark(&mut self) {
        let Some(o) = self.open.as_mut() else { return };
        let Some(start) = o.pages.get(o.page).and_then(page_anchor) else {
            return;
        };
        let here = Self::page_bookmarks(o);
        if here.is_empty() {
            // Excerpt: the first words of the page.
            let mut text = String::new();
            for (b, t) in o.block_texts.iter().enumerate().skip(start.0) {
                let s: String = if b == start.0 {
                    t.chars().skip(start.1 as usize).collect()
                } else {
                    t.clone()
                };
                let s = s.replace('\t', " ");
                if s.trim().is_empty() {
                    continue;
                }
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(s.trim());
                if text.chars().count() > 140 {
                    break;
                }
            }
            let mut excerpt: String = text.chars().take(140).collect();
            if text.chars().count() > 140 {
                excerpt.push('…');
            }
            o.annotations.add(Annotation {
                chapter: o.chapter,
                block: start.0,
                from: start.1,
                to: start.1,
                text: excerpt,
            });
            self.log.push("marque-page posé".into());
        } else {
            o.annotations.remove_all(here);
            self.log.push("marque-page retiré".into());
        }
        let from_panel = self.view == View::Settings;
        self.draw_reader();
        if from_panel {
            self.render_settings();
        } else {
            let w = self.canvas.width;
            self.present(
                Rect::new(w - 48, (STATUS_H - 16) as u32, 44, 64),
                Waveform::Gl16,
                false,
            );
        }
    }

    /// Goes to page `n` of the book (footer numbering: exact when
    /// the book is counted, estimated otherwise), remembering the page that was left.
    fn go_to_page(&mut self, n: usize) {
        let Some(o) = self.open.as_mut() else { return };
        let (pages, _) = Self::chapter_pages(o);
        let total: usize = pages.iter().sum();
        let n = n.clamp(1, total.max(1));
        let (mut acc, mut target) = (0, (pages.len().saturating_sub(1), 0));
        for (i, p) in pages.iter().enumerate() {
            if n <= acc + p {
                target = (i, n - acc - 1);
                break;
            }
            acc += p;
        }
        Self::remember_back(o);
        o.anchor_hold = None;
        o.pending_anchor = None;
        o.pending_fragment = None;
        if target.0 == o.chapter && !o.pages.is_empty() {
            o.page = target.1.min(o.pages.len() - 1);
        } else {
            o.chapter = target.0;
            o.pages.clear();
            o.page = target.1;
        }
        self.log.push(format!(
            "aller à la page {n} : chapitre {}, page {}",
            target.0, target.1
        ));
        let w = self.page_wfm();
        self.render_reader(w);
        self.note_position();
    }

    fn render_goto(&mut self) {
        let Some(o) = self.open.as_ref() else { return };
        let (current, total, _) = Self::page_numbers(o);
        let (zones, rect, _) = ui::goto_page(
            &mut self.canvas,
            &mut self.ts,
            &self.goto_input,
            current,
            total,
        );
        self.zones = zones;
        self.view = View::GoToPage;
        self.screen.snapshot(&self.canvas, "11-aller");
        self.present_soft(rect);
    }

    /// A digit typed: only the field is redrawn and refreshed.
    fn refresh_goto_field(&mut self) {
        let Some(o) = self.open.as_ref() else { return };
        let (current, total, _) = Self::page_numbers(o);
        let r = ui::goto_field(
            &mut self.canvas,
            &mut self.ts,
            &self.goto_input,
            current,
            total,
        );
        self.screen.snapshot(&self.canvas, "11-aller");
        self.present(r, Waveform::Gl16, false);
    }

    /// Search screen. `refresh`: the whole screen with a flash (opening),
    /// without flash (keyboard ⇄ results, results page), or only the field (one key).
    fn render_search(&mut self, refresh: Refresh) {
        let (clock, battery, charging) = self.status("");
        let Some(o) = self.open.as_ref() else { return };
        let title = format!("‹  {}", o.book.metadata.title);
        let status = Status {
            title: &title,
            clock: &clock,
            battery,
            charging,
        };
        let per_page = ui::search_per_page(self.canvas.height as i32);
        let s = &self.search;
        let pages = s.results.len().div_ceil(per_page);
        let first = s.page * per_page;
        let shown: Vec<(String, crate::search::Found)> = s
            .results
            .iter()
            .skip(first)
            .take(per_page)
            .map(|f| (self.section_title(f.chapter, (f.block, f.from)), f.clone()))
            .collect();
        let total = s
            .done
            .then_some((s.results.len(), s.results.len() >= SEARCH_MAX));
        let v = ui::SearchView {
            query: &s.query,
            keyboard: s.keyboard,
            results: &shown,
            first,
            total,
            page: s.page,
            pages,
            busy: s.busy,
        };
        let (zones, field) = ui::search(&mut self.canvas, &mut self.ts, &status, &v);
        self.zones = zones;
        self.view = View::Search;
        self.screen.snapshot(&self.canvas, "12-recherche");
        match refresh {
            Refresh::Full => {
                let r = self.full_rect();
                self.present(r, Waveform::Gc16, true);
                self.partial_since_full = 0;
            }
            Refresh::Soft => {
                let r = self.full_rect();
                self.present_soft(r);
            }
            Refresh::Field => self.present(field, Waveform::Gl16, false),
        }
    }

    /// Searches for the input in the whole book. The first time, the chapters are
    /// read (about a second on the reader, "Recherche…" is displayed); their
    /// texts stay in memory for subsequent searches.
    fn run_search(&mut self) {
        let q = self.search.query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.search.keyboard = false;
        self.search.busy = true;
        self.render_search(Refresh::Soft);
        self.search.busy = false;
        let Some(o) = self.open.as_mut() else { return };
        if o.search_texts.is_none() {
            let t0 = Instant::now();
            let (mut texts, mut anchors) = (Vec::new(), Vec::new());
            for i in 0..o.book.spine.len() {
                let ch = o.book.chapter_full(i).unwrap_or_default();
                texts.push(ch.blocks.iter().map(|b| b.text()).collect::<Vec<String>>());
                anchors.push(ch.anchors);
            }
            o.search_texts = Some(texts);
            o.all_anchors = Some(anchors);
            self.log.push(format!(
                "textes du livre lus pour la recherche en {:.0} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            ));
        }
        let t0 = Instant::now();
        let results = crate::search::find(
            o.search_texts.as_deref().unwrap_or_default(),
            &q,
            SEARCH_MAX,
        );
        self.log.push(format!(
            "recherche « {q} » : {} résultat(s) en {:.0} ms",
            results.len(),
            t0.elapsed().as_secs_f64() * 1000.0
        ));
        self.search.results = results;
        self.search.done = true;
        self.search.page = 0;
        self.render_search(Refresh::Soft);
    }

    /// Opens the found passage, marked on its page; "Revenir" leads back to reading.
    fn open_search_result(&mut self, i: usize) {
        let Some(f) = self.search.results.get(i).cloned() else {
            return;
        };
        let Some(o) = self.open.as_mut() else { return };
        Self::remember_back(o);
        o.search_mark = Some((f.chapter, f.block, f.from, f.to));
        o.pending_fragment = None;
        if f.chapter == o.chapter && !o.pages.is_empty() {
            o.page = page_for_anchor(&o.pages, (f.block, f.from));
        } else {
            o.chapter = f.chapter;
            o.pages.clear();
            o.page = 0;
            o.pending_anchor = Some((f.block, f.from));
        }
        o.anchor_hold = Some((f.block, f.from));
        self.render_reader(Waveform::Gc16);
        self.note_position();
    }

    fn render_note(&mut self) {
        let Some(n) = self.note.as_ref() else { return };
        let cfg = self.page_config();
        let (zones, rect) = ui::note(
            &mut self.canvas,
            &mut self.ts,
            &n.title,
            &n.blocks,
            cfg.px.min(30.0),
            cfg.family,
        );
        self.zones = zones;
        self.view = View::Note;
        self.screen.snapshot(&self.canvas, "10-note");
        self.present_soft(rect);
    }

    /// The link under the finger, on the displayed page (not during a selection).
    fn link_at(&self, x: i32, y: i32) -> Option<usize> {
        let o = self.open.as_ref()?;
        if self.view != View::Reader || self.selection.is_some() || o.links.is_empty() {
            return None;
        }
        ui::link_at(o.pages.get(o.page)?, &self.page_config(), &o.links, x, y)
    }

    /// Range [from, to) of the word containing character `ch` of the block.
    fn word_range(text: &str, ch: u32) -> (u32, u32) {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len() as u32;
        if n == 0 {
            return (0, 0);
        }
        let i = ch.min(n - 1);
        let is_sep = |c: char| c.is_whitespace() && c != '\u{a0}';
        let mut from = i;
        while from > 0 && !is_sep(chars[(from - 1) as usize]) {
            from -= 1;
        }
        let mut to = i;
        while to < n && !is_sep(chars[to as usize]) {
            to += 1;
        }
        (from, to)
    }

    /// Long press on a word: new selection, or existing highlight.
    fn start_selection(&mut self, x: i32, y: i32) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_ref() else { return };
        let page = &o.pages[o.page];
        let Some((block, ch)) = ui::glyph_at(page, &cfg, x, y) else {
            self.log.push("appui long hors texte".into());
            return;
        };
        if let Some((i, a)) =
            o.annotations.items.iter().enumerate().find(|(_, a)| {
                a.chapter == o.chapter && a.block == block && ch >= a.from && ch < a.to
            })
        {
            self.selection = Some(Selection {
                block,
                from: a.from,
                to: a.to,
                existing: Some(i),
            });
        } else {
            let (from, to) = Self::word_range(&o.block_texts[block], ch);
            self.selection = Some(Selection {
                block,
                from,
                to,
                existing: None,
            });
        }
        self.log.push(format!("sélection : {:?}", self.selection));
        self.render_reader(Waveform::Glr16);
    }

    const LONG_PRESS: Duration = Duration::from_millis(350);
    /// Tap on the "Lecture" panel title, which toggles dev mode.
    const DEV_PRESS: Duration = Duration::from_millis(2000);

    /// Is the point on the "Lecture" title of the open panel?
    fn on_dev_secret(&self, x: i32, y: i32) -> bool {
        self.view == View::Settings
            && self
                .zones
                .iter()
                .any(|z| z.contains(x, y) && z.hit == Hit::DevSecret)
    }
    /// Time given to a computer to enumerate the reader.
    const USB_PROBE: Duration = Duration::from_secs(6);
    const LIVE_INTERVAL: Duration = Duration::from_millis(150);

    /// Press held: the word under the finger lights up immediately.
    fn begin_drag(&mut self, x: i32, y: i32) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_ref() else { return };
        let page = &o.pages[o.page];
        let Some((block, ch)) = ui::glyph_at(page, &cfg, x, y) else {
            self.log.push("appui long hors texte".into());
            return;
        };
        let existing = o
            .annotations
            .items
            .iter()
            .enumerate()
            .find(|(_, a)| a.chapter == o.chapter && a.block == block && ch >= a.from && ch < a.to)
            .map(|(i, _)| i);
        let (from, to) = match existing {
            Some(i) => (o.annotations.items[i].from, o.annotations.items[i].to),
            None => Self::word_range(&o.block_texts[block], ch),
        };
        self.selection = Some(Selection {
            block,
            from,
            to,
            existing,
        });
        self.drag = Some((from, to));
        self.live_rect = None;
        self.live_base = None;
        self.log
            .push(format!("appui maintenu : {:?}", self.selection));
        self.live_render();
    }

    /// The finger drags: the selection follows the word under the finger (same paragraph).
    fn drag_to(&mut self, x: i32, y: i32) {
        let Some((af, at)) = self.drag else { return };
        let Some(sel) = self.selection else { return };
        if sel.existing.is_some() {
            return;
        }
        let cfg = self.page_config();
        let Some(o) = self.open.as_ref() else { return };
        let page = &o.pages[o.page];
        let Some((block, ch)) = ui::glyph_at(page, &cfg, x, y) else {
            return;
        };
        if block != sel.block {
            return;
        }
        let (wf, wt) = Self::word_range(&o.block_texts[block], ch);
        let (from, to) = (af.min(wf), at.max(wt));
        if from == sel.from && to == sel.to {
            return;
        }
        self.selection = Some(Selection {
            block,
            from,
            to,
            existing: None,
        });
        self.live_dirty = true;
        if self
            .live_at
            .map(|t| t.elapsed() >= Self::LIVE_INTERVAL)
            .unwrap_or(true)
        {
            self.live_render();
        }
    }

    /// "Live" render: the inverted selection (white text on black, hence
    /// purely black and white) and an A2 refresh of only the area
    /// concerned, ~150 ms, without waiting. The page without selection is
    /// composed only once per gesture, then copied.
    fn live_render(&mut self) {
        let cfg = self.page_config();
        let Some(sel) = self.selection else { return };
        // The time of the base render (the selection does not make another).
        let (clock, battery, charging) = if self.live_base.is_none() {
            self.status("")
        } else {
            (String::new(), None, false)
        };
        let Some(o) = self.open.as_ref() else { return };
        let page = o.pages[o.page].clone();
        match &self.live_base {
            Some(base) => self.canvas.data.copy_from_slice(base),
            None => {
                let title = o.book.metadata.title.clone();
                let status = Status {
                    title: &title,
                    clock: &clock,
                    battery,
                    charging,
                };
                let label = Self::book_page_label(o);
                let marks: Vec<ui::Mark> = o
                    .annotations
                    .in_chapter(o.chapter)
                    .into_iter()
                    .map(|(b, f, t, _)| (b, f, t, GRAY_LIGHT))
                    .collect();
                let back = o.link_back.as_ref().map(|(_, _, l)| l.clone());
                let chrome = ui::Chrome {
                    label: &label,
                    back: back.as_deref(),
                    bookmarked: !Self::page_bookmarks(o).is_empty(),
                    night: self.state.settings.night,
                };
                ui::reader(
                    &mut self.canvas,
                    &mut self.ts,
                    &status,
                    &page,
                    &cfg,
                    &chrome,
                    &marks,
                    None,
                );
                self.live_base = Some(self.canvas.data.clone());
            }
        }
        self.live_dirty = false;
        let rects = ui::range_rects(&page, &cfg, sel.block, sel.from, sel.to);
        let mut union: Option<Rect> = self.live_rect;
        for r in &rects {
            self.canvas.invert_rect(*r);
            union = Some(match union {
                None => *r,
                Some(u) => union_rect(u, *r),
            });
        }
        if let Some(u) = union {
            // Vertical margin: one line above and below, to erase the old inversion.
            let pad = (cfg.px * cfg.line_height) as u32;
            let r = Rect::new(0, u.y.saturating_sub(pad), self.canvas.width, u.h + 2 * pad);
            self.screen.snapshot(&self.canvas, "10-selection-direct");
            self.present(r, Waveform::A2, false);
            // We remember the area without its margin, otherwise the band grows at every step.
            self.live_rect = Some(u);
        }
        self.live_at = Some(Instant::now());
    }

    /// Release after a drag: clean render with the action bar.
    fn end_drag(&mut self) {
        self.drag = None;
        self.live_rect = None;
        self.live_base = None;
        self.live_dirty = false;
        self.partial_since_full = FULL_EVERY; // clears the DU inversion
        self.render_reader(Waveform::Gc16);
    }

    /// Simple tap during a selection: extends to the touched word (same block).
    fn extend_selection(&mut self, x: i32, y: i32) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_ref() else { return };
        let Some(sel) = self.selection else { return };
        if sel.existing.is_some() {
            return;
        }
        let page = &o.pages[o.page];
        let hit = ui::glyph_at(page, &cfg, x, y);
        self.log.push(format!(
            "extension : tap ({x},{y}) → {hit:?}, sélection bloc {}",
            sel.block
        ));
        match hit {
            Some((block, ch)) if block == sel.block => {
                let (f, t) = Self::word_range(&o.block_texts[block], ch);
                self.selection = Some(Selection {
                    block,
                    from: sel.from.min(f),
                    to: sel.to.max(t),
                    existing: None,
                });
                self.render_reader(Waveform::Glr16);
            }
            Some(_) => {
                // Another paragraph: ignored (a highlight stays within its block).
                self.log.push("extension ignorée : autre paragraphe".into());
            }
            None => {
                self.selection = None;
                self.log.push("sélection annulée".into());
                self.render_reader(Waveform::Glr16);
            }
        }
    }

    fn page_containing(o: &Open, block: usize, ch: u32) -> Option<usize> {
        o.pages.iter().position(|p| {
            p.items.iter().any(|it| {
                it.block == block && it.lines.iter().any(|l| l.glyphs.iter().any(|g| g.ch >= ch))
            })
        })
    }

    pub fn render_toc(&mut self) {
        self.render_toc_with(false);
    }

    /// Table-of-contents entries (label, depth, action) and the one of the passage read:
    /// the last one that starts before the top of the page. An entry targeting an
    /// element of the open chapter ("ch002a.xhtml#h_3") is located by its anchor;
    /// for the other chapters, the chapter is enough for comparison.
    fn toc_entries(&self) -> (Vec<(String, u8, Hit)>, Option<usize>) {
        let Some(o) = self.open.as_ref() else {
            return (Vec::new(), None);
        };
        let here = (
            o.chapter,
            o.anchor_hold
                .or_else(|| o.pages.get(o.page).and_then(page_anchor))
                .unwrap_or((0, 0)),
        );
        let mut entries = Vec::new();
        let mut current = None;
        for (k, e) in o.book.toc.iter().enumerate() {
            let Some(ch) = o.book.spine_index(&e.href) else {
                continue;
            };
            let at = if ch == o.chapter {
                e.fragment
                    .as_ref()
                    .and_then(|f| o.anchors.get(f))
                    .map(|a| (a.block, a.ch))
                    .unwrap_or((0, 0))
            } else {
                (0, 0)
            };
            if (ch, at) <= here {
                current = Some(entries.len());
            }
            entries.push((e.label.clone(), e.depth, Hit::GoToToc(k)));
        }
        if entries.is_empty() {
            entries = (0..o.book.spine.len())
                .map(|i| (format!("Section {}", i + 1), 0, Hit::GoToChapter(i)))
                .collect();
            current = Some(o.chapter);
        }
        (entries, current)
    }

    /// `soft`: next or previous table-of-contents page (same screen, without flash).
    fn render_toc_with(&mut self, soft: bool) {
        let (clock, battery, charging) = self.status("");
        let Some(o) = self.open.as_ref() else { return };
        // The bar's title leads back to the page: it says so.
        let title = format!("‹  {}", o.book.metadata.title);
        let (entries, current) = self.toc_entries();
        let status = Status {
            title: &title,
            clock: &clock,
            battery,
            charging,
        };
        let page = self.toc_page;
        let zones = ui::toc(
            &mut self.canvas,
            &mut self.ts,
            &status,
            &entries,
            page,
            current,
        );
        self.zones = zones;
        self.view = View::Toc;
        self.screen.snapshot(&self.canvas, "04-sommaire");
        let r = self.full_rect();
        if soft {
            self.present_soft(r);
        } else {
            self.present(r, Waveform::Gc16, true);
            self.partial_since_full = 0;
        }
    }

    // ----------------------------------------------------------- actions ----

    /// The book already open in memory (return to the home screen then "Reprendre"):
    /// its pages are composed, they only need displaying. Otherwise, we open it.
    fn resume_or_open(&mut self, path: &Path) {
        if self.open.as_ref().map(|o| o.path == path).unwrap_or(false) {
            self.selection = None;
            self.log.push("livre déjà ouvert : reprise directe".into());
            self.render_reader(Waveform::Gc16);
        } else {
            self.open_book(path);
        }
    }

    fn open_book(&mut self, path: &Path) {
        let t0 = Instant::now();
        let Ok(book) = Book::open(path) else {
            self.log
                .push(format!("impossible d'ouvrir {}", path.display()));
            return;
        };
        let key = path.to_string_lossy().into_owned();
        let pos = self.state.positions.get(&key).copied().unwrap_or_default();
        let chapter = pos.chapter.min(book.spine.len().saturating_sub(1));
        let annotations = Annotations::load(
            &Path::new(&self.state_path).with_file_name("annotations"),
            &crate::library::cache_key(path),
        );
        let n_ch = book.spine.len();
        let mut open = Open {
            path: path.to_path_buf(),
            book,
            chapter,
            pages: Vec::new(),
            page: pos.page,
            cache_cfg: None,
            chars: Vec::new(),
            block_texts: Vec::new(),
            links: Vec::new(),
            anchors: HashMap::new(),
            next: None,
            annotations,
            images: Default::default(),
            counts: vec![None; n_ch],
            counts_cfg: None,
            counts_saved: true,
            pending_anchor: pos.anchor.filter(|_| pos.chapter == chapter),
            anchor_hold: pos.anchor.filter(|_| pos.chapter == chapter),
            pending_fragment: None,
            link_back: None,
            search_texts: None,
            all_anchors: None,
            search_mark: None,
        };
        // Page counts already established for this configuration and this binary.
        let cfg = self.page_config();
        let pages_file = self
            .cache_dir
            .join(format!("{}.pages", crate::library::cache_key(path)));
        if let Ok(txt) = std::fs::read_to_string(&pages_file) {
            let mut lines = txt.lines();
            if lines.next() == Some(pages_fingerprint(&cfg).as_str()) {
                let counts: Vec<Option<usize>> = lines.map(|l| l.trim().parse().ok()).collect();
                if counts.len() == n_ch {
                    open.counts = counts;
                    open.counts_cfg = Some(cfg);
                    self.log.push(format!(
                        "pagination du livre relue : {} chapitres comptés",
                        open.counts.iter().filter(|c| c.is_some()).count()
                    ));
                }
            }
        }
        // Size of each chapter, to estimate the pages not yet counted:
        // only if some are missing (reading the 60 chapters costs 200 ms on the reader).
        if open.counts.iter().any(|c| c.is_none()) {
            open.ensure_chars();
        }
        self.log.push(format!(
            "livre ouvert : {} chapitres, en {:.0} ms{}",
            n_ch,
            t0.elapsed().as_secs_f64() * 1000.0,
            if open.chars.is_empty() {
                " (pages déjà comptées)"
            } else {
                ""
            }
        ));
        self.open = Some(open);
        self.selection = None;
        // The search of another book is worthless here.
        self.search = SearchState::default();
        // Last opening: the "recents" order of the library and the home screen.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.state.seen.insert(key.clone(), now);
        self.sort_recent();
        self.state.last_book = Some(key);
        self.state.save();
        self.render_reader(Waveform::Gc16);
    }

    /// Persistent shaping cache: the words already composed in previous
    /// sessions save 0.3 ms of shaping each on the Cortex-A7.
    fn load_shape_cache(&mut self) {
        let path = Path::new(&self.state_path).with_file_name("shape-cache.bin");
        if let Ok(data) = std::fs::read(&path) {
            let t0 = Instant::now();
            let n = self.ts.import_shaped(&data);
            self.log.push(format!(
                "cache de façonnage : {n} mots rechargés en {:.0} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            ));
        }
        // The cache has just been reloaded: nothing to rewrite as long as no new word
        // has been shaped (2.3 MB on the partition at every return to the home screen).
        self.shape_saved = self.ts.shaped_len();
    }

    /// Writes the shaping cache to the partition (if it is mounted).
    fn save_shape_cache(&mut self) {
        if !crate::device::onboard_mounted() && self.state_path.starts_with("/mnt/onboard/") {
            return;
        }
        let n = self.ts.shaped_len();
        if n == 0 || n == self.shape_saved {
            return;
        }
        let path = Path::new(&self.state_path).with_file_name("shape-cache.bin");
        let data = self.ts.export_shaped();
        let tmp = path.with_extension("tmp");
        if crate::device::with_rw(|| {
            std::fs::write(&tmp, &data).is_ok() && std::fs::rename(&tmp, &path).is_ok()
        }) {
            self.shape_saved = n;
            self.log.push(format!(
                "cache de façonnage écrit : {n} mots, {} Ko",
                data.len() / 1024
            ));
        }
    }

    /// Current position, written immediately to the partition (sleep, home screen,
    /// USB export, shutdown).
    fn save_position(&mut self) {
        if self.note_position() {
            self.state.save();
        }
        self.position_dirty = false;
    }

    /// Current position kept in memory only: a turned page no longer pays
    /// for the write (remounting the partition read-write, `sync`, back to read-only:
    /// ~50 ms on the reader, before display). It is written after two seconds
    /// without a gesture, or before sleep and shutdown.
    fn note_position(&mut self) -> bool {
        let Some(o) = &self.open else { return false };
        let key = o.path.to_string_lossy().into_owned();
        let anchor = if o.cache_cfg.is_some() {
            o.pages.get(o.page).and_then(page_anchor)
        } else {
            None
        };
        // Passage found again with no page turned since, or pages not yet composed:
        // we keep the anchor already known.
        let anchor = o.anchor_hold.or(anchor).or(o.pending_anchor);
        self.state.positions.insert(
            key,
            Position {
                chapter: o.chapter,
                page: o.page,
                anchor,
            },
        );
        self.position_dirty = true;
        true
    }

    /// Waveform for turning a page: GLR16 (16 grays, anti-ghosting) or
    /// DU (black and white, ~375 ms) when the text is rendered in binary.
    fn page_wfm(&self) -> Waveform {
        if self.state.settings.fast_pages {
            Waveform::Du
        } else {
            Waveform::Glr16
        }
    }

    fn next_page(&mut self) {
        let Some(o) = self.open.as_mut() else { return };
        o.anchor_hold = None;
        o.search_mark = None;
        if o.page + 1 < o.pages.len() {
            o.page += 1;
        } else if o.chapter + 1 < o.book.spine.len() {
            o.chapter += 1;
            o.page = 0;
            o.pages.clear();
        } else {
            let key = o.path.to_string_lossy().into_owned();
            self.state.finished.insert(key, true);
            self.state.save();
            self.go_home();
            return;
        }
        let w = self.page_wfm();
        self.render_reader(w);
        self.note_position();
    }

    fn prev_page(&mut self) {
        let Some(o) = self.open.as_mut() else { return };
        o.anchor_hold = None;
        o.search_mark = None;
        if o.page > 0 {
            o.page -= 1;
        } else if o.chapter > 0 {
            o.chapter -= 1;
            o.page = usize::MAX;
            o.pages.clear();
        } else {
            return;
        }
        let w = self.page_wfm();
        self.render_reader(w);
        self.note_position();
    }

    fn go_home(&mut self) {
        self.save_position();
        self.save_shape_cache();
        // Recompute the displayed progress (in pages, like the page footer).
        self.refresh_progress();
        self.render_home();
    }

    fn handle_hit(&mut self, hit: Hit) {
        self.log.push(format!("action {hit:?}"));
        let t0 = Instant::now();
        self.do_hit(hit);
        self.log.push(format!(
            "action traitée en {:.0} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        ));
    }

    fn do_hit(&mut self, hit: Hit) {
        match hit {
            Hit::Resume => {
                if let Some(p) = self.current_entry().map(|b| b.path.clone()) {
                    self.resume_or_open(&p);
                }
            }
            Hit::OpenBook(i) => {
                if let Some(p) = self.library.books.get(i).map(|b| b.path.clone()) {
                    self.resume_or_open(&p);
                }
            }
            Hit::SeeAll => {
                self.library_page = 0;
                self.render_library();
            }
            Hit::LibraryPage(p) => {
                self.library_page = p;
                self.render_library();
            }
            Hit::PrevPage => self.prev_page(),
            Hit::NextPage => self.next_page(),
            Hit::ToggleSettings => self.render_settings(),
            // Return to the page from a panel or an overlay: like a turned page
            // (flashless, full cleanup in its turn), instead of a flash at every menu.
            Hit::CloseSettings => {
                let w = self.page_wfm();
                self.render_reader(w);
            }
            Hit::SetSize(i) => {
                self.state.settings.size = i;
                self.state.save();
                self.relayout_keep_settings();
            }
            Hit::SetMargins(i) => {
                self.state.settings.margins = i;
                self.state.save();
                self.relayout_keep_settings();
            }
            Hit::SetFont(i) => {
                self.state.settings.font = i.min(FAMILIES.len() - 1);
                self.state.save();
                self.relayout_keep_settings();
            }
            Hit::SetLeading(i) => {
                self.state.settings.leading = i;
                self.state.save();
                self.relayout_keep_settings();
            }
            Hit::SetLight(l) => {
                self.state.settings.light = l;
                self.state.save();
                self.manual_light_at = Some(Instant::now());
                self.applied_light = None;
                self.apply_light("curseur");
                self.render_settings_only();
            }
            Hit::ToggleFastPages => {
                self.state.settings.fast_pages = !self.state.settings.fast_pages;
                self.state.save();
                self.ts.set_binary(self.state.settings.fast_pages);
                self.relayout_keep_settings();
            }
            Hit::SetInk(i) => {
                self.state.settings.ink = i.min(2);
                self.state.save();
                self.apply_ink();
                self.relayout_keep_settings();
            }
            Hit::ToggleAutoLight => {
                self.state.settings.auto_light = !self.state.settings.auto_light;
                self.state.save();
                self.manual_light_at = None;
                self.applied_light = None;
                self.apply_light("auto basculé");
                self.render_settings_only();
            }
            Hit::Toc => {
                // The table of contents opens at the page of the current entry.
                let (_, current) = self.toc_entries();
                self.toc_page = current
                    .map(|c| c / ui::toc_per_page(self.canvas.height as i32))
                    .unwrap_or(0);
                self.render_toc();
            }
            Hit::TocPage(p) => {
                self.toc_page = p;
                self.render_toc_with(true);
            }
            Hit::GoToChapter(i) => {
                if let Some(o) = self.open.as_mut() {
                    o.anchor_hold = None;
                    if o.chapter != i {
                        o.chapter = i;
                        o.page = 0;
                        o.pages.clear();
                    }
                }
                self.save_position();
                self.render_reader(Waveform::Gc16);
            }
            Hit::Back => {
                if self.view == View::Library {
                    self.render_home()
                } else {
                    self.render_reader(Waveform::Gc16)
                }
            }
            Hit::Annotations => {
                self.annotations_page = 0;
                self.render_annotations();
            }
            Hit::AnnotationsPage(p) => {
                self.annotations_page = p;
                self.render_annotations_with(true);
            }
            Hit::GoToAnnotation(i) => {
                let target = self
                    .open
                    .as_ref()
                    .and_then(|o| o.annotations.items.get(i).cloned());
                if let Some(a) = target {
                    if let Some(o) = self.open.as_mut() {
                        if o.chapter != a.chapter {
                            o.chapter = a.chapter;
                            o.pages.clear();
                        }
                    }
                    self.ensure_pages();
                    if let Some(o) = self.open.as_mut() {
                        o.page = Self::page_containing(o, a.block, a.from).unwrap_or(0);
                        o.anchor_hold = Some((a.block, a.from));
                    }
                    self.save_position();
                    self.render_reader(Waveform::Gc16);
                }
            }
            Hit::Highlight => {
                if let (Some(sel), Some(o)) = (self.selection.take(), self.open.as_mut()) {
                    // The tab of a numbered list ("1.\tTexte") becomes a space.
                    let text: String = o
                        .block_texts
                        .get(sel.block)
                        .map(|t| {
                            t.chars()
                                .skip(sel.from as usize)
                                .take((sel.to - sel.from) as usize)
                                .map(|c| if c == '\t' { ' ' } else { c })
                                .collect()
                        })
                        .unwrap_or_default();
                    let excerpt: String = text.chars().take(160).collect();
                    o.annotations.add(Annotation {
                        chapter: o.chapter,
                        block: sel.block,
                        from: sel.from,
                        to: sel.to,
                        text: excerpt,
                    });
                    self.log.push(format!(
                        "surlignage ajouté ({} caractères)",
                        text.chars().count()
                    ));
                }
                self.render_reader(Waveform::Glr16);
            }
            Hit::CancelSelection => {
                self.selection = None;
                self.render_reader(Waveform::Glr16);
            }
            Hit::Define => self.render_definition(),
            Hit::CloseDefinition => {
                self.selection = None;
                self.dict_state = None;
                self.render_reader(Waveform::Glr16);
            }
            Hit::DictLink(i) => {
                if let Some(st) = self.dict_state.take() {
                    match st.links.get(i).cloned() {
                        Some(target) => {
                            let mut back = st.back;
                            back.push(st.word);
                            self.dict_state = Some(self.look_up(&target, back));
                        }
                        None => self.dict_state = Some(st),
                    }
                    self.render_dict();
                }
            }
            Hit::DictBack => {
                if let Some(mut st) = self.dict_state.take() {
                    match st.back.pop() {
                        Some(previous) => self.dict_state = Some(self.look_up(&previous, st.back)),
                        None => self.dict_state = Some(st),
                    }
                    self.render_dict();
                }
            }
            Hit::DictPage(p) => {
                if let Some(st) = self.dict_state.as_mut() {
                    st.page = p;
                }
                self.render_dict();
            }
            Hit::DeleteHighlight(i) => {
                self.selection = None;
                if let Some(o) = self.open.as_mut() {
                    o.annotations.remove(i);
                }
                self.log.push("surlignage supprimé".into());
                self.render_reader(Waveform::Glr16);
            }
            Hit::ToggleDevMode => {
                // "Passer en lecture" cuts Wi-Fi and SSH: a single stray tap
                // was enough to lose access to the reader. It must be confirmed by a
                // second tap within 5 s.
                if self
                    .dev_confirm
                    .map(|t| t.elapsed() < Duration::from_secs(5))
                    .unwrap_or(false)
                {
                    self.dev_confirm = None;
                    self.toggle_dev_mode();
                } else {
                    self.dev_confirm = Some(Instant::now());
                    self.log
                        .push("« Passer en lecture » : appuyer encore pour confirmer".into());
                    self.draw_reader();
                    self.render_settings();
                }
            }
            Hit::LibrarySort(k) => {
                self.state.settings.library_sort = k.min(ui::LIBRARY_SORTS.len() - 1);
                self.state.save();
                self.library_page = 0;
                self.render_library_with(true);
            }
            Hit::BookMenu(i) => {
                self.book_menu = Some((i, false));
                self.render_book_menu();
            }
            Hit::ToggleFinished(i) => {
                if let Some(key) = self
                    .library
                    .books
                    .get(i)
                    .map(|b| b.path.to_string_lossy().into_owned())
                {
                    let done = !self.state.finished.get(&key).copied().unwrap_or(false);
                    self.state.finished.insert(key, done);
                    self.state.save();
                    self.refresh_progress();
                    self.log.push(format!(
                        "livre marqué {}",
                        if done { "terminé" } else { "non terminé" }
                    ));
                }
                self.book_menu = None;
                self.render_library_with(true);
            }
            Hit::AskDelete(i) => {
                self.book_menu = Some((i, true));
                // The panel changes height: the list underneath is redrawn.
                self.render_library_with(true);
                self.render_book_menu();
            }
            Hit::ConfirmDelete(i) => {
                self.book_menu = None;
                self.delete_book(i);
                let per_page =
                    1.max(((self.canvas.height as i32 - STATUS_H - 84 - 80) / 120) as usize);
                self.library_page = self
                    .library_page
                    .min(self.library.books.len().saturating_sub(1) / per_page);
                self.render_library();
            }
            Hit::CloseBookMenu => {
                self.book_menu = None;
                self.render_library_with(true);
            }
            Hit::ToggleNight => {
                self.state.settings.night = !self.state.settings.night;
                self.state.save();
                self.log.push(format!(
                    "mode nuit : {}",
                    if self.state.settings.night {
                        "oui"
                    } else {
                        "non"
                    }
                ));
                // The whole page changes color: a full refresh, without trails.
                self.draw_reader();
                self.render_settings();
                let r = self.full_rect();
                self.present(r, Waveform::Gc16, true);
                self.partial_since_full = 0;
            }
            Hit::ToggleBookmark => self.toggle_bookmark(),
            Hit::GoToPage => {
                self.goto_input.clear();
                self.draw_reader();
                self.render_goto();
            }
            Hit::Search => {
                // A search already done in this book: its results; otherwise the keyboard.
                self.search.keyboard = !self.search.done;
                self.render_search(Refresh::Full);
            }
            Hit::Key(c) => match self.view {
                View::GoToPage if c.is_ascii_digit() && self.goto_input.len() < 5 => {
                    self.goto_input.push(c);
                    self.refresh_goto_field();
                }
                View::Search
                    if self.search.query.chars().count() < 60
                    // No leading space and no double space.
                    && (c != ' '
                        || !(self.search.query.is_empty() || self.search.query.ends_with(' '))) =>
                {
                    self.search.query.push(c);
                    self.search.done = false;
                    self.render_search(Refresh::Field);
                }
                _ => {}
            },
            Hit::KeyErase => match self.view {
                View::GoToPage => {
                    self.goto_input.pop();
                    self.refresh_goto_field();
                }
                View::Search => {
                    self.search.query.pop();
                    self.search.done = false;
                    self.render_search(Refresh::Field);
                }
                _ => {}
            },
            Hit::KeyEnter => match self.view {
                View::GoToPage => {
                    let n = self.goto_input.parse::<usize>().ok();
                    self.goto_input.clear();
                    match n {
                        Some(n) => self.go_to_page(n),
                        None => {
                            let w = self.page_wfm();
                            self.render_reader(w);
                        }
                    }
                }
                View::Search => self.run_search(),
                _ => {}
            },
            Hit::CloseKeypad => {
                self.goto_input.clear();
                let w = self.page_wfm();
                self.render_reader(w);
            }
            Hit::SearchField => {
                self.search.keyboard = true;
                self.render_search(Refresh::Soft);
            }
            Hit::SearchPage(p) => {
                self.search.page = p;
                self.render_search(Refresh::Soft);
            }
            Hit::SearchResult(i) => self.open_search_result(i),
            Hit::FollowLink(i) => self.follow_link(i),
            Hit::LinkBack => {
                if let Some(o) = self.open.as_mut() {
                    o.search_mark = None;
                    if let Some((ch, a, _)) = o.link_back.take() {
                        if ch == o.chapter && !o.pages.is_empty() {
                            o.page = page_for_anchor(&o.pages, a);
                        } else {
                            o.chapter = ch;
                            o.pages.clear();
                            o.page = 0;
                            o.pending_anchor = Some(a);
                        }
                        o.anchor_hold = Some(a);
                    }
                }
                let w = self.page_wfm();
                self.render_reader(w);
                self.note_position();
            }
            Hit::CloseNote => {
                self.note = None;
                let w = self.page_wfm();
                self.render_reader(w);
            }
            Hit::GoToNote => {
                if let Some(n) = self.note.take() {
                    let w = self.page_wfm();
                    self.jump_to(n.chapter, n.fragment, true, w);
                }
            }
            Hit::GoToToc(k) => {
                let target = self.open.as_ref().and_then(|o| {
                    o.book
                        .toc
                        .get(k)
                        .and_then(|e| o.book.spine_index(&e.href).map(|i| (i, e.fragment.clone())))
                });
                if let Some((chapter, fragment)) = target {
                    self.jump_to(chapter, fragment, false, Waveform::Gc16);
                    self.save_position();
                }
            }
            Hit::Home => self.go_home(),
            // Simple tap on the panel title: nothing (see the long press).
            Hit::DevSecret => {}
        }
    }

    /// Toggles the .signet/dev file then reboots (the daemon reads the mode at boot).
    fn toggle_dev_mode(&mut self) {
        let flag = Path::new(&self.state_path).with_file_name("dev");
        let r = crate::device::with_rw(|| {
            if flag.exists() {
                std::fs::remove_file(&flag)
            } else {
                std::fs::write(&flag, "")
            }
        });
        match r {
            Ok(()) => {
                let to_dev = flag.exists();
                self.log.push(format!(
                    "mode {} : redémarrage",
                    if to_dev { "développement" } else { "lecture" }
                ));
                // What follows (saving, rebooting) changes nothing on screen for
                // a minute: it says so right away.
                let book = self.current_entry().cloned();
                let caption = if to_dev {
                    "Redémarrage en mode dev, une minute environ…"
                } else {
                    "Redémarrage en mode lecture, une minute environ…"
                };
                ui::sleep_screen(
                    &mut self.canvas,
                    &mut self.ts,
                    book.as_ref(),
                    caption,
                    Icon::Power,
                );
                self.screen.snapshot(&self.canvas, "14-redemarrage");
                let r = self.full_rect();
                self.present(r, Waveform::Gc16, true);
                self.save_position();
                self.save_shape_cache();
                let _ = std::process::Command::new("/bin/sync").status();
                self.exit_code = 3;
                self.quit = true;
            }
            Err(e) => self.log.push(format!("bascule de mode impossible : {e}")),
        }
    }

    /// After a typographic change: recomposes the page behind the
    /// panel, then redraws the panel over it.
    fn relayout_keep_settings(&mut self) {
        if let Some(o) = self.open.as_mut() {
            // The passage read stays on screen (before: return to the start of the chapter).
            let anchor = o
                .anchor_hold
                .or_else(|| o.pages.get(o.page).and_then(page_anchor));
            o.pages.clear();
            o.page = 0;
            o.pending_anchor = anchor;
            o.anchor_hold = anchor;
        }
        self.draw_reader();
        self.render_settings();
        self.note_position();
    }

    fn render_settings_only(&mut self) {
        self.render_settings();
    }

    // ---------------------------------------------------------- loop ----

    /// Will the action take a noticeable time? Opening a book, composing a
    /// chapter not composed yet, recomposing after a typographic setting,
    /// opening the dictionary the first time.
    fn slow_action(&self, hit: Hit) -> bool {
        let cfg = self.page_config();
        let ready = |o: &Open, ch: usize| {
            (o.chapter == ch && o.cache_cfg == Some(cfg) && !o.pages.is_empty())
                || o.next
                    .as_ref()
                    .map(|(c, c2, _)| *c == ch && *c2 == cfg)
                    .unwrap_or(false)
        };
        let open_ready = |path: Option<&Path>| match (path, self.open.as_ref()) {
            (Some(p), Some(o)) if p == o.path => !ready(o, o.chapter),
            _ => true,
        };
        match hit {
            Hit::Resume => open_ready(self.current_entry().map(|b| b.path.as_path())),
            Hit::OpenBook(i) => open_ready(self.library.books.get(i).map(|b| b.path.as_path())),
            Hit::GoToChapter(i) => self.open.as_ref().map(|o| !ready(o, i)).unwrap_or(false),
            Hit::GoToToc(k) => self
                .open
                .as_ref()
                .and_then(|o| {
                    o.book
                        .toc
                        .get(k)
                        .and_then(|e| o.book.spine_index(&e.href))
                        .map(|i| !ready(o, i))
                })
                .unwrap_or(false),
            Hit::GoToAnnotation(i) => self
                .open
                .as_ref()
                .and_then(|o| o.annotations.items.get(i).map(|a| !ready(o, a.chapter)))
                .unwrap_or(false),
            Hit::SearchResult(i) => self
                .open
                .as_ref()
                .and_then(|o| self.search.results.get(i).map(|f| !ready(o, f.chapter)))
                .unwrap_or(false),

            Hit::SetSize(_)
            | Hit::SetMargins(_)
            | Hit::SetLeading(_)
            | Hit::SetInk(_)
            | Hit::ToggleFastPages
            | Hit::SetFont(_) => true,
            Hit::Define => self.dict.is_none(),
            _ => false,
        }
    }

    fn tap(&mut self, x: i32, y: i32) {
        // A link in the text takes precedence over the page zones (except the "Revenir" button).
        if let Some(i) = self.link_at(x, y) {
            if !self
                .zones
                .iter()
                .any(|z| z.hit == Hit::LinkBack && z.contains(x, y))
            {
                self.handle_hit(Hit::FollowLink(i));
                return;
            }
        }
        let zone = self.zones.iter().find(|z| z.contains(x, y)).cloned();
        match zone {
            Some(z) => {
                // Touch taken into account: the button inverts immediately, but only
                // if the action will take a noticeable time. A quick action shows
                // its result faster than the inversion finishes, and the inversion
                // would only delay it (the controller finishes one update before
                // starting another on the same pixels). In DU, clean: no forced
                // full refresh afterwards. Not for the page zones
                // (thirds of the screen).
                let button_like = !matches!(
                    z.hit,
                    Hit::PrevPage
                        | Hit::NextPage
                        | Hit::ToggleSettings
                        | Hit::Home
                        | Hit::CancelSelection
                        | Hit::Back
                ) && z.w < (self.canvas.width as i32) - 2 * MARGIN + 1
                    && z.h <= 130;
                if button_like && self.slow_action(z.hit) {
                    let r = Rect::new(z.x.max(0) as u32, z.y.max(0) as u32, z.w as u32, z.h as u32);
                    self.canvas.invert_rect(r);
                    self.present(r, Waveform::Du, false);
                    self.canvas.invert_rect(r);
                }
                self.handle_hit(z.hit)
            }
            None => self.log.push(format!("tap ({x},{y}) hors zone")),
        }
    }

    /// Pre-composes the next chapter during inactivity: opening it
    /// becomes instant. Only once per current chapter.
    /// Idle time: counts the pages of a chapter not yet composed (the nearest
    /// after the current chapter first), for exact numbering across the
    /// whole book. One chapter per call; when everything is counted, the result is kept.
    fn count_pages_idle(&mut self) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_mut() else { return };
        if o.counts_cfg != Some(cfg) {
            let (ch, n) = (o.chapter, o.pages.len());
            if o.cache_cfg == Some(cfg) && n > 0 {
                o.record_count(cfg, ch, n);
            } else {
                return;
            }
        }
        let n_ch = o.counts.len();
        let target = (o.chapter..n_ch)
            .chain(0..o.chapter)
            .find(|&i| o.counts[i].is_none());
        match target {
            Some(ci) => {
                let t0 = Instant::now();
                let blocks = o.book.chapter(ci).unwrap_or_default();
                let book = &mut o.book;
                let images = &mut o.images;
                // Images: their size only (read from the header).
                let mut image = |href: &str| images.size(book, href, &cfg);
                let n = paginate(&blocks, &self.ts, &cfg, &mut image).len().max(1);
                o.record_count(cfg, ci, n);
                self.log.push(format!(
                    "pagination : chapitre {ci} = {n} pages ({:.0} ms)",
                    t0.elapsed().as_secs_f64() * 1000.0
                ));
            }
            None if !o.counts_saved => {
                let mut txt = pages_fingerprint(&cfg);
                for c in &o.counts {
                    txt.push('\n');
                    txt.push_str(&c.map(|n| n.to_string()).unwrap_or_default());
                }
                txt.push('\n');
                let file = self
                    .cache_dir
                    .join(format!("{}.pages", crate::library::cache_key(&o.path)));
                let tmp = file.with_extension("pages.tmp");
                let ok = crate::device::with_rw(|| {
                    std::fs::create_dir_all(&self.cache_dir).is_ok()
                        && std::fs::write(&tmp, &txt).is_ok()
                        && std::fs::rename(&tmp, &file).is_ok()
                });
                o.counts_saved = true;
                self.log.push(format!(
                    "pagination du livre complète : {} pages{}",
                    o.counts.iter().map(|c| c.unwrap_or(0)).sum::<usize>(),
                    if ok { ", enregistrée" } else { "" }
                ));
            }
            None => {}
        }
    }

    fn prefetch_next_chapter(&mut self) {
        let cfg = self.page_config();
        let Some(o) = self.open.as_mut() else { return };
        let next = o.chapter + 1;
        if next >= o.book.spine.len()
            || o.next
                .as_ref()
                .map(|(c, cfg2, _)| *c == next && *cfg2 == cfg)
                .unwrap_or(false)
        {
            return;
        }
        let t0 = Instant::now();
        let chapter = o.book.chapter_full(next).unwrap_or_default();
        let book = &mut o.book;
        let images = &mut o.images;
        let mut image = |href: &str| images.size(book, href, &cfg);
        let pages = paginate(&chapter.blocks, &self.ts, &cfg, &mut image);
        let texts: Vec<String> = chapter.blocks.iter().map(|b| b.text()).collect();
        let n = pages.len().max(1);
        o.next = Some((
            next,
            cfg,
            Composed {
                pages,
                texts,
                links: chapter.links,
                anchors: chapter.anchors,
            },
        ));
        o.record_count(cfg, next, n);
        self.log.push(format!(
            "chapitre {next} précomposé en {:.0} ms",
            t0.elapsed().as_secs_f64() * 1000.0
        ));
    }

    /// Two-tap touch calibration. `raw` = raw extent of the sensor.
    /// Returns the transformation found (already saved in the state).
    pub fn calibrate(&mut self, raw: (i32, i32)) -> Option<signet_input::Transform> {
        let w = self.canvas.width as i32;
        let h = self.canvas.height as i32;
        let targets = [(60, 60), (w - 60, h - 60)];
        let mut points = Vec::new();
        for (step, t) in targets.iter().enumerate() {
            ui::calibration(&mut self.canvas, &mut self.ts, *t, step as u8);
            self.screen
                .snapshot(&self.canvas, &format!("00-calibration-{step}"));
            let r = self.full_rect();
            self.present(r, Waveform::Gc16, true);
            loop {
                let Some(ev) = self.input.next(Duration::from_secs(120)) else {
                    if self.input.exhausted() {
                        return None;
                    } else {
                        continue;
                    }
                };
                match ev {
                    Event::Touch {
                        phase: Phase::Up,
                        x,
                        y,
                        ..
                    } => {
                        points.push((x, y));
                        break;
                    }
                    Event::Key {
                        code: signet_input::KEY_POWER,
                        pressed: true,
                    } => return None,
                    _ => {}
                }
            }
        }
        let t = signet_input::Transform::calibrate(raw.0, raw.1, points[0], points[1], w, h);
        self.log
            .push(format!("calibration : brut {:?} → {:?}", points, t));
        self.state.touch = Some(t.to_string());
        self.state.save();
        Some(t)
    }

    pub fn touch_transform(&self) -> Option<signet_input::Transform> {
        self.state
            .touch
            .as_deref()
            .and_then(signet_input::Transform::parse)
    }

    /// Applies the lighting: the chosen daytime level, modulated by the time of day
    /// if automatic adaptation is active and no recent manual adjustment
    /// suspends it.
    fn apply_light(&mut self, reason: &str) {
        let s = &self.state.settings;
        let manual_recent = self
            .manual_light_at
            .map(|t| t.elapsed() < Duration::from_secs(30 * 60))
            .unwrap_or(false);
        let level = if s.auto_light && !manual_recent {
            crate::device::auto_light_level(
                s.light,
                crate::device::local_hour(s.utc_offset_minutes, s.eu_dst),
            )
        } else {
            s.light
        };
        if Some(level) != self.applied_light {
            match crate::device::set_frontlight(level) {
                Ok(()) => self.log.push(format!("éclairage {level} ({reason})")),
                Err(e) => self.log.push(format!("éclairage {level} impossible : {e}")),
            }
            self.applied_light = Some(level);
        }
        self.last_light_check = Some(Instant::now());
    }

    /// Contrast and weight of the ink according to the setting.
    fn apply_ink(&mut self) {
        let (gamma, emb) = crate::theme::INKS[self.state.settings.ink.min(2)];
        self.ts.set_ink(gamma, emb);
    }

    pub fn run(&mut self) {
        if let Some(m) = crate::device::protect_onboard() {
            self.log.push(m);
        }
        self.ts.set_binary(self.state.settings.fast_pages);
        self.apply_ink();
        // Restart (new version, or application stop) during reading:
        // we return to the page, not to the home screen.
        // The shaping cache is used for composing: reloaded before opening the book, but
        // after the home screen (which is thus displayed about a hundred ms earlier).
        let resume = std::fs::remove_file("/tmp/signet-reprise").is_ok();
        match self.current_entry().map(|b| b.path.clone()) {
            Some(p) if resume => {
                self.log
                    .push("relance pendant la lecture : retour à la page".into());
                self.load_shape_cache();
                self.resume_or_open(&p);
            }
            _ => {
                self.render_home();
                self.load_shape_cache();
            }
        }
        // Screen initialized: the dev-mode hook can power the Wi-Fi chip (we
        // avoid initializing the screen and the SDIO bus at the same time).
        let _ = std::fs::write("/tmp/signet-ready", "");
        self.apply_light("démarrage");
        if crate::device::dev_mode() {
            self.log
                .push("mode dev : Wi-Fi conservé, pas de veille, appui long = redémarrage".into());
            // Safety net: if the daemon did not get an address, rerun the hook ourselves.
            if !std::path::Path::new("/tmp/signet-ip").exists()
                && std::path::Path::new("/mnt/onboard/.signet/dev.sh").exists()
            {
                self.log
                    .push("mode dev : pas d'IP, lancement du hook dev.sh".into());
                crate::device::run_dev_hook();
            }
        } else {
            for l in crate::device::wifi_off() {
                self.log.push(format!("wifi : {l}"));
            }
        }
        let _ = crate::device::flush_log();
        loop {
            if self.input.exhausted() || self.quit {
                break;
            }
            // Read timeout: short when a finger is down (long-press detection).
            let timeout = match self.touch_down {
                Some((_, _, t0))
                    if !self.touch_moved && self.drag.is_none() && self.view == View::Reader =>
                {
                    Self::LONG_PRESS
                        .saturating_sub(t0.elapsed())
                        .max(Duration::from_millis(5))
                }
                // Finger on "Lecture": the toggle starts after 2 s, without waiting for the release.
                Some((x0, y0, t0)) if !self.touch_moved && self.on_dev_secret(x0, y0) => {
                    Self::DEV_PRESS
                        .saturating_sub(t0.elapsed())
                        .max(Duration::from_millis(5))
                }
                // Drag in progress with a render pending: just the interval's time.
                Some(_) if self.drag.is_some() && self.live_dirty => self
                    .live_at
                    .map(|t| Self::LIVE_INTERVAL.saturating_sub(t.elapsed()))
                    .unwrap_or(Duration::ZERO)
                    .max(Duration::from_millis(5)),
                // USB probe: check often whether the computer has enumerated the reader.
                _ if self.usb_probe.is_some() => Duration::from_millis(200),
                _ => Duration::from_secs(2),
            };
            let ev = self.input.next(timeout);
            // Shutdown requested (the system reboots or powers off): position first.
            if crate::device::term_requested() {
                self.log
                    .push("arrêt demandé (SIGTERM) : position écrite, sortie".into());
                self.mark_resume();
                if !self.usb_exported {
                    self.save_position();
                }
                let _ = crate::device::flush_log();
                self.exit_code = 4;
                return;
            }
            // Drag: the selection catches up with the finger even if it has stopped.
            if ev.is_none() && self.drag.is_some() && self.live_dirty && self.touch_down.is_some() {
                self.live_render();
                continue;
            }
            // Press held on "Lecture": dev-mode toggle, the screen says so immediately.
            if ev.is_none() {
                if let Some((x0, y0, t0)) = self.touch_down {
                    if !self.touch_moved
                        && self.on_dev_secret(x0, y0)
                        && t0.elapsed() >= Self::DEV_PRESS
                    {
                        self.touch_down = None;
                        self.log
                            .push("appui long sur « Lecture » : bascule du mode dev".into());
                        self.toggle_dev_mode();
                        if self.quit {
                            break;
                        }
                        continue;
                    }
                }
            }
            // Press held without moving: immediate selection of the word.
            if ev.is_none() {
                if let Some((x, y, t0)) = self.touch_down {
                    if !self.touch_moved
                        && self.drag.is_none()
                        && self.view == View::Reader
                        && t0.elapsed() >= Self::LONG_PRESS
                    {
                        self.begin_drag(x, y);
                        if self.drag.is_none() {
                            // Outside the text: we neutralize the touch so as not to turn the page on release.
                            self.touch_moved = true;
                        }
                        continue;
                    }
                }
            }
            // USB cable: a computer or a charger? We probe before exporting;
            // the partition is taken back at unplug or when the computer ejects
            // the disk (the reader then stays charging, reading resumes).
            let plugged = crate::device::usb_plugged();
            if plugged != self.usb_plugged {
                self.usb_plugged = plugged;
                self.log.push(format!(
                    "câble USB {}",
                    if plugged { "branché" } else { "débranché" }
                ));
                if plugged && !self.usb_exported {
                    self.usb_probe_start();
                } else if !plugged {
                    if self.usb_exported {
                        self.usb_disconnect();
                    } else if self.usb_probe.take().is_some() {
                        for l in crate::device::usb_gadget_unload() {
                            self.log.push(format!("usb : {l}"));
                        }
                    }
                }
            }
            self.usb_probe_poll();
            if self.usb_exported && crate::device::usb_medium_ejected() {
                self.log.push(
                    "usb : disque éjecté par l'ordinateur : retour à la lecture, en charge".into(),
                );
                self.usb_disconnect();
            }
            if ev.is_some() {
                self.last_activity = Instant::now();
            }
            let Some(ev) = ev else {
                // Idle: the chance to copy the log to the partition.
                if !self.usb_exported {
                    let _ = crate::device::flush_log();
                }
                // Time passes: the status bar corner follows, without waiting for a page.
                self.tick_status();
                // Pages turned: the position is written once the gesture is over.
                if self.position_dirty && !self.usb_exported {
                    self.save_position();
                }
                // New version installed (development tools): quit
                // cleanly with code 4, which signetd restarts immediately. An application
                // update no longer requires rebooting the reader.
                if std::fs::remove_file("/tmp/signet-restart").is_ok() && !self.usb_exported {
                    self.log.push(
                        "relance demandée : position et cache écrits, sortie (code 4)".into(),
                    );
                    self.mark_resume();
                    self.save_position();
                    self.save_shape_cache();
                    let _ = crate::device::flush_log();
                    self.exit_code = 4;
                    return;
                }
                if self
                    .last_light_check
                    .map(|t| t.elapsed() >= Duration::from_secs(600))
                    .unwrap_or(true)
                {
                    self.apply_light("heure");
                }
                if self.view == View::Reader && !self.usb_exported {
                    self.prefetch_next_chapter();
                    self.count_pages_idle();
                }
                // Automatic sleep after N minutes without interaction (except in dev mode and USB export).
                let idle = self.state.settings.idle_minutes;
                if idle > 0
                    && !self.usb_exported
                    && !crate::device::dev_mode()
                    && self.last_activity.elapsed() >= Duration::from_secs(idle as u64 * 60)
                {
                    self.log
                        .push(format!("inactif depuis {idle} min : veille automatique"));
                    self.sleep();
                    continue;
                }
                // Dev mode without an IP address: rerun the network hook every 60 s.
                if crate::device::dev_mode()
                    && !self.usb_exported
                    && !std::path::Path::new("/tmp/signet-ip").exists()
                    && self
                        .last_hook
                        .map(|t| t.elapsed() >= Duration::from_secs(60))
                        .unwrap_or(true)
                    && std::path::Path::new("/mnt/onboard/.signet/dev.sh").exists()
                {
                    self.last_hook = Some(Instant::now());
                    self.log
                        .push("mode dev : toujours pas d'IP, relance de dev.sh".into());
                    crate::device::run_dev_hook();
                }
                continue;
            };
            if self.usb_exported {
                // During the export, only the very long press (reboot) is listened for.
                if let Event::Key {
                    code: signet_input::KEY_POWER,
                    pressed,
                } = ev
                {
                    if pressed {
                        self.power_down = Some(Instant::now());
                    } else if self
                        .power_down
                        .take()
                        .map(|t| t.elapsed() >= Duration::from_secs(5))
                        .unwrap_or(false)
                    {
                        self.log.push("redémarrage pendant l'export USB".into());
                        self.exit_code = 3;
                        return;
                    }
                }
                continue;
            }
            match ev {
                Event::Touch {
                    phase: Phase::Down,
                    x,
                    y,
                    ..
                } => {
                    self.touch_down = Some((x, y, Instant::now()));
                    self.touch_moved = false;
                }
                Event::Touch {
                    phase: Phase::Move,
                    x,
                    y,
                    ..
                } => {
                    if self.drag.is_some() {
                        self.drag_to(x, y);
                    } else if let Some((x0, y0, _)) = self.touch_down {
                        if (x - x0).abs() > 15 || (y - y0).abs() > 15 {
                            self.touch_moved = true;
                        }
                    }
                }
                Event::Touch {
                    phase: Phase::Up,
                    x,
                    y,
                    ..
                } => {
                    if let Some((x0, y0, t0)) = self.touch_down.take() {
                        let dx = x - x0;
                        let dy = y - y0;
                        let held = t0.elapsed();
                        let moved = self.touch_moved;
                        self.touch_moved = false;
                        if self.drag.is_some() {
                            self.drag_to(x, y);
                            self.end_drag();
                        } else if moved
                            && !(self.view == View::Reader
                                && self.selection.is_none()
                                && dx.abs() > 60
                                && dx.abs() > dy.abs())
                        {
                            // Moved without being a page drag: nothing.
                        } else if !moved
                            && held >= Duration::from_millis(2000)
                            && self.view == View::Settings
                            && self
                                .zones
                                .iter()
                                .any(|z| z.contains(x0, y0) && z.hit == Hit::DevSecret)
                        {
                            // Long press on "Lecture": dev-mode toggle (reboot).
                            self.log
                                .push("appui long sur « Lecture » : bascule du mode dev".into());
                            self.toggle_dev_mode();
                        } else if !moved && held >= Self::LONG_PRESS && self.view == View::Reader {
                            self.start_selection(x, y);
                        } else if !moved && held >= Self::LONG_PRESS && self.view == View::Library {
                            // Long press on a book: its menu (like the ⋮ on the row).
                            if let Some(Hit::OpenBook(i)) = self
                                .zones
                                .iter()
                                .find(|z| z.contains(x0, y0) && matches!(z.hit, Hit::OpenBook(_)))
                                .map(|z| z.hit)
                            {
                                self.handle_hit(Hit::BookMenu(i));
                            }
                        } else if !moved
                            && dx.abs() < 24
                            && dy.abs() < 24
                            && self.view == View::Reader
                            && self.selection.is_some()
                        {
                            // Selection in progress: action bar first, otherwise extension.
                            match self.zones.iter().find(|z| z.contains(x, y)).map(|z| z.hit) {
                                Some(h) => self.handle_hit(h),
                                None => self.extend_selection(x, y),
                            }
                        } else if !moved && dx.abs() < 24 && dy.abs() < 24 {
                            self.tap(x, y);
                        } else if self.view == View::Reader
                            && dx.abs() > 60
                            && dx.abs() > dy.abs()
                            && self.selection.is_none()
                        {
                            // horizontal drag: next / previous page
                            if dx < 0 {
                                self.next_page()
                            } else {
                                self.prev_page()
                            }
                        }
                    }
                }
                Event::Key {
                    code: signet_input::KEY_POWER,
                    pressed: true,
                } => {
                    self.power_down = Some(Instant::now());
                }
                Event::Key {
                    code: signet_input::KEY_POWER,
                    pressed: false,
                } => {
                    // Release with no recorded touch (typically: the button that
                    // woke us): we ignore it, otherwise we would fall asleep again immediately.
                    let Some(t0) = self.power_down.take() else {
                        continue;
                    };
                    let held = t0.elapsed();
                    if crate::device::dev_mode() {
                        if held >= Duration::from_millis(1500) {
                            self.log.push("mode dev : appui long → redémarrage".into());
                            self.save_position();
                            self.exit_code = 3;
                            return;
                        }
                        if Path::new(&self.state_path)
                            .with_file_name("allow-sleep-in-dev")
                            .exists()
                        {
                            self.log.push(
                                "mode dev : veille autorisée pour essai (allow-sleep-in-dev)"
                                    .into(),
                            );
                            self.sleep();
                            crate::device::run_dev_hook();
                            continue;
                        }
                        self.log.push(
                            "mode dev : appui court ignoré (pas de veille en mode dev)".into(),
                        );
                        continue;
                    }
                    if held >= Duration::from_secs(5) {
                        // Very long press: full reboot (signetd runs reboot).
                        self.log.push("bouton power très long : redémarrage".into());
                        self.save_position();
                        self.exit_code = 3;
                        return;
                    }
                    if held >= Duration::from_millis(1500) {
                        // Long press: hand back control (signetd restarts Nickel).
                        self.log.push("bouton power long : sortie".into());
                        self.save_position();
                        return;
                    }
                    self.log.push("bouton power : veille".into());
                    self.sleep();
                }
                Event::Key { .. } => {}
                Event::Cover(closed) => {
                    self.log.push(format!("capot fermé={closed}"));
                    if closed {
                        self.sleep();
                    }
                }
            }
        }
        self.save_position();
        self.save_shape_cache();
        let _ = crate::device::flush_log();
    }

    pub fn input_mut(&mut self) -> &mut I {
        &mut self.input
    }
}

#[cfg(test)]
mod tests_position {
    use super::*;
    use crate::reader::{paginate, PageConfig};
    use signet_epub::{Block, BlockStyle, Span, SpanStyle};

    /// A position taken at one text size finds, after a new layout
    /// at another size, the page that contains the same passage.
    #[test]
    fn anchor_survives_relayout() {
        let ts = crate::typesetter();
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
        let blocks: Vec<Block> = (0..60usize)
            .map(|i| {
                let text = (0..(i * 37) % 70 + 20)
                    .map(|w| vocab[(w * 7 + i) % vocab.len()])
                    .collect::<Vec<_>>()
                    .join(" ");
                Block::Paragraph {
                    spans: vec![Span {
                        text,
                        style: SpanStyle::default(),
                    }],
                    style: BlockStyle::default(),
                }
            })
            .collect();
        let cfg = |px: f32| PageConfig {
            px,
            line_height: 1.45,
            margin_x: 64,
            margin_top: 92,
            margin_bottom: 64,
            screen_w: 758,
            screen_h: 1024,
            family: 0,
        };
        let big = paginate(&blocks, &ts, &cfg(30.0), &mut |_| None);
        let small = paginate(&blocks, &ts, &cfg(26.0), &mut |_| None);
        assert!(big.len() > small.len() && small.len() > 5);
        for (k, page) in big.iter().enumerate().skip(1) {
            let anchor = page_anchor(page).unwrap();
            let found = page_for_anchor(&small, anchor);
            // The anchor's character is on the page found.
            let has = small[found].items.iter().any(|it| {
                it.block == anchor.0
                    && it
                        .lines
                        .iter()
                        .any(|l| l.glyphs.iter().any(|g| g.ch == anchor.1))
            });
            assert!(
                has,
                "page {k} : ancre {anchor:?} introuvable sur la page {found}"
            );
            // And the converse: the same starting page.
            assert_eq!(page_for_anchor(&big, anchor), k);
        }
    }

    /// The state file keeps the anchor, and rereads the old format without it.
    #[test]
    fn state_keeps_anchor() {
        let dir = std::env::temp_dir().join(format!("signet-state-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("state.txt");
        std::fs::write(&path, "tz=120\npos:/a.epub=8,4\npos:/b.epub=3,2,17,512\n").unwrap();
        let st = crate::state::State::load(path.to_str().unwrap());
        assert_eq!(
            st.positions["/a.epub"],
            Position {
                chapter: 8,
                page: 4,
                anchor: None
            }
        );
        assert_eq!(
            st.positions["/b.epub"],
            Position {
                chapter: 3,
                page: 2,
                anchor: Some((17, 512))
            }
        );
        assert_eq!(
            (st.settings.utc_offset_minutes, st.settings.eu_dst),
            (60, true)
        );
        st.save();
        let again = crate::state::State::load(path.to_str().unwrap());
        assert_eq!(again.positions["/b.epub"].anchor, Some((17, 512)));
        let txt = std::fs::read_to_string(&path).unwrap();
        assert!(
            txt.contains("utc_offset=60") && txt.contains("dst=1") && !txt.contains("tz="),
            "{txt}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests_liens {
    use super::*;
    use crate::backend::{HostScreen, ScriptedInput};

    fn current_has(app: &App<HostScreen, ScriptedInput>, anchor: (usize, u32)) -> bool {
        let o = app.open.as_ref().unwrap();
        let p = &o.pages[o.page];
        page_anchor(p).map(|a| a <= anchor).unwrap_or(false)
            && (o.page + 1 == o.pages.len()
                || page_anchor(&o.pages[o.page + 1])
                    .map(|a| a > anchor)
                    .unwrap_or(true))
    }

    /// On a real book (SIGNET_EPUB_SAMPLE): table of contents to an element, note
    /// call → window → "Aller à la note" → "Revenir", a note's back-reference to
    /// its call (a jump, not a window), link to another chapter.
    #[test]
    fn notes_and_links_end_to_end() {
        let Ok(path) = std::env::var("SIGNET_EPUB_SAMPLE") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("signet-liens-{}", std::process::id()));
        let books = dir.join("livres");
        let _ = std::fs::create_dir_all(&books);
        let book_path = books.join("livre.epub");
        std::fs::copy(&path, &book_path).unwrap();
        let screen = HostScreen::new(
            dir.join("captures").to_str().unwrap(),
            crate::theme::SCREEN_W,
            crate::theme::SCREEN_H,
        );
        let mut app = App::new(
            screen,
            ScriptedInput::new(Vec::new()),
            crate::typesetter(),
            &books,
            &dir.join("state"),
        );
        app.open_book(&book_path);

        // Table of contents: an entry that targets an element in the middle of a chapter.
        let (k, chapter, fragment) = {
            let o = app.open.as_ref().unwrap();
            o.book
                .toc
                .iter()
                .enumerate()
                .find_map(|(k, e)| {
                    let f = e.fragment.clone()?;
                    let ch = o.book.spine_index(&e.href)?;
                    Some((k, ch, f))
                })
                .expect("une entrée de sommaire avec fragment")
        };
        app.do_hit(Hit::GoToToc(k));
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!(o.chapter, chapter);
            let a = o.anchors[&fragment];
            assert!(
                current_has(&app, (a.block, a.ch)),
                "entrée {k} ({fragment}) : page {}",
                o.page
            );
            assert!(o.link_back.is_none());
        }
        let (_, current) = app.toc_entries();
        assert_eq!(
            current,
            Some(
                app.open
                    .as_ref()
                    .unwrap()
                    .book
                    .toc
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| app
                        .open
                        .as_ref()
                        .unwrap()
                        .book
                        .spine_index(&e.href)
                        .is_some())
                    .position(|(i, _)| i == k)
                    .unwrap()
            )
        );

        // First chapter that has a note call.
        let (ci, li) = {
            let o = app.open.as_mut().unwrap();
            (0..o.book.spine.len())
                .find_map(|ci| {
                    let ch = o.book.chapter_full(ci).ok()?;
                    ch.links
                        .iter()
                        .position(|l| l.note && l.from > 0)
                        .map(|li| (ci, li))
                })
                .unwrap()
        };
        app.do_hit(Hit::GoToChapter(ci));
        let (call, call_page) = {
            let o = app.open.as_mut().unwrap();
            let l = o.links[li].clone();
            o.page = App::<HostScreen, ScriptedInput>::page_containing(o, l.block, l.from).unwrap();
            (l, o.page)
        };
        app.do_hit(Hit::FollowLink(li));
        assert_eq!(app.view, View::Note);
        assert_eq!(app.note.as_ref().unwrap().title, "Note 1");
        app.do_hit(Hit::GoToNote);
        assert_eq!(app.view, View::Reader);
        {
            let o = app.open.as_ref().unwrap();
            let a = o.anchors[call.fragment.as_ref().unwrap()];
            assert!(current_has(&app, (a.block, a.ch)), "la page de la note");
            assert_eq!(o.link_back.as_ref().map(|(c, _, _)| *c), Some(ci));
        }
        app.do_hit(Hit::LinkBack);
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!(
                (o.chapter, o.page),
                (ci, call_page),
                "retour à la page de l'appel"
            );
            assert!(o.link_back.is_none());
        }

        // The note's back-reference to its call: a jump (the target is in the middle of a paragraph).
        let back_link = app
            .open
            .as_ref()
            .unwrap()
            .links
            .iter()
            .position(|l| l.note && l.from == 0 && l.fragment.is_some())
            .unwrap();
        app.do_hit(Hit::FollowLink(back_link));
        assert_eq!(app.view, View::Reader);
        assert_eq!(app.open.as_ref().unwrap().page, call_page);

        // A reference to another chapter ("p. 178").
        let found = {
            let o = app.open.as_mut().unwrap();
            (0..o.book.spine.len()).find_map(|ci| {
                let ch = o.book.chapter_full(ci).ok()?;
                let li = ch.links.iter().position(|l| {
                    !l.note
                        && !l.external
                        && l.fragment.is_some()
                        && o.book
                            .spine_index(&l.href)
                            .map(|t| t != ci)
                            .unwrap_or(false)
                })?;
                Some((ci, li, ch.links[li].clone()))
            })
        };
        let (ci, li, link) = found.unwrap();
        app.do_hit(Hit::GoToChapter(ci));
        app.do_hit(Hit::FollowLink(li));
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!(
                Some(o.chapter),
                o.book.spine_index(&link.href),
                "« {} »",
                link.label
            );
            let a = o.anchors[link.fragment.as_ref().unwrap()];
            assert!(
                current_has(&app, (a.block, a.ch)),
                "« {} » → {:?}",
                link.label,
                link.fragment
            );
            assert_eq!(o.link_back.as_ref().map(|(c, _, _)| *c), Some(ci));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests_navigation {
    use super::*;
    use crate::backend::{HostScreen, ScriptedInput};

    type TestApp = App<HostScreen, ScriptedInput>;

    /// Capture of the current screen if SIGNET_UI_DUMP is defined.
    fn dump(app: &TestApp, name: &str) {
        if let Ok(dir) = std::env::var("SIGNET_UI_DUMP") {
            app.canvas.save_pgm(&format!("{dir}/{name}.pgm")).unwrap();
        }
    }

    fn app_with_book(tag: &str) -> Option<(TestApp, PathBuf, PathBuf)> {
        let path = std::env::var("SIGNET_EPUB_SAMPLE").ok()?;
        let dir = std::env::temp_dir().join(format!("signet-{tag}-{}", std::process::id()));
        let books = dir.join("livres");
        let _ = std::fs::create_dir_all(&books);
        let book_path = books.join("livre.epub");
        std::fs::copy(&path, &book_path).unwrap();
        let screen = HostScreen::new(
            dir.join("captures").to_str().unwrap(),
            crate::theme::SCREEN_W,
            crate::theme::SCREEN_H,
        );
        let mut app = App::new(
            screen,
            ScriptedInput::new(Vec::new()),
            crate::typesetter(),
            &books,
            &dir.join("state"),
        );
        app.open_book(&book_path);
        Some((app, dir, book_path))
    }

    /// Library sort (recents, title, author by family name) and
    /// deletion of a book: file, caches, annotations and position erased.
    #[test]
    fn library_sort_and_delete() {
        let dir = std::env::temp_dir().join(format!("signet-biblio-{}", std::process::id()));
        let books = dir.join("livres");
        let _ = std::fs::create_dir_all(&books);
        let screen = HostScreen::new(
            dir.join("captures").to_str().unwrap(),
            crate::theme::SCREEN_W,
            crate::theme::SCREEN_H,
        );
        let mut app = App::new(
            screen,
            ScriptedInput::new(Vec::new()),
            crate::typesetter(),
            &books,
            &dir.join("state"),
        );
        let entry = |name: &str, title: &str, author: &str, mtime: u64| BookEntry {
            path: books.join(name),
            title: title.into(),
            author: author.into(),
            chapters: 1,
            percent: 0,
            pages: None,
            started: false,
            finished: false,
            cover: None,
            mtime,
        };
        app.library.books = vec![
            entry("a.epub", "Zadig", "Voltaire", 30),
            entry("b.epub", "Éloge de la fuite", "Henri Laborit", 20),
            entry(
                "c.epub",
                "La méthode Make Time",
                "Jake Knapp, John Zeratsky",
                10,
            ),
        ];
        for b in &app.library.books {
            std::fs::write(&b.path, b"epub").unwrap();
        }
        // Recents: the last opened book first, then the most recently copied.
        app.state
            .seen
            .insert(books.join("c.epub").to_string_lossy().into_owned(), 1_000);
        app.sort_recent();
        let titles = |app: &TestApp, order: Vec<usize>| {
            order
                .iter()
                .map(|&i| app.library.books[i].title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            titles(&app, app.library_order()),
            ["La méthode Make Time", "Zadig", "Éloge de la fuite"]
        );
        app.state.settings.library_sort = 1;
        assert_eq!(
            titles(&app, app.library_order()),
            ["Éloge de la fuite", "La méthode Make Time", "Zadig"],
            "titres sans tenir compte des accents"
        );
        app.state.settings.library_sort = 2;
        assert_eq!(
            titles(&app, app.library_order()),
            ["La méthode Make Time", "Éloge de la fuite", "Zadig"],
            "Knapp, Laborit, Voltaire"
        );
        app.do_hit(Hit::SeeAll);
        dump(&app, "biblio");
        // Delete "Zadig": menu, prompt, confirmation.
        let z = app
            .library
            .books
            .iter()
            .position(|b| b.title == "Zadig")
            .unwrap();
        let key = app.library.books[z].path.to_string_lossy().into_owned();
        let cache = crate::library::cache_key(&app.library.books[z].path);
        std::fs::create_dir_all(&app.cache_dir).unwrap();
        std::fs::write(app.cache_dir.join(format!("{cache}.meta")), "x").unwrap();
        std::fs::write(app.cache_dir.join(format!("{cache}-230x345.pgm")), "x").unwrap();
        let ann = Path::new(&app.state_path).with_file_name("annotations");
        std::fs::create_dir_all(&ann).unwrap();
        std::fs::write(ann.join(format!("{cache}.txt")), "0\t0\t0\t5\tZadig\n").unwrap();
        app.state.positions.insert(
            key.clone(),
            Position {
                chapter: 2,
                page: 3,
                anchor: None,
            },
        );
        app.do_hit(Hit::BookMenu(z));
        assert_eq!(app.view, View::BookMenu);
        dump(&app, "biblio-menu");
        app.do_hit(Hit::AskDelete(z));
        dump(&app, "biblio-supprimer");
        assert!(
            app.library.books[z].path.exists(),
            "rien d'effacé avant la confirmation"
        );
        app.do_hit(Hit::ConfirmDelete(z));
        assert_eq!(app.view, View::Library);
        assert!(!books.join("a.epub").exists());
        assert!(
            !app.cache_dir.join(format!("{cache}.meta")).exists()
                && !app.cache_dir.join(format!("{cache}-230x345.pgm")).exists()
        );
        assert!(!ann.join(format!("{cache}.txt")).exists());
        assert!(!app.state.positions.contains_key(&key));
        assert_eq!(app.library.books.len(), 2);
        // Mark as finished.
        app.do_hit(Hit::ToggleFinished(0));
        assert!(app.library.books[0].finished);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Finger held 2 s on "Lecture": the toggle starts without waiting for the release,
    /// and the screen announces the reboot (before: nothing moved for a minute).
    #[test]
    fn dev_toggle_fires_while_holding() {
        use crate::backend::Step;
        use signet_input::{Event, Phase};
        let Ok(path) = std::env::var("SIGNET_EPUB_SAMPLE") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("signet-devpress-{}", std::process::id()));
        let books = dir.join("livres");
        let _ = std::fs::create_dir_all(&books);
        std::fs::copy(&path, books.join("livre.epub")).unwrap();
        let (w, h) = (crate::theme::SCREEN_W as i32, crate::theme::SCREEN_H as i32);
        // Home screen → book → panel → finger held 2.1 s on "Lecture".
        let mut steps: Vec<Step> = Vec::new();
        steps.extend(ScriptedInput::tap(w / 2, 589));
        steps.extend(ScriptedInput::tap(w / 2, h / 2));
        steps.push(Step::Ev(Event::Touch {
            id: 1,
            phase: Phase::Down,
            x: 80,
            y: h - 692 + 50,
        }));
        steps.push(Step::Wait(std::time::Duration::from_millis(2100)));
        steps.push(Step::Ev(Event::Touch {
            id: 1,
            phase: Phase::Up,
            x: 80,
            y: h - 692 + 50,
        }));
        let screen = HostScreen::new(dir.join("captures").to_str().unwrap(), w as u32, h as u32);
        let mut app = App::new(
            screen,
            ScriptedInput::new(steps),
            crate::typesetter(),
            &books,
            &dir.join("state"),
        );
        app.run();
        assert_eq!(app.exit_code, 3, "redémarrage demandé pendant l'appui");
        assert!(
            dir.join("state").join("dev").exists(),
            "drapeau du mode dev posé"
        );
        let captures: Vec<String> = std::fs::read_dir(dir.join("captures"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            captures.iter().any(|c| c.contains("redemarrage")),
            "{captures:?}"
        );
        if let Ok(d) = std::env::var("SIGNET_UI_DUMP") {
            app.canvas
                .save_pgm(&format!("{d}/redemarrage.pgm"))
                .unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Passer en lecture" (which cuts Wi-Fi and SSH) only goes ahead on the second
    /// tap; the first one only asks for confirmation.
    #[test]
    fn leaving_dev_mode_needs_confirmation() {
        let dir = std::env::temp_dir().join(format!("signet-devmode-{}", std::process::id()));
        let books = dir.join("livres");
        let _ = std::fs::create_dir_all(&books);
        let screen = HostScreen::new(
            dir.join("captures").to_str().unwrap(),
            crate::theme::SCREEN_W,
            crate::theme::SCREEN_H,
        );
        let mut app = App::new(
            screen,
            ScriptedInput::new(Vec::new()),
            crate::typesetter(),
            &books,
            &dir.join("state"),
        );
        app.do_hit(Hit::ToggleDevMode);
        assert!(
            !app.quit && app.dev_confirm.is_some(),
            "un seul appui ne bascule pas"
        );
        app.do_hit(Hit::ToggleDevMode);
        assert!(app.quit && app.exit_code == 3, "le second appui confirme");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bookmark placed by the page corner, visible in the annotations,
    /// removed by the panel; the "Aller à la page" keypad; the search, its
    /// result marked on its page, and the return.
    #[test]
    fn bookmarks_goto_and_search() {
        let Some((mut app, dir, _)) = app_with_book("navigation") else {
            return;
        };
        // A few pages further, a bookmark by the top-right corner.
        for _ in 0..30 {
            app.next_page();
        }
        let w = app.canvas.width as i32;
        let corner = app
            .zones
            .iter()
            .find(|z| z.hit == Hit::ToggleBookmark)
            .cloned()
            .expect("coin du marque-page");
        assert!(corner.contains(w - 20, STATUS_H + 20));
        app.tap(w - 20, STATUS_H + 20);
        {
            let o = app.open.as_ref().unwrap();
            let marks: Vec<&Annotation> = o
                .annotations
                .items
                .iter()
                .filter(|a| a.is_bookmark())
                .collect();
            assert_eq!(marks.len(), 1);
            assert_eq!(
                (marks[0].chapter, marks[0].block, marks[0].from),
                (
                    o.chapter,
                    page_anchor(&o.pages[o.page]).unwrap().0,
                    page_anchor(&o.pages[o.page]).unwrap().1
                )
            );
            assert!(!marks[0].text.is_empty());
            assert_eq!(App::<HostScreen, ScriptedInput>::page_bookmarks(o).len(), 1);
        }
        dump(&app, "nav-marque-page");
        // The panel shows it placed, and removes it.
        app.do_hit(Hit::ToggleSettings);
        dump(&app, "nav-panneau");
        app.do_hit(Hit::ToggleBookmark);
        assert_eq!(app.view, View::Settings);
        assert!(app
            .open
            .as_ref()
            .unwrap()
            .annotations
            .items
            .iter()
            .all(|a| !a.is_bookmark()));
        app.do_hit(Hit::ToggleBookmark);
        app.do_hit(Hit::CloseSettings);
        let (marked_chapter, marked_page) = {
            let o = app.open.as_ref().unwrap();
            (o.chapter, o.page)
        };

        // Go to page 3, then come back.
        app.do_hit(Hit::GoToPage);
        assert_eq!(app.view, View::GoToPage);
        app.do_hit(Hit::Key('3'));
        dump(&app, "nav-aller");
        app.do_hit(Hit::KeyEnter);
        assert_eq!(app.view, View::Reader);
        assert_eq!(
            App::<HostScreen, ScriptedInput>::page_numbers(app.open.as_ref().unwrap()).0,
            3
        );
        app.do_hit(Hit::LinkBack);
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!((o.chapter, o.page), (marked_chapter, marked_page));
        }

        // Keyboard search.
        app.do_hit(Hit::Search);
        assert!(app.search.keyboard);
        for c in "temps".chars() {
            app.do_hit(Hit::Key(c));
        }
        dump(&app, "nav-clavier");
        app.do_hit(Hit::KeyEnter);
        dump(&app, "nav-resultats");
        assert!(app.search.done && !app.search.keyboard);
        let n = app.search.results.len();
        assert!(n > 20, "« temps » : {n} résultats");
        let pick = n / 2;
        let f = app.search.results[pick].clone();
        app.do_hit(Hit::SearchResult(pick));
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!(o.chapter, f.chapter);
            assert_eq!(o.search_mark, Some((f.chapter, f.block, f.from, f.to)));
            let p = &o.pages[o.page];
            let on_page = p.items.iter().any(|it| {
                it.block == f.block
                    && it
                        .lines
                        .iter()
                        .any(|l| l.glyphs.iter().any(|g| g.ch == f.from))
            });
            assert!(on_page, "le mot trouvé est sur la page affichée");
            let word: String = o.block_texts[f.block]
                .chars()
                .skip(f.from as usize)
                .take((f.to - f.from) as usize)
                .collect();
            assert_eq!(crate::search::fold(word.chars().next().unwrap()), 't');
        }
        dump(&app, "nav-trouve");
        // The results' section title comes from the table of contents.
        assert!(
            !app.section_title(f.chapter, (f.block, f.from))
                .starts_with("Chapitre "),
            "{}",
            app.section_title(f.chapter, (f.block, f.from))
        );
        app.next_page();
        assert!(
            app.open.as_ref().unwrap().search_mark.is_none(),
            "la marque s'efface en tournant la page"
        );
        app.do_hit(Hit::LinkBack);
        {
            let o = app.open.as_ref().unwrap();
            assert_eq!((o.chapter, o.page), (marked_chapter, marked_page));
        }
        // The annotations list the bookmark.
        app.do_hit(Hit::Annotations);
        assert_eq!(app.view, View::Annotations);
        dump(&app, "nav-annotations");
        // Night mode: the page in white on black, from the panel.
        app.do_hit(Hit::Back);
        app.do_hit(Hit::ToggleSettings);
        app.do_hit(Hit::ToggleNight);
        assert!(app.state.settings.night);
        dump(&app, "nav-nuit-panneau");
        app.do_hit(Hit::CloseSettings);
        let dark = app.canvas.data.iter().filter(|&&v| v < 64).count() as f32
            / app.canvas.data.len() as f32;
        assert!(
            dark > 0.6,
            "page majoritairement noire en mode nuit : {dark:.2}"
        );
        dump(&app, "nav-nuit");
        app.do_hit(Hit::ToggleSettings);
        app.do_hit(Hit::ToggleNight);
        // Second font: the book recomposed in Source Sans, the interface stays in Literata.
        let before = app.open.as_ref().unwrap().pages.len();
        app.do_hit(Hit::SetFont(1));
        assert_eq!(app.page_config().family, 1);
        assert_eq!(app.ts.family(), 0, "l'interface garde sa police");
        dump(&app, "nav-police-panneau");
        app.do_hit(Hit::CloseSettings);
        assert_eq!(app.ts.family(), 0);
        let after = app.open.as_ref().unwrap().pages.len();
        assert_ne!(
            before, after,
            "autre police, autre pagination ({before} pages)"
        );
        dump(&app, "nav-police");
        app.do_hit(Hit::ToggleSettings);
        app.do_hit(Hit::SetFont(0));
        app.do_hit(Hit::CloseSettings);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
