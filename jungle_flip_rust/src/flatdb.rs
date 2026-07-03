//! Memory-lean retrograde tablebase: a flat 2-bit value array (+ 1-byte DTM) with a
//! combinatorial PERFECT INDEX, so positions need no stored keys (unlike the HashMap
//! builder in `endgame.rs`). This is what scales toward ≤6 pieces.
//!
//! Index of a fully-revealed position (colour-canonicalized to "red to move"):
//!   index = combo_rank(which k of the 16 distinct pieces) * P(16,k)
//!         + placement_rank(squares of those pieces, in ascending-code order)
//! Colour symmetry (÷2): any black-to-move position is stored/looked-up as its
//! colour-swapped red-to-move equivalent (same value). No D4.
//!
//! Performance: the build ENUMERATES boards directly (incremental build, index
//! computed forward — never reconstructs a board from an index), and PRECOMPUTES the
//! resolved "floor" (best value from terminal/lower-level moves) once, so the fixpoint
//! rounds only re-read in-level quiet children. Algorithm = the Python-validated
//! `endgame.py` retrograde.
//!
//! DTM (distance-to-terminal in plies; winner minimizes, loser maximizes; draws 0) is
//! a SECOND fixpoint after the W/L/D labels converge: every W/L position starts at the
//! `DTM_BIG` upper bound and is recomputed from scratch each round (win = 1 + min over
//! losing children, loss = 1 + max over all children), so values only decrease and
//! converge to the exact DTM. The parallel cross-combo reads stay safe by the same
//! monotonicity argument as the label fixpoint: a stale read is an upper bound, and we
//! only terminate on a full round with zero changes. Stored saturated to u8 (≤255).

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;

use crate::game::{apply, gen_moves, piece_count, result, DTM_BIG, EMPTY, NSQ};

// value codes packed in 2 bits
const WIN: u8 = 0;
const DRAW: u8 = 1;
const LOSS: u8 = 2;
const UNKNOWN: u8 = 3;

#[inline]
fn code_to_i8(v: u8) -> i8 {
    match v {
        WIN => 1,
        LOSS => -1,
        _ => 0, // DRAW / UNKNOWN→draw
    }
}

// ── combinatorics ────────────────────────────────────────────────────────────
const fn binom_table() -> [[u64; 17]; 17] {
    let mut c = [[0u64; 17]; 17];
    let mut n = 0;
    while n <= 16 {
        c[n][0] = 1;
        let mut k = 1;
        while k <= n {
            c[n][k] = c[n - 1][k - 1] + c[n - 1][k];
            k += 1;
        }
        n += 1;
    }
    c
}
static C: [[u64; 17]; 17] = binom_table();

#[inline]
fn perm_count(n: u64, k: usize) -> u64 {
    let mut p = 1u64;
    for i in 0..k {
        p *= n - i as u64;
    }
    p
}

/// Rank of a sorted k-combination of [0,16).
fn combo_rank(sorted: &[i8]) -> u64 {
    let mut r = 0u64;
    for (i, &v) in sorted.iter().enumerate() {
        r += C[v as usize][i + 1];
    }
    r
}

/// Rank of an injection of k items into 16 squares (squares in code order). Alloc-free:
/// the available-position index of a square is `sq - (#used squares below it)`.
fn placement_rank(squares: &[u8]) -> u64 {
    let mut used: u16 = 0;
    let mut r = 0u64;
    for (i, &sq) in squares.iter().enumerate() {
        let below = (used & ((1u16 << sq) - 1)).count_ones() as u64;
        r = r * (NSQ as u64 - i as u64) + (sq as u64 - below);
        used |= 1u16 << sq;
    }
    r
}

/// All sorted k-combinations of the 16 distinct piece codes, in `combo_rank` order.
fn gen_combos(k: usize) -> Vec<Vec<i8>> {
    let mut out = Vec::new();
    let mut cur: Vec<i8> = Vec::new();
    fn rec(start: i8, k: usize, cur: &mut Vec<i8>, out: &mut Vec<Vec<i8>>) {
        if cur.len() == k {
            out.push(cur.clone());
            return;
        }
        let need = k - cur.len();
        let mut c = start;
        while (c as usize) <= 16 - need {
            cur.push(c);
            rec(c + 1, k, cur, out);
            cur.pop();
            c += 1;
        }
    }
    rec(0, k, &mut cur, &mut out);
    out
}

/// Total positions at level k (one colour-frame): C(16,k) * P(16,k).
fn level_size(k: usize) -> u64 {
    C[16][k] * perm_count(16, k)
}

