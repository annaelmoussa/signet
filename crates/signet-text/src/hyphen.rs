//! Hyphenation with Liang's algorithm (the one from TeX), from the
//! `hyph-*.tex` patterns of the hyph-utf8 project.
//!
//! Principle: each pattern is a sequence of letters interspersed with digits
//! (`.anti1a2`, `2'2`, `1a2nesthési`). For a word surrounded by dots
//! (`.word.`), we look for all the patterns that appear as a substring
//! and keep, between each pair of letters, the **maximum** digit
//! encountered. Positions with an odd value are allowed hyphenation points.
//! That's all: no dictionary, 1,700 compact patterns, and correct breaks
//! in the vast majority of cases.

use std::collections::HashMap;

pub struct Hyphenator {
    /// Pattern letters (without digits) → levels (len = letters + 1).
    patterns: HashMap<Vec<char>, Vec<u8>>,
    max_len: usize,
    pub left_min: usize,
    pub right_min: usize,
}

impl Hyphenator {
    /// Loads a `\patterns{ … }` file in TeX format (UTF-8).
    pub fn from_tex(src: &str) -> Hyphenator {
        let mut patterns = HashMap::new();
        let mut max_len = 0;
        let mut in_patterns = false;
        for raw in src.lines() {
            let line = raw.split('%').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("\\patterns") {
                in_patterns = true;
                continue;
            }
            if line.starts_with('}') || line.starts_with("\\hyphenation") {
                in_patterns = false;
                continue;
            }
            if !in_patterns {
                continue;
            }
            for tok in line.split_whitespace() {
                let mut letters = Vec::new();
                let mut levels = vec![0u8];
                for ch in tok.chars() {
                    if let Some(d) = ch.to_digit(10) {
                        *levels.last_mut().unwrap() = d as u8;
                    } else {
                        letters.push(ch);
                        levels.push(0);
                    }
                }
                if letters.is_empty() {
                    continue;
                }
                max_len = max_len.max(letters.len());
                patterns.insert(letters, levels);
            }
        }
        Hyphenator {
            patterns,
            max_len,
            left_min: 2,
            right_min: 3,
        }
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Positions (in number of characters from the start of `word`) where a
    /// hyphenation is allowed. Does not break words containing digits or
    /// punctuation other than the apostrophe; an existing hyphen is a break.
    pub fn hyphenate(&self, word: &str) -> Vec<usize> {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() < self.left_min + self.right_min {
            return Vec::new();
        }
        if chars
            .iter()
            .any(|c| !(c.is_alphabetic() || *c == '\'' || *c == '’' || *c == '-'))
        {
            return Vec::new();
        }
        if chars.contains(&'-') {
            // Compound word: break after each hyphen, and hyphenate each part
            // (« chas-seur-cueil-leur »), positions shifted within the whole word.
            let mut out = Vec::new();
            let mut start = 0usize;
            let parts: Vec<String> = word.split('-').map(str::to_string).collect();
            for (i, part) in parts.iter().enumerate() {
                let n = part.chars().count();
                if i > 0 && start >= 1 && start < chars.len() {
                    out.push(start);
                }
                if n >= self.left_min + self.right_min {
                    out.extend(self.hyphenate(part).into_iter().map(|p| p + start));
                }
                start += n + 1;
            }
            out.sort_unstable();
            out.dedup();
            return out;
        }
        // lowercased word, typographic apostrophe normalized, '.' boundaries
        let mut w: Vec<char> = Vec::with_capacity(chars.len() + 2);
        w.push('.');
        w.extend(chars.iter().map(|c| {
            if *c == '’' {
                '\''
            } else {
                c.to_lowercase().next().unwrap_or(*c)
            }
        }));
        w.push('.');
        let mut levels = vec![0u8; w.len() + 1];
        for start in 0..w.len() {
            let end_max = (start + self.max_len).min(w.len());
            for end in start + 1..=end_max {
                if let Some(lv) = self.patterns.get(&w[start..end]) {
                    for (k, v) in lv.iter().enumerate() {
                        let idx = start + k;
                        if *v > levels[idx] {
                            levels[idx] = *v;
                        }
                    }
                }
            }
        }
        // levels[i] refers to the position BEFORE w[i]; w[0] = '.', so
        // the break before letter n (1-based in the word) is levels[n].
        let n = chars.len();
        (self.left_min..=n - self.right_min)
            .filter(|&i| levels[i + 1] % 2 == 1)
            .collect()
    }

    /// The word with visible soft hyphens, for tests.
    pub fn show(&self, word: &str) -> String {
        let pts = self.hyphenate(word);
        let mut out = String::new();
        for (i, ch) in word.chars().enumerate() {
            if pts.contains(&i) {
                out.push('-');
            }
            out.push(ch);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fr() -> Hyphenator {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/hyphenation/hyph-fr.tex"
        ))
        .unwrap();
        Hyphenator::from_tex(&src)
    }

    #[test]
    fn loads_french_patterns() {
        let h = fr();
        assert!(h.len() > 1000, "{} patrons", h.len());
    }

    #[test]
    fn french_words() {
        let h = fr();
        assert_eq!(h.show("respiration"), "res-pi-ra-tion");
        assert_eq!(h.show("silencieuse"), "si-len-cieuse");
        assert_eq!(h.show("exactement"), "exac-te-ment");
        assert_eq!(h.show("constitution"), "consti-tu-tion");
        assert_eq!(h.show("Automne"), "Au-tomne");
        assert_eq!(h.show("bibliothèque"), "bi-blio-thèque");
        assert_eq!(h.show("aujourd'hui"), "au-jour-d'hui");
        assert_eq!(h.show("tue"), "tue");
        assert_eq!(h.show("peut-être"), "peut--être");
        assert!(h.hyphenate("2024").is_empty());
    }
}
