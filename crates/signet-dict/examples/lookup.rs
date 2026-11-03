//! Looks up words in a Kobo dicthtml: `cargo run -p signet-dict --example lookup -- dicthtml-fr.zip word…`
fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("chemin du dicthtml");
    let mut d = signet_dict::Dictionary::open(&path).expect("ouverture");
    for w in args {
        let t = std::time::Instant::now();
        let entries = d.lookup(&w, 3);
        let heads: Vec<&str> = entries.iter().map(|e| e.headword.as_str()).collect();
        println!("{w:>16} → {heads:?} ({} ms)", t.elapsed().as_millis());
    }
}
