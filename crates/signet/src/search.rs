//! Search in the book: ignoring accents and case
//! ("eleve" finds "Élève"), "oe" finds "œ", the keyboard's straight apostrophe
//! finds the typographic apostrophe. The positions found are
//! character ranges of the blocks, like highlights.

/// A passage found: chapter, block, character range, and the excerpt to
/// display (before, word found, after).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub chapter: usize,
    pub block: usize,
    pub from: u32,
    pub to: u32,
    pub before: String,
    pub matched: String,
    pub after: String,
}

/// Comparable form of a character: lowercase, without accent.
pub fn fold(c: char) -> char {
    match c {
        'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' | 'À' | 'Â' | 'Ä' | 'Á' | 'Ã' | 'Å' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'î' | 'ï' | 'í' | 'ì' | 'Î' | 'Ï' | 'Í' | 'Ì' => 'i',
        'ô' | 'ö' | 'ó' | 'ò' | 'õ' | 'Ô' | 'Ö' | 'Ó' | 'Ò' | 'Õ' => 'o',
        'ù' | 'û' | 'ü' | 'ú' | 'Ù' | 'Û' | 'Ü' | 'Ú' => 'u',
        'ç' | 'Ç' => 'c',
        'ÿ' | 'Ÿ' => 'y',
        'ñ' | 'Ñ' => 'n',
        '’' | '‘' | 'ʼ' | '´' => '\'',
        '\u{a0}' | '\u{202f}' | '\u{2009}' | '\u{2002}' | '\u{2003}' | '\t' | '\n' => ' ',
        '‐' | '‑' | '–' | '—' => '-',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Length (in characters of the text) of an occurrence of `query` (already folded)
/// starting at `text[i]`, or nothing.
fn match_at(text: &[char], i: usize, query: &[char]) -> Option<usize> {
    let (mut t, mut q) = (i, 0);
    while q < query.len() {
        let c = *text.get(t)?;
        if c == '\u{ad}' && t > i {
            // Soft hyphen in the middle of a word: invisible.
            t += 1;
            continue;
        }
        let ligature = match c {
            'œ' | 'Œ' => Some(['o', 'e']),
            'æ' | 'Æ' => Some(['a', 'e']),
            _ => None,
        };
        match ligature {
            Some(pair) if query.get(q) == Some(&pair[0]) => {
                // "oe" typed for "œ" (or the ligature typed as is).
                if query.get(q + 1) == Some(&pair[1]) {
                    q += 2;
                } else if q + 1 == query.len() {
                    q += 1;
                } else {
                    return None;
                }
            }
            _ if fold(c) == query[q] => q += 1,
            _ => return None,
        }
        t += 1;
    }
    Some(t - i)
}

/// Excerpt around [from, to): about sixty characters before, about a
/// hundred after, cut at a space.
fn snippet(text: &[char], from: usize, to: usize) -> (String, String, String) {
    let mut a = from.saturating_sub(60);
    if a > 0 {
        while a < from && text[a - 1] != ' ' {
            a += 1;
        }
    }
    let mut b = (to + 100).min(text.len());
    if b < text.len() {
        while b > to && text[b] != ' ' {
            b -= 1;
        }
    }
    let clean = |s: &[char]| {
        s.iter()
            .map(|&c| if c == '\t' || c == '\n' { ' ' } else { c })
            .filter(|&c| c != '\u{ad}')
            .collect::<String>()
    };
    let before = format!("{}{}", if a > 0 { "…" } else { "" }, clean(&text[a..from]));
    let after = format!(
        "{}{}",
        clean(&text[to..b]),
        if b < text.len() { "…" } else { "" }
    );
    (before, clean(&text[from..to]), after)
}

/// All occurrences of `query` in the block texts of each
/// chapter (`texts[chapter][block]`), in book order, at most `max`.
pub fn find(texts: &[Vec<String>], query: &str, max: usize) -> Vec<Found> {
    let q: Vec<char> = query.trim().chars().map(fold).collect();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    for (chapter, blocks) in texts.iter().enumerate() {
        for (block, text) in blocks.iter().enumerate() {
            let chars: Vec<char> = text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                match match_at(&chars, i, &q) {
                    Some(n) => {
                        let (before, matched, after) = snippet(&chars, i, i + n);
                        out.push(Found {
                            chapter,
                            block,
                            from: i as u32,
                            to: (i + n) as u32,
                            before,
                            matched,
                            after,
                        });
                        if out.len() >= max {
                            return out;
                        }
                        i += n.max(1);
                    }
                    None => i += 1,
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_majuscules_ligatures_apostrophes() {
        let texts = vec![
            vec![
                "Un élève de l’École.".to_string(),
                "Le cœur a ses raisons.".to_string(),
            ],
            vec!["ELEVE encore, et con\u{ad}centration.".to_string()],
        ];
        let f = find(&texts, "eleve", 10);
        assert_eq!(
            f.iter()
                .map(|f| (f.chapter, f.block, f.from, f.to))
                .collect::<Vec<_>>(),
            vec![(0, 0, 3, 8), (1, 0, 0, 5)]
        );
        assert_eq!(
            (
                f[0].before.as_str(),
                f[0].matched.as_str(),
                f[0].after.as_str()
            ),
            ("Un ", "élève", " de l’École.")
        );
        assert_eq!(
            find(&texts, "l'ecole", 10).len(),
            1,
            "apostrophe droite pour la typographique"
        );
        let c = find(&texts, "coeur", 10);
        assert_eq!((c[0].from, c[0].to, c[0].matched.as_str()), (3, 7, "cœur"));
        assert_eq!(find(&texts, "cœur", 10).len(), 1);
        assert_eq!(
            find(&texts, "concentration", 10).len(),
            1,
            "césure conditionnelle ignorée"
        );
        assert_eq!(find(&texts, "  ", 10).len(), 0);
        assert_eq!(find(&texts, "e", 2).len(), 2, "plafond");
    }

    #[test]
    fn extrait_coupe_aux_espaces() {
        let long = format!("{} cible {}", "mot ".repeat(40), "suite ".repeat(40));
        let f = &find(&[vec![long]], "cible", 1)[0];
        assert!(
            f.before.starts_with('…') && f.before.ends_with(' '),
            "{:?}",
            f.before
        );
        assert!(
            f.before.len() < 80 && f.after.ends_with('…'),
            "{:?}",
            f.after
        );
        assert!(
            !f.before[3..].starts_with(' ') && f.after.trim_end_matches('…').ends_with("suite"),
            "{:?} / {:?}",
            f.before,
            f.after
        );
    }
}
