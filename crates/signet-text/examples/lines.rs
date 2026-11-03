//! Typesets a paragraph and prints each line with its ratio.
//! `cargo run -p signet-text --release --example lines -- <width> <px> "text"`
use signet_text::{Align, Font, Hyphenator, ParagraphStyle, Style, Typesetter};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let width: f32 = a[0].parse().unwrap();
    let px: f32 = a[1].parse().unwrap();
    let text = &a[2];
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/literata/");
    let load =
        |n: &str| Font::from_bytes(std::fs::read(format!("{base}{n}.ttf")).unwrap()).unwrap();
    let mut ts = Typesetter::new(
        load("Literata-Regular"),
        load("Literata-Italic"),
        load("Literata-Bold"),
        load("Literata-BoldItalic"),
    );
    ts.hyphenator = Some(Hyphenator::from_tex(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/hyphenation/hyph-fr.tex"
        ))
        .unwrap(),
    ));
    let st = ParagraphStyle {
        style: Style::Regular,
        px,
        line_height: 1.4,
        align: Align::Justify,
        indent: px * 1.2,
    };
    let chars: Vec<char> = text.chars().collect();
    for kp in [true, false] {
        ts.knuth_plass.set(kp);
        println!("--- {}", if kp { "Knuth-Plass" } else { "gloutonne" });
        let (lines, _) = ts.layout_rich(&[(text, Style::Regular)], width, st);
        for l in &lines {
            let (a, b) = l
                .glyphs
                .iter()
                .fold((u32::MAX, 0u32), |(a, b), g| (a.min(g.ch), b.max(g.ch)));
            let s: String = chars[a as usize..=(b as usize).min(chars.len() - 1)]
                .iter()
                .collect();
            println!(
                "{:6.1}px r={:6.2} {} « {s} »",
                l.width,
                l.ratio,
                if l.hyphenated { "-" } else { " " }
            );
        }
    }
    println!(
        "espace = {:.2} px",
        ts.font(Style::Regular).measure(" ", px)
    );
}