/// Canonical index of (board, stm): colour-swap to red-to-move, then combo*P + place.
fn index_of(board: &[i8; NSQ], stm: i8) -> u64 {
    let mut b = *board;
    if stm == 1 {
        for c in b.iter_mut() {
            if *c >= 0 {
                *c = (*c + 8) % 16;
            }
        }
    }
    let mut pcs: Vec<(i8, u8)> = Vec::new();
    for (sq, &c) in b.iter().enumerate() {
        if c >= 0 {
            pcs.push((c, sq as u8));
        }
    }
    pcs.sort_by_key(|&(c, _)| c);
    let k = pcs.len();
    let codes: Vec<i8> = pcs.iter().map(|&(c, _)| c).collect();
    let squares: Vec<u8> = pcs.iter().map(|&(_, s)| s).collect();
    combo_rank(&codes) * perm_count(16, k) + placement_rank(&squares)
}

// ── per-combo placement enumeration (base=0 ⇒ yields the within-combo local index) ──
fn enum_place(
    codes: &[i8; 16],
    depth: usize,
    k: usize,
    board: &mut [i8; NSQ],
    avail: &mut Vec<u8>,
    prank: u64,
    base: u64,
    f: &mut impl FnMut(u64, &[i8; NSQ]),
) {
    if depth == k {
        f(base + prank, board);
        return;
    }
    let radix = (NSQ - depth) as u64;
    let m = avail.len();
    for pos in 0..m {
        let sq = avail[pos];
        board[sq as usize] = codes[depth];
        let removed = avail.remove(pos);
        enum_place(codes, depth + 1, k, board, avail, prank * radix + pos as u64, base, f);
        avail.insert(pos, removed);
        board[sq as usize] = EMPTY;
    }
}

// ── 2-bit packed array ───────────────────────────────────────────────────────
struct Bits {
    data: Vec<u8>,
}
impl Bits {
    fn new(n: u64, fill: u8) -> Bits {
        let byte = (fill & 3) * 0b01010101;
        Bits {
            data: vec![byte; ((n + 3) / 4) as usize],
        }
    }
    #[inline]
    fn get(&self, i: u64) -> u8 {
        (self.data[(i >> 2) as usize] >> ((i & 3) * 2)) & 3
    }
    #[inline]
    fn set(&mut self, i: u64, v: u8) {
        let b = (i >> 2) as usize;
        let sh = (i & 3) * 2;
        self.data[b] = (self.data[b] & !(3 << sh)) | ((v & 3) << sh);
    }
}

/// On-disk format v2 magic (v1 files — 2-bit labels only, no DTM — are rejected).
const MAGIC: &[u8; 4] = b"JFT2";

pub struct FlatDB {
    levels: Vec<Bits>,   // levels[k] for k pieces (2-bit W/L/D labels)
    dtms: Vec<Vec<u8>>,  // dtms[k][index] = distance-to-terminal, saturated at 255; 0 for draws
}

impl FlatDB {
    pub fn value(&self, board: &[i8; NSQ], stm: i8) -> i8 {
        let k = piece_count(board);
        if k == 0 || k >= self.levels.len() {
            return 2; // out of range
        }
        code_to_i8(self.levels[k].get(index_of(board, stm)))
    }

    /// `(wld, dtm)` — wld=2 if out of range. DTM saturates at 255 plies.
    pub fn value_dtm(&self, board: &[i8; NSQ], stm: i8) -> (i8, u16) {
        let k = piece_count(board);
        if k == 0 || k >= self.levels.len() {
            return (2, 0);
        }
        let idx = index_of(board, stm);
        (code_to_i8(self.levels[k].get(idx)), self.dtms[k][idx as usize] as u16)
    }

    pub fn max_pieces(&self) -> usize {
        self.levels.len().saturating_sub(1)
    }

    /// Per-level `(max win dtm, max loss dtm)` — the longest forced win and the longest
    /// forced loss (best defense) at each piece count.
    pub fn dtm_stats(&self) -> Vec<(u16, u16)> {
        self.levels
            .iter()
            .zip(&self.dtms)
            .map(|(lv, dt)| {
                let (mut w, mut l) = (0u16, 0u16);
                for (i, &d) in dt.iter().enumerate() {
                    match lv.get(i as u64) {
                        0 => w = w.max(d as u16),        // WIN
                        2 => l = l.max(d as u16),        // LOSS
                        _ => {}
                    }
                }
                (w, l)
            })
            .collect()
    }

