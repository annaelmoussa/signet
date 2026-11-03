//! Walks through all the chapters of an EPUB: block count, CSS styles seen.
//! `cargo run -p signet-epub --release --example dump -- livre.epub`
fn main() {
    let path = std::env::args().nth(1).expect("chemin de l'epub");
    let mut book = signet_epub::Book::open(std::path::Path::new(&path)).expect("ouverture");
    // With a chapter number: its blocks and their CSS style.
    if let Some(ci) = std::env::args()
        .nth(2)
        .and_then(|v| v.parse::<usize>().ok())
    {
        for (i, b) in book.chapter(ci).expect("chapitre").iter().enumerate() {
            println!(
                "[{i}] {:?} « {} »",
                b.style(),
                b.text().chars().take(60).collect::<String>()
            );
        }
        return;
    }
    let t = std::time::Instant::now();
    let (mut blocks, mut styled, mut hidden_like) = (0usize, 0usize, 0usize);
    for i in 0..book.spine.len() {
        match book.chapter(i) {
            Ok(b) => {
                blocks += b.len();
                styled += b
                    .iter()
                    .filter(|x| x.style() != signet_epub::BlockStyle::default())
                    .count();
                hidden_like += b.iter().filter(|x| x.text().trim().is_empty()).count();
            }
            Err(e) => eprintln!("chapitre {i} : {e:?}"),
        }
    }
    println!("{} chapitres, {blocks} blocs dont {styled} avec un style CSS, {hidden_like} vides, en {:?}", book.spine.len(), t.elapsed());
}
