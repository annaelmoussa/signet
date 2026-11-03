//! Compares greedy breaking and Knuth-Plass on the same paragraphs.
//! `cargo run -p signet-text --release --example breaks -- output.pgm`
use signet_fb::Canvas;
use signet_text::{Align, Font, Hyphenator, ParagraphStyle, Style, Typesetter};

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "breaks.pgm".into());
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
    ts.set_ink(1.4, 0.25);
    let paras = [
        "Ne vous demandez-vous jamais : « Mais qu’est-ce que j’ai vraiment fait aujourd’hui ? » N’avez-vous aucun projet en tête que vous comptez faire « bientôt », sans que cela n’arrive ? Chaque jour, les heures filent, les réunions s’enchaînent, la liste des mails à traiter s’allonge, sans compter les sollicitations permanentes des réseaux sociaux…",
        "Mais faut-il vraiment être toujours occupé ? Le divertissement sans fin est-il vraiment une récompense ? Ou bien sommes-nous tous coincés en mode pilotage automatique ?",
    ];
    let st = ParagraphStyle {
        style: Style::Regular,
        px: 26.0,
        line_height: 1.4,
        align: Align::Justify,
        indent: 30.0,
    };
    let mut c = Canvas::new(758, 1024);
    c.clear(0xFF);
    let mut y = 40;
    for (kp, label) in [(false, "gloutonne"), (true, "Knuth-Plass")] {
        ts.knuth_plass.set(kp);
        ts.draw_text(&mut c, label, 40, y, Style::Italic, 18.0, 0x60);
        y += 16;
        let t = std::time::Instant::now();
        for p in &paras {
            let (lines, h) = ts.layout_rich(&[(p, Style::Regular)], 678.0, st);
            ts.draw(&mut c, &lines, 40, y, st, 0x00);
            y += h as i32 + 12;
        }
        println!("{label}: {:?}", t.elapsed());
        y += 30;
    }
    c.save_pgm(&out).unwrap();
    println!("{out}");
}