    /// Serialize to disk (v2): magic "JFT2", `u32 num_levels`, then per level
    /// `u64 label_byte_len` + label bytes + `u64 dtm_byte_len` + dtm bytes.
    pub fn save(&self, path: &str) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        f.write_all(MAGIC)?;
        f.write_all(&(self.levels.len() as u32).to_le_bytes())?;
        for (lv, dt) in self.levels.iter().zip(&self.dtms) {
            f.write_all(&(lv.data.len() as u64).to_le_bytes())?;
            f.write_all(&lv.data)?;
            f.write_all(&(dt.len() as u64).to_le_bytes())?;
            f.write_all(dt)?;
        }
        f.flush()
    }

    pub fn load(path: &str) -> std::io::Result<FlatDB> {
        use std::io::Read;
        let mut f = std::io::BufReader::new(std::fs::File::open(path)?);
        let mut magic = [0u8; 4];
        f.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "not a v2 flat tablebase (v2 adds DTM) — rebuild with build_tb",
            ));
        }
        let mut u32b = [0u8; 4];
        f.read_exact(&mut u32b)?;
        let nl = u32::from_le_bytes(u32b) as usize;
        let mut levels = Vec::with_capacity(nl);
        let mut dtms = Vec::with_capacity(nl);
        for _ in 0..nl {
            let mut u64b = [0u8; 8];
            f.read_exact(&mut u64b)?;
            let len = u64::from_le_bytes(u64b) as usize;
            let mut data = vec![0u8; len];
            f.read_exact(&mut data)?;
            levels.push(Bits { data });
            f.read_exact(&mut u64b)?;
            let dlen = u64::from_le_bytes(u64b) as usize;
            let mut dt = vec![0u8; dlen];
            f.read_exact(&mut dt)?;
            dtms.push(dt);
        }
        Ok(FlatDB { levels, dtms })
    }
}

#[inline]
fn lower_val(levels: &[Bits], child: &[i8; NSQ]) -> i8 {
    code_to_i8(levels[piece_count(child)].get(index_of(child, 1)))
}

/// `(wld, dtm)` of an already-built lower level.
#[inline]
fn lower_val_dtm(levels: &[Bits], dtms: &[Vec<u8>], child: &[i8; NSQ]) -> (i8, u16) {
    let k = piece_count(child);
    let idx = index_of(child, 1);
    (code_to_i8(levels[k].get(idx)), dtms[k][idx as usize] as u16)
}

// Working cell during the build: label in bits 0-1, (floor+2) in bits 2-3. One u8 per
// position (full level index). Each position is written only by its own combo's task, so
// reads of OTHER combos' cells (a quiet child colour-swaps into a different combo) are safe
// under a monotone fixpoint — labels only move UNKNOWN→decided, so a stale read costs at
// most one extra round, never correctness.
#[inline]
fn wlabel(b: u8) -> u8 {
    b & 3
}
#[inline]
fn wmake(label: u8, floor2: u8) -> u8 {
    (label & 3) | ((floor2 & 3) << 2)
}

/// Pass 0 for one combo: terminal/lower-level floor + the immediately-decided labels.
fn pass0_combo(codes_k: &[i8], k: usize, levels: &[Bits], work: &[AtomicU8]) {
    let base = combo_rank(codes_k) * perm_count(16, k);
    let mut codes = [0i8; 16];
    codes[..k].copy_from_slice(codes_k);
    let mut board = [EMPTY; NSQ];
    let mut avail: Vec<u8> = (0..NSQ as u8).collect();
    enum_place(&codes, 0, k, &mut board, &mut avail, 0, 0, &mut |li, b| {
        let idx = (base + li) as usize;
        let mut mv: Vec<(u8, u8)> = Vec::new();
        gen_moves(b, 0, &mut mv);
        if mv.is_empty() {
            work[idx].store(wmake(LOSS, 0), Ordering::Relaxed);
            return;
        }
        let mut floor: i8 = -2;
        let mut has_quiet = false;
        for (f, t) in &mv {
            let mut child = *b;
            apply(&mut child, *f as usize, *t as usize);
            let r = result(&child, 1);
            if r != 2 {
                if -r > floor {
                    floor = -r;
                }
            } else if piece_count(&child) < k {
                let cv = lower_val(levels, &child);
                if -cv > floor {
                    floor = -cv;
                }
            } else {
                has_quiet = true;
            }
        }
        let cell = if floor == 1 {
            wmake(WIN, 0)
        } else if !has_quiet {
            wmake(if floor == 0 { DRAW } else { LOSS }, 0)
        } else {
            wmake(UNKNOWN, (floor + 2) as u8)
        };
        work[idx].store(cell, Ordering::Relaxed);
    });
}

