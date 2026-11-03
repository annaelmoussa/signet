//! Formatted dictionary entry: the HTML of Kobo dictionaries (here
//! Le Robert) split into styled pieces, for a modern definition window
//! rather than a block of text.
//!
//! What the HTML contains, and what we do with it:
//!
//! - header: `<b>lent, lente</b> [lɑ̃, lɑ̃t] <i><b>adj.</i></b>` → headword,
//!   pronunciation (phonetic alphabet), category;
//! - numbered senses: `<ol><li>Definition. </li>`; what follows `</li>`
//!   (examples, synonyms) still belongs to the same sense;
//! - examples: `<i>"Perdre son temps."</i>`, in italics, quotation marks
//!   removed; a parenthesized gloss stays in roman;
//! - synonyms: `⇒durée.` → "→ durée"; antonyms: three non-breaking spaces
//!   followed by a word (`&nbsp;&nbsp;&nbsp;dispersion`) → "≠";
//!   cross-references: `baroque*` (the word has its own entry). All three touch;
//! - usage labels after a line break ("Abstrait.", "(Choses)",
//!   `<i><b>V._intr.</i></b>`); a label right before an `<li>` opens the
//!   next sense;
//! - `_` is a non-breaking space, `<sup>`/`<sub>` are superscripts and subscripts.

/// Piece of text of a definition, with its formatting.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Run {
    pub text: String,
    pub italic: bool,
    pub bold: bool,
    /// Small caps: cross-references (synonyms, antonyms) and usage labels.
    pub caps: bool,
    pub sup: bool,
    pub sub: bool,
    /// Word to look up when this piece is tapped.
    pub link: Option<String>,
}

/// Paragraph of an entry: a numbered sense, its continuation (after a line
/// break: empty `number`, `continued`), or a division mark ("V. intr.").
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Para {
    pub number: Option<usize>,
    /// Continuation of the previous sense (same indent, no number).
    pub continued: bool,
    pub runs: Vec<Run>,
}

