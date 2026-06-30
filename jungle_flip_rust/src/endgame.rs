//! Retrograde endgame tablebase (Rust) — transcription of the Python-validated
//! `endgame.py` algorithm, with colour+D4 symmetry canonicalization so far more
//! pieces fit. Clockless exact W/L/D, built bottom-up by piece count.
//!
//! Memory model: per level we keep ONE map `canonical-key -> (floor, label)` and
//! recompute quiet children on each fixpoint pass (no stored adjacency), so memory is
//! ~one i16 per canonical position. (≥6 pieces ultimately wants a perfect-index flat
//! array instead of a HashMap; ≤5 is reachable this way.)

use std::collections::HashMap;

use crate::game::{apply, canonical, gen_moves, piece_count, result, EMPTY, NSQ};

const UNKNOWN: i8 = 2;

/// Decode a packed canonical key back into a representative (board, stm).
fn unpack(key: u128) -> ([i8; NSQ], i8) {
    let stm = (key & 1) as i8;
    let mut rest = key >> 1;
    let mut board = [EMPTY; NSQ];
    for i in 0..NSQ {
        board[i] = ((rest & 31) as i8) - 1;
        rest >>= 5;
    }
    (board, stm)
}

/// Distinct full 16-piece set as codes 0..15.
fn all_codes() -> Vec<i8> {
    (0..16).collect()
}

/// Iterate every k-subset of the 16 distinct pieces (as code lists).
fn combinations(codes: &[i8], k: usize, mut f: impl FnMut(&[i8])) {
    let n = codes.len();
    let mut idx: Vec<usize> = (0..k).collect();
    if k == 0 || k > n {
        return;
    }
    loop {
        let pick: Vec<i8> = idx.iter().map(|&i| codes[i]).collect();
        f(&pick);
        // advance the combination
        let mut i = k;
        loop {
            if i == 0 {
                return;
            }
            i -= 1;
            if idx[i] != i + n - k {
                break;
            }
        }
        idx[i] += 1;
        for j in i + 1..k {
            idx[j] = idx[j - 1] + 1;
        }
    }
}

/// Iterate every placement of `pick` pieces onto distinct squares (k-permutations of
/// the 16 squares), calling `f(board)` for each, for BOTH sides to move.
fn placements(pick: &[i8], mut f: impl FnMut(&[i8; NSQ], i8)) {
    // k-permutations of the 16 squares, recursively, for both sides to move.
    fn rec(
        pick: &[i8],
        depth: usize,
        used: &mut [bool; NSQ],
        board: &mut [i8; NSQ],
        f: &mut impl FnMut(&[i8; NSQ], i8),
    ) {
        if depth == pick.len() {
            f(board, 0);
            f(board, 1);
            return;
        }
        for sq in 0..NSQ {
            if used[sq] {
                continue;
            }
            used[sq] = true;
            board[sq] = pick[depth];
            rec(pick, depth + 1, used, board, f);
            board[sq] = EMPTY;
            used[sq] = false;
        }
    }
    let mut used = [false; NSQ];
    let mut board = [EMPTY; NSQ];
    rec(pick, 0, &mut used, &mut board, &mut f);
}

/// Build the tablebase up to `max_pieces`. Returns `(db, per_level_stats)` where stats
/// is `[(n, wins, losses, draws); per level]`.
pub fn build(max_pieces: usize) -> (HashMap<u128, i8>, Vec<(usize, usize, usize, usize)>) {
    let codes = all_codes();
    let mut db: HashMap<u128, i8> = HashMap::new();
    let mut stats = Vec::new();

    for k in 1..=max_pieces {
        // level map: canonical key -> [floor, label]
        let mut level: HashMap<u128, [i8; 2]> = HashMap::new();

        // Pass 0: discover canonical positions + compute the resolved "floor".
        combinations(&codes, k, |pick| {
            placements(pick, |board, stm| {
                let key = canonical(board, stm);
                if level.contains_key(&key) {
                    return;
                }
                let (rb, rs) = unpack(key);
                // floor = best parent-contribution from resolved (terminal/lower) moves.
                let mut mv: Vec<(u8, u8)> = Vec::new();
                gen_moves(&rb, rs, &mut mv);
                if mv.is_empty() {
                    level.insert(key, [-2, -1]); // no move ⇒ loss
                    return;
                }
                let mut floor: i8 = -2;
                let mut has_quiet = false;
                for (f, t) in &mv {
                    let mut child = rb;
                    apply(&mut child, *f as usize, *t as usize);
                    let cs = 1 - rs;
                    let r = result(&child, cs);
                    if r != 2 {
                        floor = floor.max(-r);
                    } else if piece_count(&child) < k {
                        let cv = db[&canonical(&child, cs)];
                        floor = floor.max(-cv);
                    } else {
                        has_quiet = true;
                    }
                }
                let label = if floor == 1 {
                    1
                } else if !has_quiet {
                    floor // in {-1, 0}
                } else {
                    UNKNOWN
                };
                level.insert(key, [floor, label]);
            });
        });

        // Fixpoint over quiet moves (recompute children each pass; no stored adjacency).
        loop {
            let mut changed = false;
            let keys: Vec<u128> = level
                .iter()
                .filter(|(_, v)| v[1] == UNKNOWN)
                .map(|(&kk, _)| kk)
                .collect();
            for key in keys {
                let floor = level[&key][0];
                let (rb, rs) = unpack(key);
                let mut mv: Vec<(u8, u8)> = Vec::new();
                gen_moves(&rb, rs, &mut mv);
                let mut best = floor;
                let mut all_decided = true;
                for (f, t) in &mv {
                    let mut child = rb;
                    apply(&mut child, *f as usize, *t as usize);
                    let cs = 1 - rs;
                    // Only quiet (same-level, non-terminal) moves matter here.
                    if result(&child, cs) != 2 || piece_count(&child) < k {
                        continue;
                    }
                    let cl = level[&canonical(&child, cs)][1];
                    if cl == UNKNOWN {
                        all_decided = false;
                    } else if -cl > best {
                        best = -cl;
                    }
                }
                if best == 1 {
                    level.get_mut(&key).unwrap()[1] = 1;
                    changed = true;
                } else if all_decided {
                    level.get_mut(&key).unwrap()[1] = if best == -2 { -1 } else { best };
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Undecided ⇒ draw; commit to db; collect stats.
        let (mut w, mut l, mut d) = (0usize, 0usize, 0usize);
        for (key, v) in level.iter() {
            let val = if v[1] == UNKNOWN { 0 } else { v[1] };
            match val {
                1 => w += 1,
                -1 => l += 1,
                _ => d += 1,
            }
            db.insert(*key, val);
        }
        stats.push((w + l + d, w, l, d));
    }
    (db, stats)
}

/// Canonical lookup key for a query position.
pub fn key_of(board: &[i8; NSQ], stm: i8) -> u128 {
    canonical(board, stm)
}
