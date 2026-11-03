mod annotations;
mod app;
mod backend;
mod definition;
mod device;
mod draw;
mod icons;
mod library;
mod reader;
mod search;
mod state;
mod theme;
mod ui;
mod vector;

use backend::{HostScreen, KoboInput, KoboScreen, Screen, ScriptedInput};
use signet_text::{Font, Typesetter};
use std::path::Path;

static REGULAR: &[u8] = include_bytes!("../../../assets/fonts/literata/Literata-Regular.ttf");
static ITALIC: &[u8] = include_bytes!("../../../assets/fonts/literata/Literata-Italic.ttf");
static BOLD: &[u8] = include_bytes!("../../../assets/fonts/literata/Literata-Bold.ttf");
static BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/literata/Literata-BoldItalic.ttf");
static HYPH_FR: &str = include_str!("../../../assets/hyphenation/hyph-fr.tex");
// Second reading font: Source Sans 3 (Adobe, OFL), unmodified files.
static SANS_REGULAR: &[u8] =
    include_bytes!("../../../assets/fonts/source-sans/SourceSans3-Regular.ttf");
static SANS_ITALIC: &[u8] = include_bytes!("../../../assets/fonts/source-sans/SourceSans3-It.ttf");
static SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/source-sans/SourceSans3-Bold.ttf");
static SANS_BOLD_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/source-sans/SourceSans3-BoldIt.ttf");

fn typesetter() -> Typesetter {
    let f = |b: &'static [u8]| Font::from_static(b).expect("police embarquée");
    let mut ts = Typesetter::new(f(REGULAR), f(ITALIC), f(BOLD), f(BOLD_ITALIC))
        .with_hyphenation(signet_text::Hyphenator::from_tex(HYPH_FR));
    ts.add_family(
        f(SANS_REGULAR),
        f(SANS_ITALIC),
        f(SANS_BOLD),
        f(SANS_BOLD_ITALIC),
    );
    ts
}

fn usage() -> ! {
    eprintln!("usage :\n  signet                               (sur la Kobo)\n  signet --host <dossier_livres> <dossier_sortie>   (captures PGM + scénario)");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--bench") {
        let book = args.get(2).map(String::as_str).unwrap_or_else(|| usage());
        run_bench(Path::new(book), &args[3..]);
    } else if args.get(1).map(String::as_str) == Some("--render") {
        // signet --render book.epub <chapter> <directory>: the pages of a chapter as PGM.
        let book = args.get(2).map(String::as_str).unwrap_or_else(|| usage());
        let chapter: usize = args
            .get(3)
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| usage());
        let out = args.get(4).map(String::as_str).unwrap_or_else(|| usage());
        run_render(Path::new(book), chapter, out);
    } else if args.get(1).map(String::as_str) == Some("--host") {
        let books = args.get(2).map(String::as_str).unwrap_or_else(|| usage());
        let out = args.get(3).map(String::as_str).unwrap_or_else(|| usage());
        run_host(Path::new(books), out);
    } else if args.len() > 1 {
        usage();
    } else {
        run_kobo();
    }
}