impl Para {
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RichEntry {
    pub headword: String,
    /// Pronunciation without the brackets ("lɑ̃, lɑ̃t").
    pub pronunciation: Option<String>,
    /// Grammatical category ("adj.", "n.m. invar.").
    pub category: Option<String>,
    pub paras: Vec<Para>,
}

#[derive(Clone, Debug, PartialEq)]
enum Ev {
    Text(String),
    Open(String),
    Close(String),
    Br,
}

/// Splits the HTML into tags and texts; `&nbsp;` becomes U+00A0, and so does `_`.
fn events(html: &str) -> Vec<Ev> {
    let mut out = Vec::new();
    let mut rest = html;
    let text = |s: &str, out: &mut Vec<Ev>| {
        if !s.is_empty() {
            // Typographic apostrophe for display (cross-references turn it back into "'").
            let t = s
                .replace("&nbsp;", "\u{a0}")
                .replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace('_', "\u{a0}")
                .replace('\'', "’");
            out.push(Ev::Text(t));
        }
    };
    while let Some(i) = rest.find('<') {
        text(&rest[..i], &mut out);
        let after = &rest[i + 1..];
        let Some(j) = after.find('>') else { break };
        let tag = after[..j].trim();
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        match name.as_str() {
            "br" => out.push(Ev::Br),
            "var" if !closing => {
                // Variants (inflected forms): not displayed.
                if let Some(e) = after.find("</var>") {
                    rest = &after[e + 6..];
                    continue;
                }
            }
            "a" | "variant" | "w" | "html" | "head" | "body" | "meta" => {}
            _ if closing => out.push(Ev::Close(name)),
            _ if self_closing => {}
            _ => out.push(Ev::Open(name)),
        }
        rest = &after[j + 1..];
    }
    text(rest, &mut out);
    out
}

/// Merges whitespace (spaces, markup non-breaking spaces) into one space; a
/// non-breaking space coming from `_` (between two non-blanks) stays non-breaking.
fn squeeze(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            let start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            // A single non-breaking space surrounded by text: the dictionary's `_`.
            let lone_nbsp = i - start == 1 && c == '\u{a0}' && start > 0 && i < chars.len();
            out.push(if lone_nbsp { '\u{a0}' } else { ' ' });
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

struct Builder {
    paras: Vec<Para>,
    cur: Para,
    /// Number of the last opened sense.
    number: usize,
    /// Inside an example (italics that start with a quotation mark).
    in_example: bool,
    /// Right after a line break: the text that follows is a usage label.
    after_br: bool,
}

impl Builder {
    fn push(&mut self, mut run: Run) {
        // No whitespace at the start of a paragraph nor double whitespace between two pieces.
        // Nor after an opening parenthesis: "(→ norme)".
        let prev_blank = self
            .cur
            .runs
            .last()
            .map(|l| l.text.ends_with(|c: char| c.is_whitespace() || c == '('))
            .unwrap_or(true);
        if prev_blank && run.text.starts_with(|c: char| c.is_whitespace()) {
            run.text = run.text.trim_start().to_string();
        }
        // Superscript glued to the word ("xvi <sup>e</sup>" → "xviᵉ").
        if run.sup {
            if let Some(last) = self.cur.runs.last_mut() {
                let t = last.text.trim_end().len();
                last.text.truncate(t);
            }
        }
        if run.text.is_empty() {
            return;
        }
        // Space between two pieces that touch (consecutive examples).
        if let (Some(last), Some(first)) = (self.cur.runs.last(), run.text.chars().next()) {
            let end = last.text.chars().last().unwrap_or(' ');
            if !end.is_whitespace()
                && !first.is_whitespace()
                && !matches!(first, '.' | ',' | ';' | ':' | ')')
                && !run.sup
                && !run.sub
                && !last.sup
                && !last.sub
                && end != '('
                && end != '\u{a0}'
            {
                self.cur.runs.push(Run {
                    text: " ".into(),
                    ..Default::default()
                });
            }
        }
        match self.cur.runs.last_mut() {
            Some(last)
                if Run {
                    text: String::new(),
                    ..last.clone()
                } == Run {
                    text: String::new(),
                    ..run.clone()
                } && run.link.is_none() =>
            {
                last.text.push_str(&run.text)
            }
            _ => self.cur.runs.push(run),
        }
    }

    /// Closes the current paragraph (if it has text).
    fn flush(&mut self) {
        let mut p = std::mem::take(&mut self.cur);
        // Edge whitespace removed.
        while p
            .runs
            .first()
            .map(|r| r.text.trim().is_empty())
            .unwrap_or(false)
        {
            p.runs.remove(0);
        }
        while p
            .runs
            .last()
            .map(|r| r.text.trim().is_empty())
            .unwrap_or(false)
        {
            p.runs.pop();
        }
        if let Some(f) = p.runs.first_mut() {
            f.text = f.text.trim_start().to_string();
        }
        if let Some(l) = p.runs.last_mut() {
            l.text = l.text.trim_end().to_string();
        }
        if !p.runs.is_empty() {
            self.paras.push(p);
        }
    }

    /// Text outside an example: synonyms (⇒), antonyms, cross-references (*). Synonyms and
    /// antonyms follow three non-breaking spaces: "␣␣␣⇒durée." or
    /// "␣␣␣dispersion" (a single word, without ⇒: an antonym).
    fn plain(&mut self, raw: &str, bold: bool, italic: bool, sup: bool, sub: bool) {
        let base = Run {
            bold,
            italic,
            sup,
            sub,
            ..Default::default()
        };
        for (i, seg) in raw.split("\u{a0}\u{a0}\u{a0}").enumerate() {
            let s = seg.trim_start_matches(['\u{a0}', ' ']);
            // An antonym is a dictionary word, in lowercase ("REM." is not one).
            if i > 0
                && !s.starts_with('⇒')
                && s.chars().next().map(|c| c.is_lowercase()).unwrap_or(false)
            {
                self.push(Run {
                    text: " ≠ ".into(),
                    ..base.clone()
                });
                self.words(s, &base);
                // The antonym list has no period: the sentence that follows needs one.
                if self
                    .cur
                    .runs
                    .last()
                    .map(|r| !r.text.trim_end().ends_with(['.', ';', ':', '!', '?']))
                    .unwrap_or(false)
                {
                    self.push(Run {
                        text: ".".into(),
                        ..base.clone()
                    });
                }
                continue;
            }
            let mut parts = seg.split('⇒');
            if let Some(first) = parts.next() {
                self.refs(&squeeze(first), &base);
            }
            for part in parts {
                self.push(Run {
                    text: " → ".into(),
                    ..base.clone()
                });
                self.words(part, &base);
            }
        }
    }

    /// Text with cross-references ("mot*") and arrows to a synonym ("⇒coryza").
    fn arrows(&mut self, s: &str, base: &Run) {
        let mut parts = s.split('⇒');
        if let Some(first) = parts.next() {
            self.refs(first, base);
        }
        for part in parts {
            self.push(Run {
                text: " → ".into(),
                italic: false,
                ..base.clone()
            });
            // The cross-reference is one or two words; the rest of the sentence resumes after.
            let end = part.find([',', '.', ';', ')', ' ']).unwrap_or(part.len());
            let word = part[..end].trim_start_matches(|c: char| c.is_ascii_digit());
            if !word.is_empty() {
                self.push(Run {
                    text: word.to_string(),
                    caps: true,
                    italic: false,
                    link: Some(link_of(word)),
                    ..base.clone()
                });
            }
            if end < part.len() {
                self.refs(&part[end..], base);
            }
        }
    }

    /// List of referenced words ("moment, période."): each one touches the next.
    fn words(&mut self, raw: &str, base: &Run) {
        let s = squeeze(raw);
        let s = s.trim();
        // End of the list: the period or what follows (free text).
        let (list, tail) = match s.find(['.', ';']) {
            Some(i) => (&s[..i], &s[i..]),
            None => (s, ""),
        };
        // Commas outside parentheses only: "près (II, 1), proche".
        let mut items: Vec<&str> = Vec::new();
        let (mut depth, mut start) = (0i32, 0usize);
        for (i, c) in list.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    items.push(list[start..i].trim());
                    start = i + 1;
                }
                _ => {}
            }
        }
        items.push(list[start..].trim());
        items.retain(|w| !w.is_empty());
        for (k, w) in items.iter().enumerate() {
            if k > 0 {
                self.push(Run {
                    text: ", ".into(),
                    ..base.clone()
                });
            }
            // "près (II, 1)": the cross-reference specifies a sense, only the word touches;
            // "2haut": homograph number removed.
            let (word, detail) = match w.find('(') {
                Some(p) => (w[..p].trim(), &w[p..]),
                None => (*w, ""),
            };
            let word = word
                .trim_end_matches('*')
                .trim_start_matches(|c: char| c.is_ascii_digit());
            self.push(Run {
                text: word.to_string(),
                caps: true,
                link: Some(link_of(word)),
                ..base.clone()
            });
            if !detail.is_empty() {
                self.push(Run {
                    text: format!(" {detail}"),
                    ..base.clone()
                });
            }
        }
        if !tail.is_empty() {
            self.refs(tail, base);
        }
    }

    /// Text with "mot*" cross-references: the word becomes tappable, the asterisk disappears.
    fn refs(&mut self, s: &str, base: &Run) {
        let mut rest = s;
        while let Some(i) = rest.find('*') {
            let before = &rest[..i];
            let start = before
                .rfind(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\''))
                .map(|k| k + before[k..].chars().next().unwrap().len_utf8())
                .unwrap_or(0);
            let word = &before[start..];
            if !before[..start].is_empty() {
                self.push(Run {
                    text: before[..start].to_string(),
                    ..base.clone()
                });
            }
            if !word.is_empty() {
                self.push(Run {
                    text: word.to_string(),
                    link: Some(link_of(word)),
                    ..base.clone()
                });
            }
            rest = &rest[i + 1..];
        }
        if !rest.is_empty() {
            self.push(Run {
                text: rest.to_string(),
                ..base.clone()
            });
        }
    }
}