/// One fixpoint round for a combo: resolve UNKNOWN positions via in-level quiet children
/// (read by FULL canonical index — a quiet child is opponent-to-move, so it colour-swaps
/// into a possibly-different combo). Sets `changed` if anything resolved.
fn round_combo(codes_k: &[i8], k: usize, work: &[AtomicU8], changed: &AtomicBool) {
    let base = combo_rank(codes_k) * perm_count(16, k);
    let mut codes = [0i8; 16];
    codes[..k].copy_from_slice(codes_k);
    let mut board = [EMPTY; NSQ];
    let mut avail: Vec<u8> = (0..NSQ as u8).collect();
    enum_place(&codes, 0, k, &mut board, &mut avail, 0, 0, &mut |li, b| {
        let idx = (base + li) as usize;
        let cur = work[idx].load(Ordering::Relaxed);
        if wlabel(cur) != UNKNOWN {
            return;
        }
        let mut best: i8 = ((cur >> 2) & 3) as i8 - 2; // floor
        let mut mv: Vec<(u8, u8)> = Vec::new();
        gen_moves(b, 0, &mut mv);
        let mut all_decided = true;
        for (f, t) in &mv {
            let mut child = *b;
            apply(&mut child, *f as usize, *t as usize);
            if result(&child, 1) != 2 || piece_count(&child) < k {
                continue; // resolved into the floor already
            }
            let cl = wlabel(work[index_of(&child, 1) as usize].load(Ordering::Relaxed));
            if cl == UNKNOWN {
                all_decided = false;
            } else {
                let civ = -code_to_i8(cl);
                if civ > best {
                    best = civ;
                }
            }
        }
        if best == 1 {
            work[idx].store(wmake(WIN, 0), Ordering::Relaxed);
            changed.store(true, Ordering::Relaxed);
        } else if all_decided {
            work[idx].store(wmake(if best == 0 { DRAW } else { LOSS }, 0), Ordering::Relaxed);
            changed.store(true, Ordering::Relaxed);
        }
    });
}

/// One DTM-relaxation round for a combo (labels already converged, read-only). Recomputes
/// each W/L position's distance from scratch — win = 1 + min over losing children, loss =
/// 1 + max over ALL children — and stores it only if smaller (monotone decrease from the
/// DTM_BIG upper bound, so stale cross-combo reads are safe; see module doc).
#[allow(clippy::too_many_arguments)]
fn dtm_round_combo(
    codes_k: &[i8], k: usize, levels: &[Bits], dtms: &[Vec<u8>],
    work: &[AtomicU8], dtm: &[AtomicU16], changed: &AtomicBool,
) {
    let base = combo_rank(codes_k) * perm_count(16, k);
    let mut codes = [0i8; 16];
    codes[..k].copy_from_slice(codes_k);
    let mut board = [EMPTY; NSQ];
    let mut avail: Vec<u8> = (0..NSQ as u8).collect();
    enum_place(&codes, 0, k, &mut board, &mut avail, 0, 0, &mut |li, b| {
        let idx = (base + li) as usize;
        let lab = wlabel(work[idx].load(Ordering::Relaxed));
        if lab != WIN && lab != LOSS {
            return; // draws (incl. UNKNOWN→draw) carry no distance
        }
        let mut mv: Vec<(u8, u8)> = Vec::new();
        gen_moves(b, 0, &mut mv);
        // no-move ⇒ already-terminal loss, distance 0
        let mut new_d: u16 = if mv.is_empty() { 0 } else if lab == WIN { DTM_BIG } else { 0 };
        for (f, t) in &mv {
            let mut child = *b;
            apply(&mut child, *f as usize, *t as usize);
            let r = result(&child, 1);
            let (cval, cd) = if r != 2 {
                (-r, 0u16)
            } else if piece_count(&child) < k {
                let (cv, cdtm) = lower_val_dtm(levels, dtms, &child);
                (-cv, cdtm)
            } else {
                let cidx = index_of(&child, 1) as usize;
                let clab = wlabel(work[cidx].load(Ordering::Relaxed));
                (-code_to_i8(clab), dtm[cidx].load(Ordering::Relaxed))
            };
            let total = 1 + cd.min(DTM_BIG);
            if lab == WIN {
                if cval == 1 && total < new_d {
                    new_d = total;
                }
            } else if total > new_d {
                new_d = total;
            }
        }
        if new_d < dtm[idx].load(Ordering::Relaxed) {
            dtm[idx].store(new_d, Ordering::Relaxed);
            changed.store(true, Ordering::Relaxed);
        }
    });
}

