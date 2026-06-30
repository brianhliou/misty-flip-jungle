//! Standalone Flip Jungle tablebase builder — pure Rust, no Python/PyO3, so it runs on a
//! bare server (e.g. Hetzner) with only the Rust toolchain. It includes the game model and
//! the parallel flat builder directly (no dependency on the PyO3 lib), so:
//!
//!   cargo build --release --no-default-features --bin build_tb
//!   ./target/release/build_tb <max_pieces> [out.bin]
//!
//! Parallelizes over all CPU cores (rayon). Memory ≈ 2 × (level_size/4) bytes for the top
//! level (≈24 GB at k=6). Load the artifact back with `flatdb::FlatDB::load`.

#[path = "../game.rs"]
mod game;
#[path = "../flatdb.rs"]
mod flatdb;

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let k: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("jungle_flip_tb_{k}.bin"));

    eprintln!(
        "building flat tablebase <={k} on {} threads -> {out}",
        rayon::current_num_threads()
    );
    let t = Instant::now();
    let (db, stats) = flatdb::build(k);
    eprintln!("built in {:.1}s", t.elapsed().as_secs_f64());
    for (i, s) in stats.iter().enumerate() {
        eprintln!("  L{}: n={} W={} L={} D={}", i + 1, s.0, s.1, s.2, s.3);
    }
    match db.save(&out) {
        Ok(()) => eprintln!("wrote {out}"),
        Err(e) => {
            eprintln!("save failed: {e}");
            std::process::exit(1);
        }
    }
}
