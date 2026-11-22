// Compile directly with rustc. Length-prefixed UTF-8 avoids a second seed implementation.
#[allow(dead_code)]
#[path = "../src/seed.rs"]
mod seed;
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let count: usize = std::env::args()
        .nth(1)
        .expect("history size")
        .parse()
        .unwrap();
    let mut out = io::BufWriter::new(io::stdout().lock());
    let mut write = |text: &str| -> io::Result<()> {
        out.write_all(&(text.len() as u32).to_le_bytes())?;
        out.write_all(text.as_bytes())
    };
    for i in 0..count {
        write(&if i % 2 == 0 {
            seed::user_text(i)
        } else {
            seed::assistant_text(i)
        })?;
    }
    for token in seed::tokenize(&seed::stream_script(80_000)) {
        write(&token)?;
    }
    out.flush()
}
