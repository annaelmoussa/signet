//! The library: the EPUBs on the partition, their metadata,
//! their covers (cached as PGM) and progress.

use crate::draw::{decode_image_for, Gray};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct BookEntry {
    pub path: PathBuf,
    pub title: String,
    pub author: String,
    pub chapters: usize,
    /// Progress 0–100 (computed from the saved position).
    pub percent: u8,
    /// Page read and page count of the book, when the book is counted for the
    /// current reading configuration.
    pub pages: Option<(usize, usize)>,
    /// Reading started (a position beyond the first page).
    pub started: bool,
    pub finished: bool,
    pub cover: Option<Gray>,
    /// Modification date, to sort by "recent".
    pub mtime: u64,
}

pub struct Library {
    pub books: Vec<BookEntry>,
}

impl Library {
    /// Walks `root` (two levels) looking for `.epub`.
    pub fn scan(root: &Path, cache_dir: &Path, cover_w: u32, cover_h: u32) -> Library {
        let mut files = Vec::new();
        collect(root, 0, &mut files);
        let _ = fs::create_dir_all(cache_dir);
        let mut books = Vec::new();
        for path in files {
            let mtime = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let key = cache_key(&path);
            let cover_path = cache_dir.join(format!("{key}-{cover_w}x{cover_h}.pgm"));
            // Cached metadata (date, title, author, chapters): avoids
            // opening every EPUB at each startup.
            let meta_path = cache_dir.join(format!("{key}.meta"));
            if let Some((title, author, chapters)) = read_meta(&meta_path, mtime) {
                let cover = fs::read(&cover_path).ok().and_then(|b| Gray::from_pgm(&b));
                books.push(BookEntry {
                    path,
                    title,
                    author,
                    chapters,
                    percent: 0,
                    pages: None,
                    started: false,
                    finished: false,
                    cover,
                    mtime,
                });
                continue;
            }
            let Ok(mut book) = signet_epub::Book::open(&path) else {
                continue;
            };
            let cover = match fs::read(&cover_path).ok().and_then(|b| Gray::from_pgm(&b)) {
                Some(g) => Some(g),
                None => {
                    let g = book
                        .cover_href
                        .clone()
                        .and_then(|h| book.read_bytes(&h).ok())
                        .and_then(|b| decode_image_for(&b, cover_w, cover_h))
                        .map(|g| g.fit(cover_w, cover_h));
                    if let Some(g) = &g {
                        let _ = crate::device::with_rw(|| fs::write(&cover_path, g.to_pgm()));
                    }
                    g
                }
            };
            let title = if book.metadata.title.is_empty() {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                book.metadata.title.clone()
            };
            let meta = format!(
                "{mtime}\t{}\t{}\t{}\n",
                book.spine.len(),
                title.replace(['\t', '\n'], " "),
                book.metadata.author.replace(['\t', '\n'], " ")
            );
            let _ = crate::device::with_rw(|| fs::write(&meta_path, meta));
            books.push(BookEntry {
                path,
                title,
                author: book.metadata.author.clone(),
                chapters: book.spine.len(),
                percent: 0,
                pages: None,
                started: false,
                finished: false,
                cover,
                mtime,
            });
        }
        books.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.title.cmp(&b.title)));
        Library { books }
    }
}

/// Reads the metadata cache if it matches the file's date.
fn read_meta(path: &Path, mtime: u64) -> Option<(String, String, usize)> {
    let txt = fs::read_to_string(path).ok()?;
    let f: Vec<&str> = txt.trim_end_matches('\n').splitn(4, '\t').collect();
    if f.len() != 4 || f[0].parse::<u64>().ok()? != mtime {
        return None;
    }
    Some((f[2].to_string(), f[3].to_string(), f[1].parse().ok()?))
}

fn collect(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with("._") {
            continue;
        }
        if p.is_dir() {
            if depth < 2 && name != "System Volume Information" {
                collect(&p, depth + 1, out);
            }
        } else if name.to_ascii_lowercase().ends_with(".epub") {
            out.push(p);
        }
    }
}

/// Short, stable identifier for a path (64-bit FNV-1a).
pub fn cache_key(path: &Path) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.to_string_lossy().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}
