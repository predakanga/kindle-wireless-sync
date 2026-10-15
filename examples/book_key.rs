//! Prints the Kindle content key we would derive for each book file given.
//! Usage: cargo run --example book_key -- FILE...

use std::io::Read;

fn main() {
    for path in std::env::args().skip(1) {
        let mut head = Vec::new();
        let read = std::fs::File::open(&path)
            .and_then(|f| f.take(kws::kindle::HEAD_LEN as u64).read_to_end(&mut head));
        if let Err(e) = read {
            println!("{path}: {e}");
            continue;
        }
        let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
        println!("{path}: {:?}", kws::kindle::key_source(ext, &head));
    }
}
