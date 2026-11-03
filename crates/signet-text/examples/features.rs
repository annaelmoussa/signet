//! Which characters does a Literata OpenType feature transform?
//! `cargo run -p signet-text --release --example features`
use signet_text::Font;
use std::str::FromStr;

fn main() {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/literata/");
    for style in ["Regular", "Italic", "Bold", "BoldItalic"] {
        let f = Font::from_bytes(std::fs::read(format!("{base}Literata-{style}.ttf")).unwrap())
            .unwrap();
        for (feat, sample) in [
            ("sups", "abcdefghijklmnopqrstuvwxyz0123456789()"),
            ("c2sc", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
            ("smcp", "abcdefghijklmnopqrstuvwxyzéèêà"),
        ] {
            let feature = rustybuzz::Feature::from_str(feat).unwrap();
            let mut ok = String::new();
            let mut ko = String::new();
            for c in sample.chars() {
                let t = c.to_string();
                let a = f.shape(&t, 30.0).glyphs[0].id;
                let b = f.shape_with(&t, 30.0, &[feature]).glyphs[0].id;
                if a != b {
                    ok.push(c)
                } else {
                    ko.push(c)
                }
            }
            println!("{style:10} {feat} : transformés « {ok} »   inchangés « {ko} »");
        }
    }
}
