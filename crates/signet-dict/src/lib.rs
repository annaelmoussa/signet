//! `signet-dict` — Kobo's dictionaries, without Nickel.
//!
//! A `dicthtml-xx.zip` contains one `PREFIX.html` file per two-letter prefix
//! (itself gzip-compressed), plus a `words` index. Each HTML file is a
//! sequence of entries `<w><p><a name="word"/><b>word</b> … definition
//! …</p></w>`. We compute the prefix of the word being looked up, decompress
//! the matching file (cached), find the entry in it, and reduce its HTML to
//! readable text.

pub mod rich;
pub use rich::{Para, RichEntry, Run};

use std::fs::File;
use std::io::Read;
use std::path::Path;
use zip::ZipArchive;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub headword: String,
    /// Definition text, lines separated by `\n`, bullets "•".
    pub text: String,
}

pub struct Dictionary {
    archive: ZipArchive<File>,
    /// File name (UTF-8) → index in the archive. Kobo writes names in UTF-8
    /// without the flag that announces it: the zip library reads them as
    /// CP437, and "éc.html" became unfindable (no word starting with é, ê, â…).
    names: std::collections::HashMap<String, usize>,
    /// Last decompressed prefix file: (prefix, contents).
    cache: Option<(String, String)>,
    /// Entries of `11.html`, kept: numbered homographs ("1aide", "2aide")
    /// and words with an apostrophe ("d'abord") are all filed there, since
    /// their prefix starts with a character that is not a letter.
    special: Option<Vec<(String, Vec<String>, String)>>,
    /// Dictionary word list (`words.original`), for near-miss suggestions.
    words: Option<Vec<(String, String)>>,
}

/// File prefix of a word, following Kobo's algorithm: first two characters
/// lowercased, whitespace removed, padded with "a", "11" if either of the
/// two is not a letter; Cyrillic is returned as is.
pub fn prefix(word: &str) -> String {
    let word = word.split('\0').next().unwrap_or("");
    let two: String = word
        .chars()
        .take(2)
        .flat_map(|c| c.to_lowercase())
        .collect();
    let trimmed = two.trim();
    if trimmed.is_empty() {
        return "11".into();
    }
    let first = trimmed.chars().next().unwrap();
    if ('\u{0400}'..='\u{04FF}').contains(&first) {
        return trimmed.to_string();
    }
    let mut chars: Vec<char> = trimmed.chars().collect();
    while chars.len() < 2 {
        chars.push('a');
    }
    if !chars[0].is_alphabetic() || !chars[1].is_alphabetic() {
        return "11".into();
    }
    chars.iter().take(2).collect()
}

/// Comparison key for an entry name: without the leading homograph number
/// ("2aide" → "aide"), without the feminine form ("1adhérent, ente").
fn name_key(name: &str) -> String {
    let n = name.trim_start_matches(|c: char| c.is_ascii_digit());
    n.split(',').next().unwrap_or("").trim().to_lowercase()
}

/// Forms listed by the bold headword: "lent, lente" → lent, lente;
/// "adhérent, ente" → adhérent, adhérente; "beau ou bel, belle".
fn headword_forms(body: &str) -> Vec<String> {
    let Some(i) = body.find("<b>") else {
        return Vec::new();
    };
    let rest = &body[i + 3..];
    let Some(j) = rest.find("</b>") else {
        return Vec::new();
    };
    let bold = rest[..j].to_lowercase();
    let parts: Vec<&str> = bold
        .split(',')
        .flat_map(|p| p.split(" ou "))
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    let Some(base) = parts.first().copied() else {
        return Vec::new();
    };
    let mut out = vec![base.to_string()];
    for p in &parts[1..] {
        let first = p.chars().next().unwrap_or(' ');
        // Full form, or an ending to graft onto the word ("ente" → "adhérente").
        match base.rfind(first) {
            Some(k) if p.chars().count() < base.chars().count() => {
                out.push(format!("{}{p}", &base[..k]))
            }
            _ => out.push(p.to_string()),
        }
    }
    out
}

/// Word as it is looked up: lowercase, straight apostrophe, punctuation stripped.
pub fn normalize(word: &str) -> String {
    word.trim()
        .replace(['’', 'ʼ', '‘'], "'")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'')
        .to_lowercase()
}

