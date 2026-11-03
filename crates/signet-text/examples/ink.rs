//! Visual test bench for ink rendering: the same paragraph with
//! several contrast and weight settings, in a PGM.
//! `cargo run -p signet-text --release --example ink -- output.pgm`
use signet_fb::Canvas;
use signet_text::{Align, Font, ParagraphStyle, Style, Typesetter};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "ink.pgm".into());
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/literata/");
    let load =
        |n: &str| Font::from_bytes(std::fs::read(format!("{base}{n}.ttf")).unwrap()).unwrap();
    let mut ts = Typesetter::new(
        load("Literata-Regular"),
        load("Literata-Italic"),
        load("Literata-Bold"),
        load("Literata-BoldItalic"),
    );
    let text = "Mais faut-il vraiment être toujours occupé ? Le divertissement sans fin est-il vraiment une récompense ? Ou bien sommes-nous tous coincés en mode pilotage automatique ?";
    let st = ParagraphStyle {
        style: Style::Regular,
        px: 26.0,
        line_height: 1.4,
        align: Align::Justify,
        indent: 30.0,
    };
    let mut c = Canvas::new(758, 1024);
    c.clear(0xFF);
    let settings = [
        (1.0f32, 0.0f32, "gamma 1,0 (brut)"),
        (1.4, 0.0, "gamma 1,4"),
        (1.8, 0.0, "gamma 1,8"),
        (1.4, 0.35, "gamma 1,4 + graisse 0,35"),
        (1.8, 0.5, "gamma 1,8 + graisse 0,5"),
    ];
    let mut y = 40;
    for (gamma, emb, label) in settings {
        ts.set_ink(1.0, 0.0);
        ts.draw_text(&mut c, label, 40, y, Style::Italic, 18.0, 0x60);
        y += 16;
        ts.set_ink(gamma, emb);
        let (lines, h) = ts.layout_rich(&[(text, Style::Regular)], 678.0, st);
        ts.draw(&mut c, &lines, 40, y, st, 0x00);
        y += h as i32 + 30;
    }
    c.save_pgm(&out).unwrap();
    println!("{out}");
}
