//! Pure-Rust smoke test (no Python): build the smallest exact tablebase end to end.
//! The full parity and oracle-agreement tests live in the private research harness; this
//! just confirms the retrograde solver compiles and runs on a clean toolchain.

use jungle_flip_rust::flatdb;

#[test]
fn builds_two_piece_tablebase() {
    // Solve every position with up to two pieces by retrograde analysis.
    let (db, stats) = flatdb::build(2);
    let total: u64 = stats.iter().map(|s| s.0 as u64).sum();
    assert!(total > 0, "expected the 2-piece tablebase to resolve some positions");
    assert!(db.max_pieces() >= 2, "tablebase should cover positions up to two pieces");
}