/// Renders the pages of a chapter (reader configuration) to PGM, to
/// compare them with another rendering engine.
fn run_render(book_path: &Path, chapter: usize, out: &str) {
    use reader::{paginate, PageConfig};
    let mut ts = typesetter();
    ts.set_ink(1.4, 0.25);
    let cfg = PageConfig {
        px: theme::READER_SIZES[1],
        line_height: theme::READER_LEADINGS[1],
        margin_x: theme::READER_MARGINS[1],
        margin_top: theme::STATUS_H + 28,
        margin_bottom: 64,
        screen_w: theme::SCREEN_W as i32,
        screen_h: theme::SCREEN_H as i32,
        family: 0,
    };
    let mut book = signet_epub::Book::open(book_path).expect("ouverture du livre");
    let blocks = book.chapter(chapter).expect("chapitre");
    let mut images = reader::BookImages::default();
    let pages = {
        let book = &mut book;
        let images = &mut images;
        let mut image = |href: &str| images.size(book, href, &cfg);
        let first = paginate(&blocks, &ts, &cfg, &mut image);
        // Profiling: `SIGNET_REPEAT=N` re-lays out the chapter N times, images cached,
        // like the app when it comes back to it.
        let repeat: usize = std::env::var("SIGNET_REPEAT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if repeat > 0 {
            let t0 = std::time::Instant::now();
            for _ in 0..repeat {
                std::hint::black_box(paginate(&blocks, &ts, &cfg, &mut image));
            }
            eprintln!(
                "composition : {:.2} ms en moyenne sur {repeat}",
                t0.elapsed().as_secs_f64() * 1000.0 / repeat as f64
            );
        }
        first
    };
    std::fs::create_dir_all(out).expect("dossier de sortie");
    let mut canvas = signet_fb::Canvas::new(theme::SCREEN_W, theme::SCREEN_H);
    for (pi, page) in pages.iter().enumerate() {
        reader::resolve_images(page, &mut |href, w, h| images.pixels(&mut book, href, w, h));
        canvas.clear(0xFF);
        let status = ui::Status {
            title: &book.metadata.title,
            clock: "12:00",
            battery: Some(80),
            charging: false,
        };
        let label = format!("{} / {}", pi + 1, pages.len());
        ui::reader(
            &mut canvas,
            &mut ts,
            &status,
            page,
            &cfg,
            &ui::Chrome {
                label: &label,
                ..Default::default()
            },
            &[],
            None,
        );
        let path = format!("{out}/{:02}.pgm", pi + 1);
        canvas.save_pgm(&path).expect("écriture");
    }
    println!("{} blocs, {} pages → {out}", blocks.len(), pages.len());
}