/// Form without the article or elided pronoun: "l'école" → "école", "qu'il" → "il".
pub fn unelide(word: &str) -> Option<String> {
    const ELIDED: [&str; 13] = [
        "jusqu'", "lorsqu'", "puisqu'", "quoiqu'", "qu'", "l'", "d'", "j'", "m'", "n'", "s'", "t'",
        "c'",
    ];
    ELIDED
        .iter()
        .find_map(|p| word.strip_prefix(p))
        .filter(|r| r.chars().count() >= 2)
        .map(String::from)
}

/// Letters without accents, for comparing near-miss words.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ÿ' => 'y',
            c => c,
        })
        .collect()
}

/// Edit distance (insertion, deletion, substitution), capped.
fn distance(a: &[char], b: &[char], cap: usize) -> usize {
    if a.len().abs_diff(b.len()) > cap {
        return cap + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        let mut row_min = cur[0];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize)
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
            row_min = row_min.min(cur[j + 1]);
        }
        if row_min > cap {
            return cap + 1;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

impl Dictionary {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Dictionary> {
        let mut archive = ZipArchive::new(File::open(path)?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let mut names = std::collections::HashMap::new();
        for i in 0..archive.len() {
            if let Ok(f) = archive.by_index_raw(i) {
                names.insert(String::from_utf8_lossy(f.name_raw()).into_owned(), i);
            }
        }
        Ok(Dictionary {
            archive,
            names,
            cache: None,
            special: None,
            words: None,
        })
    }

    /// Raw contents of an archive file, by its UTF-8 name.
    fn read_raw(&mut self, name: &str) -> Option<Vec<u8>> {
        let i = *self.names.get(name)?;
        let mut raw = Vec::new();
        self.archive.by_index(i).ok()?.read_to_end(&mut raw).ok()?;
        Some(raw)
    }

    fn read_html(&mut self, name: &str) -> Option<String> {
        let raw = self.read_raw(name)?;
        if raw.starts_with(&[0x1f, 0x8b]) {
            let mut html = String::new();
            flate2::read::GzDecoder::new(&raw[..])
                .read_to_string(&mut html)
                .ok()?;
            Some(html)
        } else {
            Some(String::from_utf8_lossy(&raw).into_owned())
        }
    }

    fn prefix_html(&mut self, pfx: &str) -> Option<&str> {
        if self.cache.as_ref().map(|(p, _)| p == pfx).unwrap_or(false) {
            return self.cache.as_ref().map(|(_, h)| h.as_str());
        }
        let html = self.read_html(&format!("{pfx}.html"))?;
        self.cache = Some((pfx.to_string(), html));
        self.cache.as_ref().map(|(_, h)| h.as_str())
    }

    /// Entries whose name (or a variant) matches `cand`: in its prefix file,
    /// then among the numbered homographs of `11.html`.
    fn find_exact(&mut self, cand: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let pfx = prefix(cand);
        if let Some(html) = self.prefix_html(&pfx) {
            for (name, variants, body) in parse_entries(html) {
                if name_key(&name) == cand || variants.iter().any(|v| v == cand) {
                    out.push((name, body));
                }
            }
        }
        if pfx != "11" {
            if self.special.is_none() {
                let html = self.read_html("11.html").unwrap_or_default();
                self.special = Some(parse_entries(&html));
            }
            for (name, variants, body) in self.special.as_ref().unwrap() {
                if name_key(name) == cand || variants.iter().any(|v| v == cand) {
                    out.push((name.clone(), body.clone()));
                }
            }
        }
        // Homographs in numeric order ("1aide" before "2aide").
        out.sort_by_key(|(name, _)| {
            name.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(0)
        });
        out
    }

    /// Entries for `word` (name, HTML): exact form, without elision, then
    /// stripped forms (plurals, feminines, conjugations), then a word that
    /// starts with the query. At most `max`.
    pub fn lookup_raw(&mut self, word: &str, max: usize) -> Vec<(String, String)> {
        let q = normalize(word);
        if q.is_empty() {
            return Vec::new();
        }
        let mut forms = vec![q.clone()];
        if let Some(u) = unelide(&q) {
            forms.push(u);
        }
        let mut candidates: Vec<String> = Vec::new();
        for f in &forms {
            for c in stems(f) {
                if !candidates.contains(&c) {
                    candidates.push(c);
                }
            }
        }
        // Exact forms first ("d'abord" before "abord"), then the stripped ones.
        candidates.sort_by_key(|c| !forms.contains(c));
        let mut out: Vec<(String, String)> = Vec::new();
        for cand in &candidates {
            let found = self.find_exact(cand);
            if forms.contains(cand) {
                out.extend(found);
            } else if out.is_empty() {
                // No entry for the form as typed: the first lemma found.
                if !found.is_empty() {
                    out.extend(found);
                    break;
                }
            } else {
                // Form as typed found ("lente", n.f.): add the lemma that
                // lists it in its headword ("lent, lente", adj.).
                for f in found {
                    if headword_forms(&f.1).iter().any(|h| forms.contains(h)) && !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
            if out.len() >= max {
                break;
            }
        }
        if !out.is_empty() {
            out.truncate(max);
            return out;
        }
        // Last resort: a word that starts like the query.
        let last = forms.last().cloned().unwrap_or(q);
        if last.chars().count() >= 4 {
            let pfx = prefix(&last);
            if let Some(html) = self.prefix_html(&pfx) {
                if let Some((name, _, body)) = parse_entries(html)
                    .into_iter()
                    .find(|e| name_key(&e.0).starts_with(last.as_str()))
                {
                    return vec![(name, body)];
                }
            }
        }
        Vec::new()
    }

    /// Looks up `word`: entries reduced to text (see [`lookup_raw`](Self::lookup_raw)).
    pub fn lookup(&mut self, word: &str, max: usize) -> Vec<Entry> {
        self.lookup_raw(word, max)
            .into_iter()
            .map(|(name, body)| Entry {
                headword: name_key(&name),
                text: html_to_text(&body),
            })
            .collect()
    }

    /// Looks up `word`: formatted entries (senses, examples, cross-references).
    pub fn lookup_rich(&mut self, word: &str, max: usize) -> Vec<RichEntry> {
        self.lookup_raw(word, max)
            .into_iter()
            .map(|(_, body)| rich::parse(&body))
            .collect()
    }

    /// Dictionary words close to `word` (one or two letters of
    /// difference, same initial), for a word not found; at most `n`.
    pub fn suggestions(&mut self, word: &str, n: usize) -> Vec<String> {
        if self.words.is_none() {
            let raw = self
                .read_raw("words.original")
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let mut list: Vec<(String, String)> = if !raw.is_empty() {
                raw.lines()
                    .map(|l| {
                        l.trim()
                            .trim_start_matches("(*)")
                            .trim_start_matches('*')
                            .trim_start_matches(|c: char| c.is_ascii_digit())
                            .trim()
                            .to_string()
                    })
                    .filter(|w| !w.is_empty() && !w.starts_with('-'))
                    .map(|w| (fold(&w.to_lowercase()), w))
                    .collect()
            } else {
                Vec::new()
            };
            list.sort();
            list.dedup_by(|a, b| a.1 == b.1);
            self.words = Some(list);
        }
        let q = normalize(word);
        let q = unelide(&q).unwrap_or(q);
        let qf: Vec<char> = fold(&q).chars().collect();
        let Some(first) = qf.first().copied() else {
            return Vec::new();
        };
        let cap = if qf.len() <= 4 { 1 } else { 2 };
        let mut scored: Vec<(usize, usize, &str)> = self
            .words
            .as_ref()
            .unwrap()
            .iter()
            .filter(|(f, _)| f.starts_with(first))
            .filter_map(|(f, w)| {
                let fc: Vec<char> = f.chars().collect();
                let d = distance(&qf, &fc, cap);
                (d <= cap && d > 0 || (d == 0 && w.to_lowercase() != q)).then_some((
                    d,
                    fc.len().abs_diff(qf.len()),
                    w.as_str(),
                ))
            })
            .collect();
        scored.sort();
        let mut out: Vec<String> = Vec::new();
        for (_, _, w) in scored {
            if !out.iter().any(|o| o == w) {
                out.push(w.to_string());
            }
            if out.len() >= n {
                break;
            }
        }
        out
    }
}

/// Forms to look up for `q` (already lowercase): the exact form, then
/// candidate lemmas by rough stripping of French. The longest endings
/// are tried first: "ralentissez" → "ralentir"
/// before "ralentisser". Without a fine-grained priority order, we prefer
/// returning several candidates and letting the dictionary decide.
pub fn stems(q: &str) -> Vec<String> {
    // (ending, what to append to the stem; "" = bare stem)
    const RULES: &[(&str, &[&str])] = &[
        // adverbs
        ("amment", &["ant"]),
        ("emment", &["ent"]),
        ("ement", &["e", ""]),
        // 2nd group (finir): present, imperfect, participle, subjunctive, simple past, future, conditional
        ("issaient", &["ir"]),
        ("issions", &["ir"]),
        ("issiez", &["ir"]),
        ("issons", &["ir"]),
        ("issent", &["ir"]),
        ("issais", &["ir"]),
        ("issait", &["ir"]),
        ("issant", &["ir"]),
        ("issante", &["ir"]),
        ("issantes", &["ir"]),
        ("issants", &["ir"]),
        ("issez", &["ir"]),
        ("isses", &["ir"]),
        ("isse", &["ir"]),
        ("iraient", &["ir"]),
        ("irions", &["ir"]),
        ("iriez", &["ir"]),
        ("irais", &["ir"]),
        ("irait", &["ir"]),
        ("irons", &["ir"]),
        ("iront", &["ir"]),
        ("irent", &["ir"]),
        ("irai", &["ir"]),
        ("iras", &["ir"]),
        ("irez", &["ir"]),
        ("ira", &["ir"]),
        ("îmes", &["ir"]),
        ("îtes", &["ir"]),
        ("ies", &["ir", "ie"]),
        ("ie", &["ir", ""]),
        ("is", &["ir", "re", ""]),
        ("it", &["ir", "re"]),
        ("i", &["ir"]),
        // 1st group (aimer): future, conditional, simple past
        ("eraient", &["er"]),
        ("erions", &["er"]),
        ("eriez", &["er"]),
        ("erais", &["er"]),
        ("erait", &["er"]),
        ("erons", &["er"]),
        ("eront", &["er"]),
        ("erai", &["er"]),
        ("eras", &["er"]),
        ("erez", &["er"]),
        ("era", &["er"]),
        ("èrent", &["er"]),
        ("âmes", &["er"]),
        ("âtes", &["er"]),
        // present, imperfect, participles: 1st group or 3rd group (-re, -ir, -oir)
        ("aient", &["er", "re", "ir", "oir"]),
        ("ions", &["er", "re", "ir", "oir"]),
        ("iez", &["er", "re", "ir", "oir"]),
        ("ais", &["er", "re", "ir", "oir"]),
        ("ait", &["er", "re", "ir", "oir"]),
        ("ant", &["er", "re", "ir", "oir"]),
        ("ante", &["er", "ant"]),
        ("antes", &["er", "ant"]),
        ("ants", &["er", "ant"]),
        ("ent", &["er", "re", "ir", ""]),
        ("ons", &["er", "re", "ir", ""]),
        ("ez", &["er", "re", "ir", ""]),
        ("ées", &["er", "ée"]),
        ("ée", &["er", "é"]),
        ("és", &["er", "é"]),
        ("é", &["er"]),
        ("ues", &["re", "oir", "ue"]),
        ("ue", &["re", "oir", ""]),
        ("us", &["re", "oir", "u"]),
        ("u", &["re", "oir"]),
        // nouns and adjectives
        ("aux", &["al", "au"]),
        ("eux", &["eux", "eu"]),
        ("es", &["e", "", "er"]),
        ("e", &["", "er"]),
        ("s", &[""]),
        ("x", &[""]),
    ];
    let mut out = vec![q.to_string()];
    let mut push = |c: String| {
        if !out.contains(&c) {
            out.push(c);
        }
    };
    for (suffix, tails) in RULES {
        if let Some(stem) = q.strip_suffix(suffix) {
            if stem.chars().count() >= 2 {
                for t in *tails {
                    push(format!("{stem}{t}"));
                    // "mangeait" → "manger", "commençons" → "commencer"
                    if *t == "er" {
                        if let Some(g) = stem.strip_suffix("ge") {
                            push(format!("{g}ger"));
                        }
                        if let Some(c) = stem.strip_suffix('ç') {
                            push(format!("{c}cer"));
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests_stems {
    use super::stems;

    #[test]
    fn verbes_et_adverbes() {
        assert!(stems("ralentissez").contains(&"ralentir".to_string()));
        assert!(stems("finissaient").contains(&"finir".to_string()));
        assert!(stems("optimiserons").contains(&"optimiser".to_string()));
        assert!(stems("mangeait").contains(&"manger".to_string()));
        assert!(stems("commençons").contains(&"commencer".to_string()));
        assert!(stems("vendent").contains(&"vendre".to_string()));
        assert!(stems("lentement").contains(&"lent".to_string()));
        assert!(stems("constamment").contains(&"constant".to_string()));
        assert!(stems("journaux").contains(&"journal".to_string()));
        assert!(stems("permis").contains(&"permis".to_string()));
        assert_eq!(stems("est")[0], "est");
    }
}

/// (headword, variants, entry HTML)
fn parse_entries(html: &str) -> Vec<(String, Vec<String>, String)> {
    let mut out = Vec::new();
    for w in html.split("<w>").skip(1) {
        let body = w.split("</w>").next().unwrap_or(w);
        let Some(i) = body.find("<a name=\"") else {
            continue;
        };
        let rest = &body[i + 9..];
        let Some(j) = rest.find('"') else { continue };
        let head = rest[..j].trim().to_string();
        let mut variants = Vec::new();
        let mut s = body;
        while let Some(k) = s.find("<variant name=\"") {
            let r = &s[k + 15..];
            if let Some(e) = r.find('"') {
                variants.push(r[..e].trim().to_lowercase());
                s = &r[e..];
            } else {
                break;
            }
        }
        out.push((head, variants, body.to_string()));
    }
    out
}

/// Reduces an entry's HTML to text: line breaks for `<br>`,
/// `<li>` as bullets, tags ignored, common entities decoded.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('>') else { break };
        let tag = after[..j].trim().to_lowercase();
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_alphanumeric())
            .collect();
        let closing = tag.starts_with('/');
        match (name.as_str(), closing) {
            ("br", _) | ("p", true) | ("ol", false) | ("ul", false) => out.push('\n'),
            ("li", false) => out.push_str("\n• "),
            ("var", false) => {
                // Skip the variants block.
                if let Some(e) = after.find("</var>") {
                    rest = &after[e + 6..];
                    continue;
                }
            }
            _ => {}
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        // The double arrow is missing from some fonts: cross-reference rendered as "›".
        .replace('⇒', "› ");
    // Compact whitespace line by line, drop excess blank lines.
    let mut lines: Vec<String> = Vec::new();
    for l in decoded.lines() {
        let t: String = l.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            if lines.last().map(|x| !x.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
        } else {
            lines.push(t);
        }
    }
    while lines.last().map(|x| x.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formes_du_mot_vedette() {
        assert_eq!(
            headword_forms("<p><b>lent, lente</b> [lɑ̃]"),
            ["lent", "lente"]
        );
        assert_eq!(
            headword_forms("<b>adhérent, ente</b>"),
            ["adhérent", "adhérente"]
        );
        assert_eq!(
            headword_forms("<b>heureux, euse</b>"),
            ["heureux", "heureuse"]
        );
        assert_eq!(
            headword_forms("<b>beau ou bel, belle</b>"),
            ["beau", "bel", "belle"]
        );
    }

    #[test]
    fn prefixes() {
        assert_eq!(prefix("abaisser"), "ab");
        assert_eq!(prefix("A"), "aa");
        assert_eq!(prefix("à"), "àa");
        assert_eq!(prefix(" x"), "xa");
        assert_eq!(prefix("123"), "11");
        assert_eq!(prefix(""), "11");
        assert_eq!(prefix("Èe"), "èe");
        assert_eq!(prefix("д"), "д");
        assert_eq!(prefix("-able"), "11");
    }

    #[test]
    fn html_to_text_keeps_structure() {
        let t = html_to_text(
            r#"<p><a name="x"/><b>x</b> [ks] <i><b>n.m.</b></i><br/><br/><ol> <li>Première. </li>&nbsp;&nbsp;⇒y. <li>Seconde.</li></ol></p>"#,
        );
        assert_eq!(t, "x [ks] n.m.\n\n• Première. › y.\n• Seconde.");
    }

    /// Every entry of the real dictionary goes through the rich parser:
    /// none crashes, none keeps markup, almost all have senses.
    #[test]
    fn rich_parse_whole_dictionary() {
        let Ok(path) = std::env::var("SIGNET_DICT") else {
            return;
        };
        let mut d = Dictionary::open(&path).unwrap();
        let names: Vec<String> = d
            .names
            .keys()
            .filter(|n| n.ends_with(".html"))
            .cloned()
            .collect();
        let (mut n, mut empty, mut odd) = (0, 0, Vec::new());
        let t0 = std::time::Instant::now();
        for name in names {
            let pfx = name.trim_end_matches(".html").to_string();
            let Some(html) = d.prefix_html(&pfx).map(|h| h.to_string()) else {
                continue;
            };
            for (head, _, body) in parse_entries(&html) {
                n += 1;
                let e = rich::parse(&body);
                if e.paras.is_empty() {
                    empty += 1;
                }
                let all: String = e
                    .paras
                    .iter()
                    .map(|p| p.text())
                    .collect::<Vec<_>>()
                    .join("\n");
                let why = [
                    ("<", all.contains('<')),
                    ("&nbsp", all.contains("&nbsp")),
                    ("⇒", all.contains('⇒')),
                    ("guillemet", all.contains('"')),
                    ("double espace", all.contains("  ")),
                    ("sans mot", e.headword.is_empty()),
                ];
                if let Some((w, _)) = why.iter().find(|(_, b)| *b) {
                    if odd.len() < 12 {
                        let i = match *w {
                            "<" => all.find('<'),
                            "guillemet" => all.find('"'),
                            "double espace" => all.find("  "),
                            "⇒" => all.find('⇒'),
                            _ => Some(0),
                        }
                        .unwrap_or(0);
                        let at: String = all[all.floor_char_boundary(i.saturating_sub(40))..]
                            .chars()
                            .take(90)
                            .collect();
                        odd.push(format!("{head} ({w}) : …{at}"));
                    }
                }
            }
        }
        eprintln!(
            "{n} entrées analysées en {:?}, {empty} sans sens, {} bizarres",
            t0.elapsed(),
            odd.len()
        );
        for o in &odd {
            eprintln!("  {o}");
        }
        assert!(n > 30_000);
        assert!(empty * 50 < n, "{empty} entrées vides");
    }

    #[test]
    fn real_dictionary() {
        let Ok(path) = std::env::var("SIGNET_DICT") else {
            return;
        };
        let mut d = Dictionary::open(&path).unwrap();
        let e = d.lookup("abaisser", 3);
        assert_eq!(e[0].headword, "abaisser");
        assert!(e[0].text.contains("Faire descendre"));
        let e = d.lookup("Abandonnées", 3);
        assert!(!e.is_empty(), "forme fléchie");
        eprintln!(
            "Abandonnées → {}\n{}",
            e[0].headword,
            e[0].text.lines().take(3).collect::<Vec<_>>().join(" | ")
        );
        let e = d.lookup("murmure", 3);
        assert!(!e.is_empty());
        eprintln!("murmure → {}", e[0].headword);
        // Numbered homographs (filed in 11.html): the two "conduite".
        let c = d.lookup_rich("conduite", 3);
        assert_eq!(c.len(), 2, "{c:#?}");
        assert!(c[0].paras[0].text().starts_with("1. ") || c[0].paras[0].number == Some(1));
        assert!(c[1].paras.iter().any(|p| p.text().contains("Canalisation")));
        assert!(
            d.lookup_rich("aide", 3).len() >= 2,
            "« aide » : homographes"
        );
        // Elisions, typographic apostrophe, word with an apostrophe.
        assert_eq!(d.lookup_rich("l’école", 1)[0].headword, "école");
        // "d'abord": the adverb's entry (displayed under "abord").
        assert!(d.lookup_rich("d'abord", 1)[0].paras[0]
            .text()
            .contains("En premier lieu"));
        assert!(!d.lookup_rich("qu'il", 1).is_empty());
        // Inflected form with its own entry: both ("lente" n.f., and "lent, lente" adj.).
        let l = d.lookup_rich("lente", 4);
        assert!(
            l.iter().any(|e| e.headword == "lent, lente"),
            "{:?}",
            l.iter().map(|e| &e.headword).collect::<Vec<_>>()
        );
        assert!(l.iter().any(|e| e.headword == "lente"));
        // Timing: a lookup on a fresh prefix file, then near-miss words
        // (first time: reads the word list).
        let mut d2 = Dictionary::open(&path).unwrap();
        let t0 = std::time::Instant::now();
        let _ = d2.lookup_rich("écrivaient", 4);
        let t1 = t0.elapsed();
        let _ = d2.suggestions("ecrivaent", 6);
        let t2 = t0.elapsed() - t1;
        let _ = d2.suggestions("concentation", 6);
        let t3 = t0.elapsed() - t1 - t2;
        eprintln!("recherche {t1:?}, mots proches {t2:?} (liste lue), puis {t3:?}");
        // Near-miss words for a typo.
        let s = d.suggestions("concentation", 6);
        assert!(s.iter().any(|w| w == "concentration"), "{s:?}");
        eprintln!("concentation → {s:?}");
    }
}
