//! Highlights and bookmarks: anchored in the text (chapter, block, character
//! range), hence insensitive to font size and margins. One text
//! file per book in `.signet/annotations/`, one line per
//! annotation. A bookmark is an empty range: the start of the marked page.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub chapter: usize,
    pub block: usize,
    /// Character range [from, to) in the block's text.
    pub from: u32,
    pub to: u32,
    /// Excerpt, for the list (truncated).
    pub text: String,
}

impl Annotation {
    pub fn is_bookmark(&self) -> bool {
        self.from == self.to
    }
}

pub struct Annotations {
    path: PathBuf,
    pub items: Vec<Annotation>,
}

impl Annotations {
    pub fn load(dir: &Path, book_key: &str) -> Annotations {
        let path = dir.join(format!("{book_key}.txt"));
        let mut items = Vec::new();
        if let Ok(txt) = fs::read_to_string(&path) {
            for line in txt.lines() {
                let f: Vec<&str> = line.splitn(5, '\t').collect();
                if f.len() == 5 {
                    if let (Ok(c), Ok(b), Ok(from), Ok(to)) =
                        (f[0].parse(), f[1].parse(), f[2].parse(), f[3].parse())
                    {
                        items.push(Annotation {
                            chapter: c,
                            block: b,
                            from,
                            to,
                            text: f[4].to_string(),
                        });
                    }
                }
            }
        }
        Annotations { path, items }
    }

    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let mut s = String::new();
        for a in &self.items {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                a.chapter,
                a.block,
                a.from,
                a.to,
                a.text.replace(['\t', '\n'], " ")
            ));
        }
        let tmp = self.path.with_extension("tmp");
        crate::device::with_rw(|| {
            if fs::write(&tmp, &s).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        });
    }

    pub fn add(&mut self, a: Annotation) {
        // Merge with an existing highlight that overlaps in the same block.
        if let Some(e) = self.items.iter_mut().find(|e| {
            !a.is_bookmark()
                && !e.is_bookmark()
                && e.chapter == a.chapter
                && e.block == a.block
                && e.from <= a.to
                && a.from <= e.to
        }) {
            e.from = e.from.min(a.from);
            e.to = e.to.max(a.to);
            e.text = a.text;
        } else {
            self.items.push(a);
            self.items.sort_by_key(|a| (a.chapter, a.block, a.from));
        }
        self.save();
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.save();
        }
    }

    /// Highlights of a chapter: (block, from, to, index).
    pub fn in_chapter(&self, chapter: usize) -> Vec<(usize, u32, u32, usize)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, a)| a.chapter == chapter && !a.is_bookmark())
            .map(|(i, a)| (a.block, a.from, a.to, i))
            .collect()
    }

    /// Bookmarks of the chapter placed in [start, end): their indices.
    pub fn bookmarks_between(
        &self,
        chapter: usize,
        start: (usize, u32),
        end: Option<(usize, u32)>,
    ) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                a.is_bookmark()
                    && a.chapter == chapter
                    && (a.block, a.from) >= start
                    && end.map(|e| (a.block, a.from) < e).unwrap_or(true)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Removes several annotations at once (a single write).
    pub fn remove_all(&mut self, mut indexes: Vec<usize>) {
        indexes.sort_unstable();
        for i in indexes.into_iter().rev() {
            if i < self.items.len() {
                self.items.remove(i);
            }
        }
        self.save();
    }
}