/// Measurement bench: lays out and renders a whole book at the reader's
/// configuration, and quantifies the typography. Options: `--greedy` (greedy
/// line breaking), `--no-hint`, `--size N`, `--csv`.
fn run_bench(book_path: &Path, opts: &[String]) {
    use reader::{paginate, ItemKind, PageConfig};
    let mut ts = typesetter();
    let greedy = opts.iter().any(|o| o == "--greedy");
    let no_hint = opts.iter().any(|o| o == "--no-hint");
    let csv = opts.iter().any(|o| o == "--csv");
    let size = opts
        .iter()
        .position(|o| o == "--size")
        .and_then(|i| opts.get(i + 1))
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .min(2);
    ts.knuth_plass.set(!greedy);
    if let Some(v) = std::env::var("SIGNET_KP_TOL")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_tolerance.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_KP_HYPHEN")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_hyphen_penalty.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_KP_SHORT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_short_head_penalty.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_KP_RUN")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_max_hyphen_run.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_KP_ADJ")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_adj_demerits.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_KP_SHRINK")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.kp_shrink.set(v);
    }
    if let Some(v) = std::env::var("SIGNET_SPACE")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        ts.space_scale.set(v);
    }
    ts.set_hinting(!no_hint);
    ts.set_ink(1.4, 0.25);
    let cfg = PageConfig {
        px: theme::READER_SIZES[size],
        line_height: theme::READER_LEADINGS[1],
        margin_x: theme::READER_MARGINS[1],
        margin_top: theme::STATUS_H + 28,
        margin_bottom: 64,
        screen_w: theme::SCREEN_W as i32,
        screen_h: theme::SCREEN_H as i32,
        family: 0,
    };
    let mut book = signet_epub::Book::open(book_path).expect("ouverture du livre");
    let mut canvas = signet_fb::Canvas::new(theme::SCREEN_W, theme::SCREEN_H);
    let (mut pages, mut lines, mut just, mut hyph, mut hyph_pairs, mut loose, mut tight, mut over2) = (
        0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize,
    );
    // Runs of three or more consecutive hyphenated lines (Apple Books: -webkit-hyphenate-limit-lines: 2).
    let mut hyph_triples = 0usize;
    // Breaks less than three letters from an edge (Apple Books: limit-before 3, limit-after 3).
    let mut short_cuts = 0usize;
    // Widows and orphans: paragraph split leaving a single line on one side (Apple Books: widows 2, orphans 2).
    let mut lonely = 0usize;
    let mut stranded_headings = 0usize;
    // Glyphs missing from the font (id 0, drawn as a square).
    let mut notdef = 0usize;
    let mut ratios: Vec<f32> = Vec::new();
    let (mut t_parse, mut t_layout, mut t_render) = (
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
        std::time::Duration::ZERO,
    );
    let mut images = reader::BookImages::default();
    let mut rendered = 0usize;
    let mut worst: Vec<(f32, usize, String)> = Vec::new();
    if csv {
        println!("chapitre,blocs,pages,lignes,justifiees,ratio_moyen,ratio_max,cesures,parse_ms,layout_ms,rendu_ms_par_page");
    }
    for ci in 0..book.spine.len() {
        let t = std::time::Instant::now();
        let blocks = match book.chapter(ci) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("chapitre {ci} : {e:?}");
                continue;
            }
        };
        let parse = t.elapsed();
        let t = std::time::Instant::now();
        let chapter_pages = {
            let book = &mut book;
            let images = &mut images;
            let mut image = |href: &str| images.size(book, href, &cfg);
            paginate(&blocks, &ts, &cfg, &mut image)
        };
        let layout = t.elapsed();
        let (mut c_lines, mut c_just, mut c_hyph, mut c_sum, mut c_max) =
            (0usize, 0usize, 0usize, 0.0f32, 0.0f32);
        // Orphaned heading: a page that ends with a heading (the text that follows is on the next page).
        for page in chapter_pages
            .iter()
            .take(chapter_pages.len().saturating_sub(1))
        {
            if let Some(last) = page.items.last() {
                if matches!(
                    blocks.get(last.block),
                    Some(signet_epub::Block::Heading { .. })
                ) {
                    stranded_headings += 1;
                    if std::env::var_os("SIGNET_BENCH_DEBUG").is_some() {
                        let next = blocks
                            .get(last.block + 1)
                            .map(|b| format!("{:?}", std::mem::discriminant(b)))
                            .unwrap_or_default();
                        eprintln!(
                            "titre en bas de page, ch. {ci} : « {} » ; bloc suivant {next} « {} »",
                            blocks[last.block].text(),
                            blocks
                                .get(last.block + 1)
                                .map(|b| b.text().chars().take(40).collect::<String>())
                                .unwrap_or_default()
                        );
                    }
                }
            }
        }
        // Widows / orphans: a block of text spread over two pages with a single line on one side.
        for (pi, page) in chapter_pages.iter().enumerate() {
            if let (Some(last), Some(next)) = (
                page.items.last(),
                chapter_pages.get(pi + 1).and_then(|p| p.items.first()),
            ) {
                if matches!(last.kind, ItemKind::Text)
                    && last.block == next.block
                    && (last.lines.len() == 1 || next.lines.len() == 1)
                {
                    lonely += 1;
                }
            }
        }
        // Short breaks: the head or tail of a hyphenated word is under three letters.
        for page in &chapter_pages {
            for item in &page.items {
                if !matches!(item.kind, ItemKind::Text) {
                    continue;
                }
                let chars: Vec<char> = blocks[item.block].text().chars().collect();
                for l in item.lines.iter().filter(|l| l.hyphenated) {
                    // The final hyphen carries the index of the first character of the tail.
                    let Some(first_tail) = l.glyphs.last().map(|g| g.ch as usize) else {
                        continue;
                    };
                    if first_tail == 0 || first_tail >= chars.len() {
                        continue;
                    }
                    let end = first_tail - 1;
                    // Hyphenated word: from the preceding whitespace to the following; we count the letters
                    // and apostrophes (as WebKit counts the word's characters), not the punctuation.
                    let is_word = |c: &char| c.is_alphanumeric() || *c == '’' || *c == '\'';
                    let mut a = end;
                    while a > 0 && !chars[a - 1].is_whitespace() && chars[a - 1] != '-' {
                        a -= 1;
                    }
                    let mut b = first_tail;
                    while b < chars.len() && !chars[b].is_whitespace() && chars[b] != '-' {
                        b += 1;
                    }
                    let head = chars[a..=end].iter().filter(|c| is_word(c)).count();
                    let tail = chars[first_tail..b].iter().filter(|c| is_word(c)).count();
                    if head < 3 || tail < 3 {
                        short_cuts += 1;
                    }
                }
            }
        }
        let mut prev_hyph = false;
        let mut run = 0usize;
        for page in &chapter_pages {
            for item in &page.items {
                if !matches!(item.kind, ItemKind::Text) {
                    continue;
                }
                for l in &item.lines {
                    c_lines += 1;
                    notdef += l.glyphs.iter().filter(|g| g.id == 0).count();
                    if l.hyphenated {
                        c_hyph += 1;
                        if prev_hyph {
                            hyph_pairs += 1;
                        }
                        run += 1;
                        if run == 3 {
                            hyph_triples += 1;
                        }
                    } else {
                        run = 0;
                    }
                    prev_hyph = l.hyphenated;
                    if l.ratio.is_finite() {
                        c_just += 1;
                        let a = l.ratio.abs();
                        c_sum += a;
                        c_max = c_max.max(a);
                        ratios.push(l.ratio);
                        if l.ratio > 1.0 {
                            loose += 1;
                        }
                        if l.ratio < -0.5 {
                            tight += 1;
                        }
                        if l.ratio > 2.0 {
                            over2 += 1;
                        }
                        if l.ratio > 1.5 {
                            let chars: Vec<char> = blocks[item.block].text().chars().collect();
                            let (a, b) = l
                                .glyphs
                                .iter()
                                .fold((u32::MAX, 0u32), |(a, b), g| (a.min(g.ch), b.max(g.ch)));
                            let text: String = chars
                                .get(a as usize..=(b as usize).min(chars.len().saturating_sub(1)))
                                .unwrap_or(&[])
                                .iter()
                                .collect();
                            worst.push((l.ratio, ci, text));
                        }
                    }
                }
            }
        }
        // Rendering: each page as on the reader (status bar, page number).
        let t = std::time::Instant::now();
        for (pi, page) in chapter_pages.iter().enumerate() {
            reader::resolve_images(page, &mut |href, w, h| images.pixels(&mut book, href, w, h));
            let status = ui::Status {
                title: &book.metadata.title,
                clock: "12:00",
                battery: Some(80),
                charging: false,
            };
            let label = format!("{} / {}", pi + 1, chapter_pages.len());
            ui::reader(
                &mut canvas,
                &mut ts,
                &status,
                page,
                &cfg,
                &ui::Chrome {
                    label: &label,
                    ..Default::default()
                },
                &[],
                None,
            );
            rendered += 1;
        }
        let render = t.elapsed();
        if csv {
            println!(
                "{ci},{},{},{c_lines},{c_just},{:.3},{:.2},{c_hyph},{:.1},{:.1},{:.2}",
                blocks.len(),
                chapter_pages.len(),
                if c_just > 0 {
                    c_sum / c_just as f32
                } else {
                    0.0
                },
                c_max,
                parse.as_secs_f64() * 1e3,
                layout.as_secs_f64() * 1e3,
                render.as_secs_f64() * 1e3 / chapter_pages.len().max(1) as f64
            );
        }
        pages += chapter_pages.len();
        lines += c_lines;
        just += c_just;
        hyph += c_hyph;
        t_parse += parse;
        t_layout += layout;
        t_render += render;
    }
    // Actual width of spaces, in ems: space × (1 + r/2) stretched, × (1 + r/3) compressed.
    let space_em =
        ts.font(signet_text::Style::Regular).measure(" ", cfg.px) * ts.space_scale.get() / cfg.px;
    let mut gaps: Vec<f32> = ratios
        .iter()
        .map(|&r| {
            space_em
                * if r >= 0.0 {
                    1.0 + r / 2.0
                } else {
                    1.0 + r / 3.0
                }
        })
        .collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let gp = |p: f64| -> f32 {
        if gaps.is_empty() {
            0.0
        } else {
            gaps[((gaps.len() - 1) as f64 * p) as usize]
        }
    };
    let wide = gaps.iter().filter(|&&g| g > 0.5).count();
    let narrow = gaps.iter().filter(|&&g| g < 0.16).count();
    let very_wide = gaps.iter().filter(|&&g| g > 0.75).count();
    println!("espaces (cadratins) : nominal {space_em:.2} | médiane {:.2} | p90 {:.2} | p99 {:.2} | max {:.2} | > 0,5 em {wide} ({:.1} %) | > 0,75 em {very_wide} ({:.1} %) | < 0,16 em {narrow} ({:.1} %) | min {:.2}", gp(0.5), gp(0.9), gp(0.99), gp(1.0), 100.0 * wide as f32 / gaps.len().max(1) as f32, 100.0 * very_wide as f32 / gaps.len().max(1) as f32, 100.0 * narrow as f32 / gaps.len().max(1) as f32, gp(0.0));
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| -> f32 {
        if ratios.is_empty() {
            0.0
        } else {
            ratios[((ratios.len() - 1) as f64 * p) as usize]
        }
    };
    let mean_abs: f32 = if ratios.is_empty() {
        0.0
    } else {
        ratios.iter().map(|r| r.abs()).sum::<f32>() / ratios.len() as f32
    };
    println!(
        "configuration : {} | hinting {} | taille {} px | colonne {} px",
        if greedy { "gloutonne" } else { "Knuth-Plass" },
        if no_hint { "non" } else { "oui" },
        cfg.px,
        cfg.text_width()
    );
    println!("chapitres {} | pages {pages} | lignes {lines} | justifiées {just} | césures {hyph} ({:.1} %) | césures consécutives {hyph_pairs}", book.spine.len(), 100.0 * hyph as f32 / lines.max(1) as f32);
    println!("ratio |r| moyen {mean_abs:.3} | médian {:.3} | p90 {:.3} | p99 {:.3} | max {:.2} | min {:.2}", pct(0.5).abs(), pct(0.9), pct(0.99), pct(1.0), pct(0.0));
    println!("lignes lâches (r > 1) {loose} ({:.2} %) | très lâches (r > 2) {over2} | serrées (r < −0,5) {tight}", 100.0 * loose as f32 / just.max(1) as f32);
    println!("règles d'Apple Books : 3 lignes césurées de suite {hyph_triples} | coupes à moins de 3 lettres {short_cuts} | veuves et orphelines {lonely} | titres en bas de page {stranded_headings} | glyphes manquants {notdef}");
    println!("temps : parse {:.0} ms | composition {:.0} ms ({:.1} ms/chapitre) | rendu {:.0} ms ({:.1} ms/page sur {rendered})", t_parse.as_secs_f64() * 1e3, t_layout.as_secs_f64() * 1e3, t_layout.as_secs_f64() * 1e3 / book.spine.len().max(1) as f64, t_render.as_secs_f64() * 1e3, t_render.as_secs_f64() * 1e3 / rendered.max(1) as f64);
    println!(
        "cache de glyphes {} | façonnage {} succès / {} échecs",
        ts.cache.len(),
        ts.shape_hits.get(),
        ts.shape_misses.get()
    );
    println!("images : en-têtes {:.0} ms (dans la composition), décodage et mise à l'échelle {:.0} ms (dans le rendu)", images.t_sizes.as_secs_f64() * 1e3, images.t_decode.as_secs_f64() * 1e3);
    println!("profil : façonnage hors cache {:.0} ms ({} mots, {:.0} µs/mot) | césure hors cache {:.0} ms ({} mots, {:.0} µs/mot) | Knuth-Plass {:.0} ms", ts.t_shape.get().as_secs_f64() * 1e3, ts.shape_misses.get(), ts.t_shape.get().as_secs_f64() * 1e6 / ts.shape_misses.get().max(1) as f64, ts.t_hyph.get().as_secs_f64() * 1e3, ts.hyph_misses.get(), ts.t_hyph.get().as_secs_f64() * 1e6 / ts.hyph_misses.get().max(1) as f64, ts.t_kp.get().as_secs_f64() * 1e3);
    let passes = ts.kp_passes.get();
    println!(
        "paragraphes : Knuth-Plass tol. 2 {} | tol. 5 {} | repli glouton {}",
        passes[0], passes[1], passes[2]
    );
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (r, ci, text) in worst.iter().take(10) {
        println!("  r = {r:5.2}  ch. {ci:2}  « {text} »");
    }
}

