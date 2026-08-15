//! Deterministic mutation smoke test: every corpus fixture is converted
//! after byte mutations at pseudo-random positions. Conversions may fail
//! with typed errors; they must never panic, hang, or exhaust memory.
//! (The cargo-fuzz targets under `fuzz/` run the same surfaces much harder.)

mod common;

use common::{fixture_root, walk};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: deterministic across runs and platforms.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[test]
fn mutated_fixtures_never_panic() {
    let root = fixture_root();
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut rng = Rng(0x5EED_1234_5678_9ABC);
    for path in files {
        let rel = path.strip_prefix(&root).unwrap();
        if rel.starts_with("abuse") {
            continue; // abuse shapes are asserted separately; mutating them is slow
        }
        let Some(format) = anydoc::Format::from_path(&path) else {
            continue;
        };
        let original = std::fs::read(&path).unwrap();
        if original.is_empty() {
            continue;
        }
        for _ in 0..25 {
            let mut bytes = original.clone();
            // A small burst of byte mutations.
            for _ in 0..1 + (rng.next() % 8) {
                let pos = (rng.next() as usize) % bytes.len();
                bytes[pos] = rng.next() as u8;
            }
            // Occasional truncation.
            if rng.next().is_multiple_of(4) {
                let cut = (rng.next() as usize) % bytes.len();
                bytes.truncate(cut.max(1));
            }
            let _ = anydoc::to_markdown_bytes(&bytes, format);
        }
    }
}
