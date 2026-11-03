//! Prints the hyphenation points (Liang, French patterns) of given words.
use signet_text::Hyphenator;
fn main() {
    let h = Hyphenator::from_tex(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/hyphenation/hyph-fr.tex"
        ))
        .unwrap(),
    );
    for w in std::env::args().skip(1) {
        println!("{w:>24} → {}", h.show(&w));
    }
}