/// Build the flat tablebase up to `max_pieces`. Parallelized over combinations against a
/// shared atomic working array (1 byte/position labels + 2 bytes DTM, packed to 2-bit +
/// 1-byte per level at the end). Returns `(db, per-level (n,w,l,d))`. Set
/// `JF_FLAT_VERBOSE` for per-level timing.
pub fn build(max_pieces: usize) -> (FlatDB, Vec<(u64, u64, u64, u64)>) {
    let mut levels: Vec<Bits> = vec![Bits::new(1, DRAW)]; // level 0 placeholder
    let mut dtms: Vec<Vec<u8>> = vec![vec![0u8]]; // level 0 placeholder
    let mut stats = Vec::new();
    let verbose = std::env::var("JF_FLAT_VERBOSE").is_ok();

    for k in 1..=max_pieces {
        let n = level_size(k) as usize;
        let combos = gen_combos(k);
        let t_lvl = Instant::now();

        // working arrays: vec! fill (fast) then reinterpret as atomics (AtomicU8/U16 are
        // repr(transparent) over u8/u16, identical layout — sound).
        let raw: Vec<u8> = vec![wmake(UNKNOWN, 0); n];
        let work: Vec<AtomicU8> = unsafe { std::mem::transmute::<Vec<u8>, Vec<AtomicU8>>(raw) };

        {
            let lref = &levels;
            combos.par_iter().for_each(|c| pass0_combo(c, k, lref, &work));
        }
        let mut round = 0u32;
        loop {
            round += 1;
            let changed = AtomicBool::new(false);
            combos.par_iter().for_each(|c| round_combo(c, k, &work, &changed));
            if !changed.load(Ordering::Relaxed) {
                break;
            }
        }

        // DTM relaxation over the converged labels (see module doc).
        let rawd: Vec<u16> = vec![DTM_BIG; n];
        let dtm: Vec<AtomicU16> = unsafe { std::mem::transmute::<Vec<u16>, Vec<AtomicU16>>(rawd) };
        let mut drounds = 0u32;
        loop {
            drounds += 1;
            let changed = AtomicBool::new(false);
            {
                let (lref, dref) = (&levels, &dtms);
                combos
                    .par_iter()
                    .for_each(|c| dtm_round_combo(c, k, lref, dref, &work, &dtm, &changed));
            }
            if !changed.load(Ordering::Relaxed) {
                break;
            }
        }

        // pack labels to 2-bit (UNKNOWN ⇒ DRAW) + DTM to u8 (saturate 255) + stats
        let mut label = Bits::new(n as u64, DRAW);
        let mut dt = vec![0u8; n];
        let (mut w, mut l, mut d) = (0u64, 0u64, 0u64);
        let saturated = AtomicU64::new(0);
        let unresolved = AtomicU64::new(0);
        for idx in 0..n {
            let lab = wlabel(work[idx].load(Ordering::Relaxed));
            match lab {
                WIN => {
                    label.set(idx as u64, WIN);
                    w += 1;
                }
                LOSS => {
                    label.set(idx as u64, LOSS);
                    l += 1;
                }
                _ => d += 1, // DRAW or UNKNOWN → DRAW (already the fill)
            }
            if lab == WIN || lab == LOSS {
                let dv = dtm[idx].load(Ordering::Relaxed);
                if dv >= DTM_BIG {
                    // a decided position must have a finite distance — builder invariant
                    unresolved.fetch_add(1, Ordering::Relaxed);
                }
                if dv > 255 {
                    saturated.fetch_add(1, Ordering::Relaxed);
                }
                dt[idx] = dv.min(255) as u8;
            }
        }
        let (sat, unres) = (saturated.into_inner(), unresolved.into_inner());
        assert_eq!(unres, 0, "L{k}: {unres} decided positions with unresolved DTM");
        if sat > 0 {
            eprintln!("L{k}: WARNING {sat} DTM values saturated at 255");
        }
        if verbose {
            eprintln!(
                "L{k}: n={n} {:.1}s rounds={round}+{drounds} W={w} L={l} D={d}",
                t_lvl.elapsed().as_secs_f64()
            );
        }
        stats.push((n as u64, w, l, d));
        levels.push(label);
        dtms.push(dt);
    }
    (FlatDB { levels, dtms }, stats)
}