fn run_host(books: &Path, out: &str) {
    let t0 = std::time::Instant::now();
    let ts = typesetter();
    let screen = HostScreen::new(out, theme::SCREEN_W, theme::SCREEN_H);
    // Scenario: open the first book, turn 3 pages, open the settings,
    // enlarge the text, close, return to the home screen.
    let mut ev = Vec::new();
    let w = theme::SCREEN_W as i32;
    let h = theme::SCREEN_H as i32;
    ev.extend(ScriptedInput::tap(w / 2, 589)); // first library row
    for _ in 0..3 {
        ev.extend(ScriptedInput::tap(w - 60, h / 2)); // next page
    }
    ev.extend(ScriptedInput::tap(w / 2, h / 2)); // settings
    ev.extend(ScriptedInput::tap(w - 60, h - 692 + 110 + 5 * 78 + 54)); // quick pages: yes
    ev.extend(ScriptedInput::tap(w / 2, 200)); // close
    ev.extend(ScriptedInput::tap(w / 2, h / 2)); // settings
    ev.extend(ScriptedInput::tap(100, h - 40)); // Table of contents
    ev.extend(ScriptedInput::tap(w / 2, 64 + 90 + 72 * 6 + 30)); // 7th entry
    ev.extend(ScriptedInput::tap(w / 2, 20)); // home
    ev.extend(ScriptedInput::tap(w - 120, 333)); // Resume reading
    ev.extend(ScriptedInput::tap(w / 2, 20)); // home (final capture with a book in progress)
    ev.extend(ScriptedInput::tap(w - 60, 489 + 12)); // See all
    ev.extend(ScriptedInput::tap(100, 20)); // back to home
                                            // Annotations: open, long press on a word, extend, highlight, then the list.
    ev.extend(ScriptedInput::tap(w - 120, 333)); // Resume
    ev.extend(ScriptedInput::long_press(200, 335)); // long press on a word (1st page of text)
    ev.extend(ScriptedInput::tap(500, 290)); // extend to the word on the previous line (same paragraph)
    ev.extend(ScriptedInput::tap(w / 2, h - 40)); // Definition
    ev.extend(ScriptedInput::tap(w / 2, 100)); // close the definition
    ev.extend(ScriptedInput::drag(200, 335, 500, 290)); // press and hold + drag
    ev.extend(ScriptedInput::tap(150, h - 40)); // Highlight
                                                // Dead time: pagination of the whole book happens chapter by chapter.
    for _ in 0..80 {
        ev.push(backend::Step::Wait(std::time::Duration::from_millis(1)));
    }
    ev.extend(ScriptedInput::tap(w / 2, h / 2)); // settings
    ev.extend(ScriptedInput::tap(242, h - 54)); // Annotations
    ev.extend(ScriptedInput::tap(w / 2, 20)); // back (status)
    ev.extend(ScriptedInput::tap(w / 2, 20)); // home
    let input = ScriptedInput::new(ev);
    let state_dir = Path::new(out).join("state");
    let mut app = app::App::new(screen, input, ts, books, &state_dir);
    app.run();
    eprintln!("terminé en {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
}

fn run_kobo() {
    let t0 = std::time::Instant::now();
    let ts = typesetter();
    let screen = match KoboScreen::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("framebuffer : {e}");
            std::process::exit(1);
        }
    };
    eprintln!("fb : {}", screen.describe());
    let (w, h) = screen.size();
    let input = KoboInput::open(signet_input::Transform::identity(w as i32, h as i32));
    eprintln!("entrées : {}", input.describe());
    let raw = input.raw_extent().unwrap_or((w as i32, h as i32));
    eprintln!("étendue tactile brute : {raw:?}");
    let state_dir = Path::new("/mnt/onboard/.signet");
    let mut app = app::App::new(screen, input, ts, Path::new("/mnt/onboard"), state_dir);
    let transform = match app.touch_transform() {
        Some(t) => t,
        None => {
            app.input_mut()
                .set_transform(signet_input::Transform::identity(raw.0, raw.1));
            match app.calibrate(raw) {
                Some(t) => t,
                None => {
                    eprintln!("calibration abandonnée");
                    return;
                }
            }
        }
    };
    eprintln!("transformation tactile : {transform:?}");
    app.input_mut().set_transform(transform);
    device::set_power_led(device::power_online());
    device::install_term_handler();
    eprintln!("prêt en {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
    app.run();
    eprintln!("Signet quitte (code {})", app.exit_code);
    std::process::exit(app.exit_code);
}
