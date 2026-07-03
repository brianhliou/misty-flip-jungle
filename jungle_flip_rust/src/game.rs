//! Fully-revealed Flip Jungle game model for the retrograde endgame tablebase.
//!
//! Square encoding (matches the Python `endgame.poskey`): an `i8` per square, -1 =
//! empty, else `color*8 + role` (color 0=red, 1=black; role 0..7 = rat..elephant, so
//! role IS rank-1 and rank comparison is just role comparison). No face-down tiles /
//! chance here — these are fully-revealed deterministic positions.

pub const NSQ: usize = 16;
/// Upper-bound sentinel during tablebase DTM relaxation (far above any real 4x4 distance).
pub const DTM_BIG: u16 = u16::MAX / 4;
pub const W: i32 = 4;
pub const H: i32 = 4;
pub const EMPTY: i8 = -1;

const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

#[inline]
pub fn pcolor(c: i8) -> i8 {
    c / 8
}
#[inline]
pub fn prole(c: i8) -> i8 {
    c % 8
}

/// Capture resolution: 0 = blocked, 1 = capture (attacker advances), 2 = 同归于尽
/// trade (both removed, no advance). Rat(role 0) takes Elephant(role 7); Elephant
/// can never take Rat; else higher role captures, equal role trades.
#[inline]
pub fn resolve(a_role: i8, t_role: i8) -> u8 {
    if a_role == 0 && t_role == 7 {
        return 1;
    }
    if a_role == 7 && t_role == 0 {
        return 0;
    }
    if a_role > t_role {
        1
    } else if a_role == t_role {
        2
    } else {
        0
    }
}

/// Pseudo-legal board moves for `stm` (no flips — board is fully revealed).
pub fn gen_moves(board: &[i8; NSQ], stm: i8, out: &mut Vec<(u8, u8)>) {
    out.clear();
    for i in 0..NSQ {
        let c = board[i];
        if c < 0 || pcolor(c) != stm {
            continue;
        }
        let (f, r) = ((i as i32) % W, (i as i32) / W);
        for (df, dr) in ORTHO {
            let (nf, nr) = (f + df, r + dr);
            if nf < 0 || nf >= W || nr < 0 || nr >= H {
                continue;
            }
            let j = (nr * W + nf) as usize;
            let t = board[j];
            if t < 0 {
                out.push((i as u8, j as u8));
            } else if pcolor(t) != stm && resolve(prole(c), prole(t)) != 0 {
                out.push((i as u8, j as u8));
            }
        }
    }
}

/// Apply a board move in place. A trade (equal rank) removes BOTH pieces and the
/// attacker does not advance; a capture/quiet move advances the mover.
#[inline]
pub fn apply(board: &mut [i8; NSQ], frm: usize, to: usize) {
    let mover = board[frm];
    let t = board[to];
    if t >= 0 && resolve(prole(mover), prole(t)) == 2 {
        board[to] = EMPTY;
        board[frm] = EMPTY;
    } else {
        board[to] = mover;
        board[frm] = EMPTY;
    }
}

#[inline]
pub fn eliminated(board: &[i8; NSQ], color: i8) -> bool {
    !board.iter().any(|&c| c >= 0 && pcolor(c) == color)
}

#[inline]
pub fn piece_count(board: &[i8; NSQ]) -> usize {
    board.iter().filter(|&&c| c >= 0).count()
}

/// Result for `stm`: 1 = stm wins, 0 = draw, -1 = stm loses, 2 = ongoing. Mirrors the
/// Python `result_with_moves` order: both-eliminated → draw; mover-eliminated → loss;
/// opp-eliminated → win; has pieces but no legal move → loss; else ongoing. (Clockless:
/// the no-progress clock is not modeled here — the retrograde analysis is clockless.)
pub fn result(board: &[i8; NSQ], stm: i8) -> i8 {
    let mover_gone = eliminated(board, stm);
    let opp_gone = eliminated(board, 1 - stm);
    if mover_gone && opp_gone {
        return 0;
    }
    if mover_gone {
        return -1;
    }
    if opp_gone {
        return 1;
    }
    let mut mv: Vec<(u8, u8)> = Vec::new();
    gen_moves(board, stm, &mut mv);
    if mv.is_empty() {
        return -1;
    }
    2
}

// ── Symmetry: colour swap (×2) + the 4×4 dihedral group D4 (×8) = 16 variants ──

/// The 8 D4 index permutations of a 4×4 board (square i moves to PERM[g][i]).
pub const D4: [[usize; NSQ]; 8] = build_d4();

const fn build_d4() -> [[usize; NSQ]; 8] {
    // (f,r) in 0..4. transforms: id, rot90, rot180, rot270, flipH, flipV, transpose,
    // anti-transpose. We build each by mapping (f,r) -> (f',r') then index = r'*4+f'.
    let mut out = [[0usize; NSQ]; 8];
    let mut i = 0;
    while i < NSQ {
        let f = (i % 4) as i32;
        let r = (i / 4) as i32;
        // id
        out[0][i] = (r * 4 + f) as usize;
        // rot90 (f,r)->(r, 3-f)
        out[1][i] = ((3 - f) * 4 + r) as usize;
        // rot180 (f,r)->(3-f,3-r)
        out[2][i] = ((3 - r) * 4 + (3 - f)) as usize;
        // rot270 (f,r)->(3-r, f)
        out[3][i] = (f * 4 + (3 - r)) as usize;
        // flipH (f,r)->(3-f,r)
        out[4][i] = (r * 4 + (3 - f)) as usize;
        // flipV (f,r)->(f,3-r)
        out[5][i] = ((3 - r) * 4 + f) as usize;
        // transpose (f,r)->(r,f)
        out[6][i] = (f * 4 + r) as usize;
        // anti-transpose (f,r)->(3-r,3-f)
        out[7][i] = ((3 - f) * 4 + (3 - r)) as usize;
        i += 1;
    }
    out
}

/// Pack (board, stm) into a u128: 5 bits per square (value+1 in 0..16) + 1 stm bit.
#[inline]
pub fn pack(board: &[i8; NSQ], stm: i8) -> u128 {
    let mut k: u128 = 0;
    for i in 0..NSQ {
        k |= ((board[i] + 1) as u128) << (5 * i);
    }
    (k << 1) | (stm as u128)
}

#[inline]
fn colorswap_code(c: i8) -> i8 {
    if c < 0 {
        c
    } else {
        (c + 8) % 16
    }
}

/// Canonical key = min packed value over all 16 (D4 × colour-swap) symmetric variants.
pub fn canonical(board: &[i8; NSQ], stm: i8) -> u128 {
    let mut best: u128 = u128::MAX;
    for g in 0..8 {
        let perm = &D4[g];
        // D4-transformed board
        let mut tb = [EMPTY; NSQ];
        for i in 0..NSQ {
            tb[perm[i]] = board[i];
        }
        // variant 1: no colour swap
        let k1 = pack(&tb, stm);
        if k1 < best {
            best = k1;
        }
        // variant 2: colour swap (swap every piece's colour AND the side to move)
        let mut cb = [EMPTY; NSQ];
        for i in 0..NSQ {
            cb[i] = colorswap_code(tb[i]);
        }
        let k2 = pack(&cb, 1 - stm);
        if k2 < best {
            best = k2;
        }
    }
    best
}