/// Word to look up for a cross-reference: lowercase, straight apostrophe.
fn link_of(word: &str) -> String {
    word.to_lowercase().replace('’', "'")
}

/// Parses an entry (the contents of a `<w>…</w>`).
pub fn parse(html: &str) -> RichEntry {
    let evs = events(html);
    let mut e = RichEntry::default();
    let mut b = Builder {
        paras: Vec::new(),
        cur: Para::default(),
        number: 0,
        in_example: false,
        after_br: false,
    };
    let (mut bold, mut italic, mut sup, mut sub) = (0i32, 0i32, false, false);
    // Header: up to the first line break after the category, or the list.
    let mut header = true;
    let mut seen_head = false;
    let mut k = 0;
    while k < evs.len() {
        let ev = &evs[k];
        k += 1;
        match ev {
            Ev::Open(t) => match t.as_str() {
                "b" => bold += 1,
                "i" => italic += 1,
                "sup" => sup = true,
                "sub" => sub = true,
                "ol" => header = false,
                "li" => {
                    header = false;
                    // A lone usage label right before ("Abstrait.") opens this sense.
                    let carried = if b
                        .cur
                        .runs
                        .iter()
                        .all(|r| r.caps && r.link.is_none() || r.text.trim().is_empty())
                        && b.cur.continued
                    {
                        std::mem::take(&mut b.cur.runs)
                    } else {
                        Vec::new()
                    };
                    b.flush();
                    b.number += 1;
                    b.cur = Para {
                        number: Some(b.number),
                        continued: false,
                        runs: Vec::new(),
                    };
                    for r in carried {
                        b.push(r);
                    }
                    if b.cur.runs.iter().any(|r| !r.text.trim().is_empty()) {
                        b.push(Run {
                            text: " ".into(),
                            ..Default::default()
                        });
                    }
                    b.after_br = false;
                }
                _ => {}
            },
            Ev::Close(t) => match t.as_str() {
                "b" => bold = (bold - 1).max(0),
                "i" => {
                    italic = (italic - 1).max(0);
                    if italic == 0 {
                        b.in_example = false;
                    }
                }
                "sup" => sup = false,
                "sub" => sub = false,
                _ => {}
            },
            Ev::Br => {
                if header {
                    if seen_head && e.category.is_some() {
                        header = false;
                    }
                    continue;
                }
                // Line break inside an example: a space ("Le temps de (+ infinitif)").
                if b.in_example {
                    b.push(Run {
                        text: " ".into(),
                        italic: true,
                        ..Default::default()
                    });
                    continue;
                }
                b.flush();
                b.cur = Para {
                    number: None,
                    continued: true,
                    runs: Vec::new(),
                };
                b.after_br = true;
            }
            Ev::Text(t) => {
                if header {
                    let s = squeeze(t);
                    let s = s.trim();
                    if s.is_empty() {
                        continue;
                    }
                    if !seen_head && bold > 0 {
                        e.headword = s.to_string();
                        seen_head = true;
                    } else if italic > 0 && e.category.is_none() {
                        e.category = Some(s.to_string());
                    } else if s.starts_with('[') {
                        e.pronunciation =
                            Some(s.trim_matches(|c| c == '[' || c == ']').trim().to_string());
                    } else if seen_head {
                        // Free text before the list: a label ("Anglic.") or a cross-reference
                        // ("⇒zieuter."); first paragraph.
                        header = false;
                        b.cur = Para {
                            number: None,
                            continued: true,
                            runs: Vec::new(),
                        };
                        if s.contains('⇒') {
                            b.plain(t, bold > 0, italic > 0, sup, sub);
                        } else {
                            b.push(Run {
                                text: s.to_string(),
                                caps: true,
                                ..Default::default()
                            });
                        }
                    }
                    continue;
                }
                if italic > 0 && (b.in_example || t.trim_start().starts_with('"')) {
                    // Example: quotation marks removed, parenthesized glosses in roman.
                    b.in_example = true;
                    let s = squeeze(t).replace('"', "");
                    let mut rest = s.as_str();
                    while let Some(i) = rest.find('(') {
                        if i > 0 {
                            b.arrows(
                                &rest[..i],
                                &Run {
                                    italic: true,
                                    sup,
                                    sub,
                                    ..Default::default()
                                },
                            );
                        }
                        let close = rest[i..].find(')').map(|j| i + j + 1).unwrap_or(rest.len());
                        b.arrows(
                            &rest[i..close],
                            &Run {
                                sup,
                                sub,
                                ..Default::default()
                            },
                        );
                        rest = &rest[close..];
                    }
                    if !rest.is_empty() {
                        b.arrows(
                            rest,
                            &Run {
                                italic: true,
                                sup,
                                sub,
                                ..Default::default()
                            },
                        );
                    }
                    continue;
                }
                if italic > 0 && bold > 0 {
                    // Grammatical division ("V. intr."): its own paragraph.
                    let s = squeeze(t);
                    if s.trim().is_empty() {
                        continue;
                    }
                    b.flush();
                    b.cur = Para {
                        number: None,
                        continued: false,
                        runs: vec![Run {
                            text: s.trim().to_string(),
                            italic: true,
                            bold: true,
                            ..Default::default()
                        }],
                    };
                    b.flush();
                    b.cur = Para {
                        number: None,
                        continued: true,
                        runs: Vec::new(),
                    };
                    continue;
                }
                // Words of the same family, at the end of the entry: "lento, ralentir".
                let sq = squeeze(t);
                let tr = sq.trim();
                if italic == 0
                    && tr.chars().count() > 2
                    && tr.starts_with('"')
                    && tr.ends_with('"')
                    && !tr[1..tr.len() - 1].contains('"')
                {
                    b.flush();
                    b.cur = Para {
                        number: None,
                        continued: true,
                        runs: vec![Run {
                            text: "Même famille : ".into(),
                            caps: true,
                            ..Default::default()
                        }],
                    };
                    b.words(&tr[1..tr.len() - 1], &Run::default());
                    b.after_br = false;
                    continue;
                }
                if b.after_br {
                    // Usage label after a line break: short, before an example or a sense.
                    let s = squeeze(t);
                    let trimmed = s.trim();
                    let next_is_example_or_sense = matches!(evs.get(k), Some(Ev::Open(n)) if n == "i" || n == "li")
                        || matches!(evs.get(k), Some(Ev::Br) | None);
                    if !trimmed.is_empty()
                        && trimmed.chars().count() <= 40
                        && next_is_example_or_sense
                        && !trimmed.contains('⇒')
                    {
                        b.push(Run {
                            text: trimmed.to_string(),
                            caps: true,
                            ..Default::default()
                        });
                        b.after_br = false;
                        continue;
                    }
                    if !trimmed.is_empty() {
                        b.after_br = false;
                    }
                }
                b.plain(t, bold > 0, italic > 0, sup, sub);
            }
        }
    }
    b.flush();
    // Trailing paragraphs with no content (a final line break): removed.
    e.paras = b.paras;
    if e.headword.is_empty() {
        e.headword = html
            .split("<a name=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or("")
            .to_string();
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(e: &RichEntry) -> Vec<String> {
        e.paras
            .iter()
            .map(|p| {
                format!(
                    "{}{}",
                    p.number
                        .map(|n| format!("{n}. "))
                        .unwrap_or_else(|| if p.continued {
                            "  ".into()
                        } else {
                            "# ".into()
                        }),
                    p.text()
                )
            })
            .collect()
    }

    #[test]
    fn concentration() {
        let html = r#"<p><a name="concentration"/><b>concentration</b> <i><b>n.f.</i></b><br/><ol> <li>Réunion en un centre, en un même lieu. </li><i>"La concentration des troupes en un point du territoire."</i>&nbsp;&nbsp;&nbsp;⇒rassemblement. <i>"Concentration économique, (réunion (d'entreprises) sous une direction commune.)"</i>&nbsp;&nbsp;&nbsp;dispersion <i>"Camp* de concentration."</i> <li>Ce qui réunit des éléments assemblés. </li><i>"Les grandes concentrations urbaines."</i>&nbsp;&nbsp;&nbsp;⇒agglomération.  <br/>En chimie.  <li>Le fait de concentrer ou d'être concentré. </li><i>"Point, degré de concentration  (rapport entre la quantité d'un corps et sa solution). "</i> <br/>Abstrait.  <li>Application de tout l'effort intellectuel sur un seul objet. </li><i>"Concentration d'esprit."</i><i>"Ce travail exige une grande concentration."</i>&nbsp;&nbsp;&nbsp;⇒attention, réflexion. &nbsp;&nbsp;&nbsp;distraction</ol></p>"#;
        let e = parse(html);
        assert_eq!(e.headword, "concentration");
        assert_eq!(e.category.as_deref(), Some("n.f."));
        assert_eq!(e.pronunciation, None);
        let t = texts(&e);
        assert_eq!(t.len(), 4, "{t:#?}");
        assert!(t[0].starts_with("1. Réunion en un centre, en un même lieu. La concentration des troupes en un point du territoire. → rassemblement."), "{}", t[0]);
        assert!(t[0].contains(" ≠ dispersion"), "{}", t[0]);
        assert!(
            t[2].starts_with("3. En chimie. Le fait de concentrer"),
            "la marque ouvre le sens suivant : {}",
            t[2]
        );
        assert!(t[3].starts_with("4. Abstrait. Application"), "{}", t[3]);
        assert!(
            t[3].ends_with("→ attention, réflexion. ≠ distraction."),
            "{}",
            t[3]
        );
        // Examples in italics, without quotation marks; gloss in roman; tappable cross-references.
        let r0 = &e.paras[0].runs;
        assert!(r0
            .iter()
            .any(|r| r.italic && r.text.contains("La concentration des troupes")));
        assert!(
            r0.iter()
                .any(|r| !r.italic && r.text.starts_with("(réunion")),
            "{r0:#?}"
        );
        let links: Vec<&str> = e
            .paras
            .iter()
            .flat_map(|p| p.runs.iter())
            .filter_map(|r| r.link.as_deref())
            .collect();
        assert_eq!(
            links,
            [
                "rassemblement",
                "dispersion",
                "camp",
                "agglomération",
                "attention",
                "réflexion",
                "distraction"
            ]
        );
    }

    #[test]
    fn lent_et_divisions() {
        let html = r#"<p><a name="lent"/><b>lent, lente</b> [lɑ̃, lɑ̃t] <i><b>adj.</i></b><br/><br/><ol> <li>Qui manque de rapidité, met plus, trop de temps. </li><i>"Il est lent dans tout ce qu'il fait."</i>&nbsp;&nbsp;&nbsp;⇒lambin, traînard. <i>"Être lent à comprendre, à agir."</i> <br/>(Choses) <i>"Une musique lente."</i><i>"Mouvements lents."</i> <br/>(Choses)  <li>Qui met du temps à agir. </li><i>"Mort lente."</i><br/>"lento, ralentir"</ol></p>"#;
        let e = parse(html);
        assert_eq!(
            (
                e.headword.as_str(),
                e.pronunciation.as_deref(),
                e.category.as_deref()
            ),
            ("lent, lente", Some("lɑ̃, lɑ̃t"), Some("adj."))
        );
        let t = texts(&e);
        assert_eq!(t[0], "1. Qui manque de rapidité, met plus, trop de temps. Il est lent dans tout ce qu’il fait. → lambin, traînard. Être lent à comprendre, à agir.");
        assert_eq!(t[1], "  (Choses) Une musique lente. Mouvements lents.");
        assert_eq!(t[2], "2. (Choses) Qui met du temps à agir. Mort lente.");
        assert_eq!(t[3], "  Même famille : lento, ralentir");
        assert_eq!(
            e.paras[3]
                .runs
                .iter()
                .filter_map(|r| r.link.as_deref())
                .collect::<Vec<_>>(),
            ["lento", "ralentir"]
        );
        let html2 = r#"<p><a name="macérer"/><b>macérer</b><var><variant name="2macération"/></var> <i><b>v.</i></b><br/><br/> <i><b>V._tr.</i></b><ol> <li>Mettre à tremper. </li> <br/>Au p._p. adj. <i>"Cerises macérées dans l'eau-de-vie."</i> <i><b>V._intr.</i></b> <li>Tremper longtemps. </li></ol></p>"#;
        let e2 = parse(html2);
        let t2 = texts(&e2);
        assert_eq!(e2.category.as_deref(), Some("v."));
        assert_eq!(t2[0], "# V.\u{a0}tr.", "{t2:#?}");
        assert_eq!(t2[1], "1. Mettre à tremper.");
        assert_eq!(
            t2[2], "  Au p.\u{a0}p. adj. Cerises macérées dans l’eau-de-vie.",
            "{t2:#?}"
        );
        assert_eq!(t2[3], "# V.\u{a0}intr.");
        assert_eq!(t2[4], "2. Tremper longtemps.");
    }

    #[test]
    fn exposants_et_renvois() {
        let html = r#"<p><a name="maniérisme"/><b>maniérisme</b> <i><b>n.m.</i></b><br/> <br/>Cour. <ol> <li>Tendance ou genre maniéré en art. </li> <br/>Art.  <li>Tendance de l'art italien au  xvi <sup>e</sup> siècle. </li><i>"Le maniérisme précède et prépare le baroque*."</i></ol></p>"#;
        let e = parse(html);
        let t = texts(&e);
        assert_eq!(t[0], "  Cour.", "{t:#?}");
        assert!(
            t[2].starts_with("2. Art. Tendance de l’art italien au xvie siècle."),
            "{}",
            t[2]
        );
        let runs = &e.paras[2].runs;
        assert!(runs.iter().any(|r| r.sup && r.text == "e"), "{runs:#?}");
        assert!(
            runs.iter()
                .any(|r| r.italic && r.link.as_deref() == Some("baroque") && r.text == "baroque"),
            "{runs:#?}"
        );
    }
}
