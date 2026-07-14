//! Masked-state Flip Jungle model — the playing engine's game model (face-down tiles
//! + bag + flips as chance nodes). Distinct from `game.rs` (fully-revealed, tablebase).
//! The αβ + Star1 chance-node search builds on this (added next).
//!
//! Square encoding (across the PyO3 boundary): i16 per square, -1 empty, -2 face-down,
//! else 0..15 = color*8 + role (color 0=red,1=black; role 0..7 = rat..elephant, so role
//! IS rank-1). bag: [u32;16] indexed by piece code. first_color: -1 unbound / 0 / 1.

pub const W: i32 = 4;
pub const H: i32 = 4;
pub const NSQ: usize = 16;
pub const EMPTY: i16 = -1;
pub const DOWN: i16 = -2;
const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

pub const RES_RED: i16 = 0;
pub const RES_BLACK: i16 = 1;
pub const RES_DRAW: i16 = 2;
pub const RES_ONGOING: i16 = 3;
pub const NO_PROGRESS_LIMIT: u32 = 40;

#[inline]
pub fn is_piece(c: i16) -> bool {
    c >= 0
}
#[inline]
pub fn code_color(c: i16) -> i16 {
    c / 8
}
#[inline]
pub fn code_role(c: i16) -> i16 {
    c % 8
}
#[inline]
fn coord(i: usize) -> (i32, i32) {
    (i as i32 % W, i as i32 / W)
}
#[inline]
fn sqi(f: i32, r: i32) -> usize {
    (r * W + f) as usize
}
#[inline]
fn in_bounds(f: i32, r: i32) -> bool {
    f >= 0 && f < W && r >= 0 && r < H
}

/// Capture resolution by ROLE: 0 = blocked, 1 = capture (advance), 2 = 同归于尽 trade.
#[inline]
pub fn resolve(a_role: i16, t_role: i16) -> u8 {
    if a_role == 0 && t_role == 7 {
        return 1; // rat takes elephant
    }
    if a_role == 7 && t_role == 0 {
        return 0; // elephant can't take rat
    }
    if a_role > t_role {
        1
    } else if a_role == t_role {
        2
    } else {
        0
    }
}

#[derive(Clone)]
pub struct State {
    pub sq: [i16; NSQ],
    pub bag: [u32; 16],
    pub first_color: i16,
    pub ply: u32,
    pub no_progress: u32,
}

impl State {
    pub fn mover_color(&self) -> i16 {
        if self.first_color < 0 {
            return -1;
        }
        if self.ply % 2 == 0 {
            self.first_color
        } else {
            1 - self.first_color
        }
    }

    fn piece_moves(&self, frm: usize, c: i16, mc: i16, out: &mut Vec<(u8, u8)>) {
        let role = code_role(c);
        let (f, r) = coord(frm);
        for (df, dr) in ORTHO {
            let (nf, nr) = (f + df, r + dr);
            if !in_bounds(nf, nr) {
                continue;
            }
            let to = sqi(nf, nr);
            let t = self.sq[to];
            if t == EMPTY {
                out.push((frm as u8, to as u8));
            } else if is_piece(t) && code_color(t) != mc && resolve(role, code_role(t)) != 0 {
                out.push((frm as u8, to as u8)); // capture or 同归于尽 trade
            }
        }
    }

    pub fn legal_moves(&self, out: &mut Vec<(u8, u8)>) {
        out.clear();
        for i in 0..NSQ {
            if self.sq[i] == DOWN {
                out.push((i as u8, i as u8)); // a flip
            }
        }
        let mc = self.mover_color();
        if mc < 0 {
            return;
        }
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) && code_color(c) == mc {
                self.piece_moves(i, c, mc, out);
            }
        }
    }

    /// Apply a move. A flip is `from == to` with `reveal` = the revealed piece code;
    /// a board move uses `reveal` = -1.
    pub fn push(&mut self, frm: usize, to: usize, reveal: i16) {
        if frm == to {
            // flip
            self.sq[frm] = reveal;
            self.bag[reveal as usize] -= 1;
            if self.first_color < 0 {
                self.first_color = code_color(reveal);
            }
            self.no_progress = 0;
        } else {
            let mover = self.sq[frm];
            let t = self.sq[to];
            if is_piece(t) && resolve(code_role(mover), code_role(t)) == 2 {
                self.sq[to] = EMPTY; // 同归于尽: both removed, no advance
                self.sq[frm] = EMPTY;
                self.no_progress = 0;
            } else if is_piece(t) {
                self.sq[to] = mover; // capture
                self.sq[frm] = EMPTY;
                self.no_progress = 0;
            } else {
                self.sq[to] = mover; // quiet
                self.sq[frm] = EMPTY;
                self.no_progress += 1;
            }
        }
        self.ply += 1;
    }

    fn eliminated(&self, color: i16) -> bool {
        for i in 0..NSQ {
            if is_piece(self.sq[i]) && code_color(self.sq[i]) == color {
                return false;
            }
        }
        let base = (color * 8) as usize;
        self.bag[base..base + 8].iter().all(|&n| n == 0)
    }

    /// RES_RED / RES_BLACK (absolute winner), RES_DRAW, or RES_ONGOING. Clockless-free:
    /// the 40-ply no-progress clock IS modeled here (the live engine enforces it).
    pub fn result(&self) -> i16 {
        self.result_with(&mut Vec::new())
    }

    pub fn result_with(&self, scratch: &mut Vec<(u8, u8)>) -> i16 {
        if self.no_progress >= NO_PROGRESS_LIMIT {
            return RES_DRAW;
        }
        let mc = self.mover_color();
        if mc < 0 {
            // colours unbound (ply 0): flips exist ⇒ ongoing.
            self.legal_moves(scratch);
            return if scratch.is_empty() { RES_DRAW } else { RES_ONGOING };
        }
        let mover_gone = self.eliminated(mc);
        let opp_gone = self.eliminated(1 - mc);
        if mover_gone && opp_gone {
            return RES_DRAW;
        }
        if mover_gone {
            return 1 - mc; // opponent wins
        }
        if opp_gone {
            return mc;
        }
        self.legal_moves(scratch);
        if scratch.is_empty() {
            1 - mc // side to move has pieces but no legal move ⇒ loses
        } else {
            RES_ONGOING
        }
    }

    /// The flip chance distribution: `(piece_code, count)` over the bag, plus the total.
    pub fn flip_outcomes(&self) -> (Vec<(i16, u32)>, u32) {
        let mut out = Vec::new();
        let mut total = 0u32;
        for code in 0..16i16 {
            let n = self.bag[code as usize];
            if n > 0 {
                out.push((code, n));
                total += n;
            }
        }
        (out, total)
    }

    fn mobility(&self, color: i16) -> i32 {
        let mut tmp: Vec<(u8, u8)> = Vec::new();
        let mut n = 0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) && code_color(c) == color {
                tmp.clear();
                self.piece_moves(i, c, color, &mut tmp);
                n += tmp.len() as i32;
            }
        }
        n
    }

    /// True if a piece of `code` (color*8+role) is still in play — on the board or in the
    /// face-down bag (it will eventually appear). The bag is color-keyed, so this is exact.
    #[inline]
    fn code_in_play(&self, code: i16) -> bool {
        self.bag[code as usize] > 0 || self.sq.iter().any(|&c| c == code)
    }

    fn material(&self, persp: i16, values: &[f64; 8], dom_term: bool) -> f64 {
        // Dynamic role values: the rat is worth its high base only while the *enemy*
        // elephant (its sole prey) survives; the elephant is worth more once the *enemy*
        // rat (its sole predator) is gone. See RAT_DEAD_FLOOR / ELE_BOOST.
        let (mut ele_in_play, mut rat_in_play) = ([false; 2], [false; 2]);
        if dom_term {
            for col in 0..2usize {
                ele_in_play[col] = self.code_in_play((col * 8 + 7) as i16);
                rat_in_play[col] = self.code_in_play((col * 8) as i16);
            }
        }
        let mut m = 0.0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if !is_piece(c) {
                continue;
            }
            let role = code_role(c) as usize;
            let col = code_color(c) as usize;
            let mut v = values[role];
            if dom_term {
                if role == 0 && !ele_in_play[1 - col] {
                    v = RAT_DEAD_FLOOR; // enemy elephant gone — rat can capture nothing
                } else if role == 7 && !rat_in_play[1 - col] {
                    v += ELE_BOOST; // enemy rat gone — elephant is uncontested
                }
            }
            if col as i16 == persp {
                m += v;
            } else {
                m -= v;
            }
        }
        m
    }

    /// Material + mobility, tanh-scaled to (-1, 1) — mirrors the Python `positional_eval`.
    fn eval(&self, persp: i16, w_mob: f64, values: &[f64; 8], dom_term: bool) -> f64 {
        if persp < 0 {
            return 0.0;
        }
        let mat = self.material(persp, values, dom_term);
        let mob = (self.mobility(persp) - self.mobility(1 - persp)) as f64;
        ((mat + w_mob * mob) / EVAL_SCALE).tanh()
    }

    fn zkey(&self) -> u64 {
        // FNV-1a over the masked state — drives the transposition table.
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |x: u64| {
            h ^= x;
            h = h.wrapping_mul(0x100000001b3);
        };
        for i in 0..NSQ {
            mix((self.sq[i] as i64 as u64).wrapping_add(3));
        }
        for i in 0..16 {
            mix(self.bag[i] as u64);
        }
        mix((self.first_color as i64 as u64).wrapping_add(2));
        mix((self.ply % 2) as u64);
        mix(self.no_progress as u64);
        h
    }

    /// Repetition key = `zkey` MINUS the no-progress clock. A position that genuinely
    /// repeats (same board, bag, side-to-move) hashes equal even though its clock
    /// advanced between visits. Used for cycle detection along a search line, and by the
    /// UCI binary to hash the game-history `reps` seed.
    pub fn rep_key(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut mix = |x: u64| {
            h ^= x;
            h = h.wrapping_mul(0x100000001b3);
        };
        for i in 0..NSQ {
            mix((self.sq[i] as i64 as u64).wrapping_add(3));
        }
        for i in 0..16 {
            mix(self.bag[i] as u64);
        }
        mix((self.first_color as i64 as u64).wrapping_add(2));
        mix((self.ply % 2) as u64);
        h
    }
}

// ── Search: αβ negamax + Star1 chance nodes + quiescence + contempt + TT ────────
use std::collections::HashMap;

const EVAL_SCALE: f64 = 24.0;
const VMIN: f64 = -1.0;
const VMAX: f64 = 1.0;
const INF: f64 = f64::INFINITY;
/// How far a DB win/loss leaf sits from a true terminal (±1.0). Small, so a forced win
/// dominates every heuristic eval, but a material gradient still breaks ties among won
/// children (WDL has no distance-to-mate, so material proxies "closer to finishing").
/// Legacy grading — only used when `win_dist` is off.
const DB_MARGIN: f64 = 0.05;
/// Distance-aware scoring (the `win_dist` flag): a win D plies from the ROOT scores
/// `1.0 − DIST_SLOPE·min(D, DIST_CAP)`, losses mirrored (prefer longer losses), whether
/// the line ends at a true terminal (D = plies searched) or a DB leaf (D = plies searched
/// + DTM). Shorter wins strictly outrank longer ones, which is what converts won endgames
/// instead of shuffling to the repetition draw (the WLD-no-distance dawdle class). Wins
/// live in [1−slope·cap, 1] = [0.90, 1.0]; the heuristic eval is clamped to ±EVAL_CLAMP
/// so the bands never overlap (raw tanh eval can reach ±0.97 on extreme material).
const DIST_SLOPE: f64 = 0.001;
const DIST_CAP: f64 = 100.0;
const EVAL_CLAMP: f64 = 0.85;
/// |v| above this is treated as a forced win/loss for TT distance adjustment.
const WIN_BAND: f64 = 0.89;
/// Dynamic-domination constants (unique-piece game). A rat captures ONLY the elephant, so
/// once the enemy elephant is gone the rat can take nothing — it collapses to ~dead weight.
/// An elephant's sole predator is the rat, so once the enemy rat is gone the elephant is
/// uncontested and worth more. Static material can't express these conditionals; search
/// rarely reaches their consequences within the horizon.
const RAT_DEAD_FLOOR: f64 = 1.5;
const ELE_BOOST: f64 = 4.0;

// TT entry flag
const TT_EXACT: u8 = 0;
const TT_LOWER: u8 = 1;
const TT_UPPER: u8 = 2;

/// A tablebase the search can probe: the in-memory HashMap (build_db) or the flat
/// perfect-index artifact (build_tb / load_flat_db). Both yield `(wld, dtm)`.
#[derive(Clone, Copy)]
pub enum DbRef<'a> {
    Map(&'a HashMap<u128, (i8, u16)>),
    Flat(&'a crate::flatdb::FlatDB),
}

impl<'a> DbRef<'a> {
    #[inline]
    fn probe(&self, board: &[i8; crate::game::NSQ], stm: i8) -> Option<(i8, u16)> {
        match self {
            DbRef::Map(m) => m.get(&crate::endgame::key_of(board, stm)).copied(),
            DbRef::Flat(f) => {
                let (v, d) = f.value_dtm(board, stm);
                if v == 2 {
                    None
                } else {
                    Some((v, d))
                }
            }
        }
    }
}

pub struct Cfg<'a> {
    pub w_mob: f64,
    pub values: [f64; 8],
    pub contempt: f64,
    pub root: i16,
    pub quiesce: bool,
    pub quiesce_max: i32,
    /// Optional exact endgame tablebase (clockless WLD+DTM from stm's view) used as a
    /// search leaf, plus the max piece count it covers. `None` ⇒ pure heuristic search.
    pub db: Option<DbRef<'a>>,
    pub db_max: usize,
    /// Distance-aware win/loss scoring (see DIST_SLOPE). Off ⇒ legacy flat ±1.0 terminals
    /// and material-graded DB leaves.
    pub win_dist: bool,
    /// Absolute ply of the search root — distances are measured from here.
    pub root_ply: u32,
    /// Enable the dynamic rat/elephant domination term in the eval.
    pub dom_term: bool,
    /// Detect threefold-style repetition along the search line (a position that recurs on
    /// the current path is scored as a draw — a forceable cycle). Matches the platform's
    /// repetition draw rule, which the bare game model (Markov, for the tablebase) omits.
    pub rep_detect: bool,
    /// rep_keys of game-history positions already seen twice (re-entering one is the
    /// threefold draw). Seeded into the search path at every root so threefold is honored
    /// across the game, not just within the forward line. Empty ⇒ history-blind.
    pub rep_seed: &'a [u64],
    /// Killer + history move ordering (value-preserving — only reorders moves for more
    /// β-cutoffs, so the search reaches deeper at a fixed node budget). Disable with the
    /// `JF_NO_ORDER_HEUR` env var (for the on/off efficiency A/B).
    pub order_heur: bool,
}

/// Side-to-move-perspective value of a draw, applying root draw contempt.
#[inline]
fn draw_score(st: &State, cfg: &Cfg) -> f64 {
    if cfg.contempt != 0.0 && cfg.root >= 0 {
        if st.mover_color() == cfg.root { -cfg.contempt } else { cfg.contempt }
    } else {
        0.0
    }
}

/// Exact-tablebase leaf: if the position is fully revealed (no face-down tiles, empty
/// bag) and within the DB's piece range, return its graded WLD value from stm's view.
fn db_probe(st: &State, cfg: &Cfg) -> Option<f64> {
    let db = cfg.db?;
    if cfg.db_max == 0 || st.bag.iter().any(|&n| n != 0) {
        return None; // unrevealed tiles remain — not a tablebase position
    }
    let stm = st.mover_color();
    if stm < 0 {
        return None;
    }
    let mut board = [crate::game::EMPTY; crate::game::NSQ];
    let mut npieces = 0usize;
    for i in 0..NSQ {
        let c = st.sq[i];
        if c == DOWN {
            return None;
        }
        if is_piece(c) {
            board[i] = c as i8;
            npieces += 1;
        }
    }
    if npieces == 0 || npieces > cfg.db_max {
        return None;
    }
    let (wld, dtm) = db.probe(&board, stm as i8)?;
    Some(grade_db(wld, dtm, st, cfg))
}

/// Root-relative plies from the search root to `st`.
#[inline]
fn dist_from_root(st: &State, cfg: &Cfg) -> f64 {
    st.ply.saturating_sub(cfg.root_ply) as f64
}

/// Win/loss value at total root-distance `d` plies (win_dist scoring): shorter wins score
/// higher, longer losses score higher (less negative).
#[inline]
fn dist_value(sign: f64, d: f64) -> f64 {
    sign * (VMAX - DIST_SLOPE * d.min(DIST_CAP))
}

fn grade_db(wld: i8, dtm: u16, st: &State, cfg: &Cfg) -> f64 {
    if wld == 0 {
        // exact draw — mirror the drawn-terminal contempt treatment.
        if cfg.contempt != 0.0 && cfg.root >= 0 {
            return if st.mover_color() == cfg.root { -cfg.contempt } else { cfg.contempt };
        }
        return 0.0;
    }
    let sign = wld as f64; // +1 win / -1 loss from stm's view
    if cfg.win_dist {
        // total distance from the root = plies already searched + tablebase DTM
        return dist_value(sign, dist_from_root(st, cfg) + dtm as f64);
    }
    let mat = st.eval(st.mover_color(), cfg.w_mob, &cfg.values, cfg.dom_term);
    sign * (1.0 - DB_MARGIN) + DB_MARGIN * mat
}

struct Ctx {
    nodes: u64,
    budget: u64,
    tt: std::collections::HashMap<u64, (i32, f64, u8, (u8, u8))>,
    path: Vec<u64>, // rep_keys of ancestors on the current search line (rep detection)
    killers: Vec<[(u8, u8); 2]>, // [depth] -> two quiet moves that caused a β-cutoff there
    history: [[u32; NSQ]; NSQ],  // [from][to] -> depth-weighted cutoff count (quiet moves)
}

impl Ctx {
    #[inline]
    fn tick(&mut self) -> Result<(), ()> {
        self.nodes += 1;
        if self.nodes > self.budget {
            Err(())
        } else {
            Ok(())
        }
    }
}

fn terminal_value(st: &State, res: i16, cfg: &Cfg) -> f64 {
    if res == RES_DRAW {
        if cfg.contempt != 0.0 && cfg.root >= 0 {
            return if st.mover_color() == cfg.root { -cfg.contempt } else { cfg.contempt };
        }
        return 0.0;
    }
    let sign = if res == st.mover_color() { 1.0 } else { -1.0 };
    if cfg.win_dist {
        // the game ended `dist_from_root` plies into the search
        return dist_value(sign, dist_from_root(st, cfg));
    }
    sign
}

/// Heuristic leaf eval, clamped under win_dist so it can never outrank a forced win
/// (raw tanh eval reaches ±0.97 on extreme material; the win band starts at 0.90).
#[inline]
fn leaf_eval(st: &State, cfg: &Cfg) -> f64 {
    let v = st.eval(st.mover_color(), cfg.w_mob, &cfg.values, cfg.dom_term);
    if cfg.win_dist {
        v.clamp(-EVAL_CLAMP, EVAL_CLAMP)
    } else {
        v
    }
}

#[inline]
fn order_key(st: &State, m: (u8, u8), values: &[f64; 8], killers: &[(u8, u8); 2], history: &[[u32; NSQ]; NSQ]) -> i32 {
    if m.0 == m.1 {
        return -1; // flips last (chance nodes)
    }
    let t = st.sq[m.1 as usize];
    if is_piece(t) {
        return 1_000_000 + values[code_role(t) as usize] as i32; // captures/trades, MVV
    }
    // quiet move: killers first, then the history heuristic
    if m == killers[0] {
        return 900_000;
    }
    if m == killers[1] {
        return 800_000;
    }
    (history[m.0 as usize][m.1 as usize]).min(700_000) as i32
}

fn ordered_moves(
    st: &State, values: &[f64; 8], tt_best: Option<(u8, u8)>,
    killers: &[(u8, u8); 2], history: &[[u32; NSQ]; NSQ],
) -> Vec<(u8, u8)> {
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    mv.sort_by(|a, b| {
        order_key(st, *b, values, killers, history).cmp(&order_key(st, *a, values, killers, history))
    });
    if let Some(best) = tt_best {
        if let Some(pos) = mv.iter().position(|&m| m == best) {
            mv.remove(pos);
            mv.insert(0, best);
        }
    }
    mv
}

fn quiesce(st: &State, mut alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx, qdepth: i32) -> Result<f64, ()> {
    ctx.tick()?;
    let mut scratch: Vec<(u8, u8)> = Vec::new();
    let res = st.result_with(&mut scratch);
    if res != RES_ONGOING {
        return Ok(terminal_value(st, res, cfg));
    }
    if let Some(v) = db_probe(st, cfg) {
        return Ok(v);
    }
    let stand = leaf_eval(st, cfg);
    if stand >= beta || qdepth <= 0 {
        return Ok(stand);
    }
    if stand > alpha {
        alpha = stand;
    }
    // tactical = captures/trades only (target occupied)
    let mut caps: Vec<(i32, (u8, u8))> = Vec::new();
    for &m in &scratch {
        if m.0 == m.1 {
            continue;
        }
        let t = st.sq[m.1 as usize];
        if is_piece(t) {
            caps.push((cfg.values[code_role(t) as usize] as i32, m));
        }
    }
    caps.sort_by(|a, b| b.0.cmp(&a.0));
    let mut best = stand;
    for (_, m) in caps {
        let mut child = st.clone();
        child.push(m.0 as usize, m.1 as usize, -1);
        let v = -quiesce(&child, -beta, -alpha, cfg, ctx, qdepth - 1)?;
        if v > best {
            best = v;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    Ok(best)
}

fn flip_value(st: &State, m: (u8, u8), depth: i32, alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    let (outcomes, total) = st.flip_outcomes();
    let (l, u) = (VMIN, VMAX);
    let mut vsum = 0.0;
    let mut rem = 1.0;
    for (code, cnt) in outcomes {
        let p = cnt as f64 / total as f64;
        rem -= p;
        if rem < 0.0 {
            rem = 0.0;
        }
        let ai = (alpha - vsum - rem * u) / p;
        let bi = (beta - vsum - rem * l) / p;
        if ai >= u {
            return Ok(alpha);
        }
        if bi <= l {
            return Ok(beta);
        }
        let cl = if ai > l { ai } else { l };
        let cu = if bi < u { bi } else { u };
        let mut child = st.clone();
        child.push(m.0 as usize, m.1 as usize, code);
        let v = -negamax(&child, depth - 1, -cu, -cl, cfg, ctx)?;
        if v <= ai {
            return Ok(alpha);
        }
        if v >= bi {
            return Ok(beta);
        }
        vsum += p * v;
    }
    Ok(vsum)
}

fn move_value(st: &State, m: (u8, u8), depth: i32, alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    if m.0 == m.1 {
        return flip_value(st, m, depth, alpha, beta, cfg, ctx);
    }
    let mut child = st.clone();
    child.push(m.0 as usize, m.1 as usize, -1);
    Ok(-negamax(&child, depth - 1, -beta, -alpha, cfg, ctx)?)
}

fn negamax(st: &State, depth: i32, mut alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    ctx.tick()?;
    let mut scratch: Vec<(u8, u8)> = Vec::new();
    let res = st.result_with(&mut scratch);
    if res != RES_ONGOING {
        return Ok(terminal_value(st, res, cfg));
    }
    if let Some(v) = db_probe(st, cfg) {
        return Ok(v);
    }
    // Repetition: a position already on the current line is a forceable cycle ⇒ draw.
    let rk = if cfg.rep_detect { st.rep_key() } else { 0 };
    if cfg.rep_detect && ctx.path.iter().any(|&k| k == rk) {
        return Ok(draw_score(st, cfg));
    }
    if depth <= 0 {
        if cfg.quiesce {
            return quiesce(st, alpha, beta, cfg, ctx, cfg.quiesce_max);
        }
        return Ok(leaf_eval(st, cfg));
    }

    let key = st.zkey();
    let alpha_orig = alpha;
    let beta_orig = beta;
    let mut beta = beta;
    let mut tt_best = None;
    // Under win_dist, forced win/loss values are root-distance-dependent, but the same
    // zkey can be reached at different plies from the root (zkey hashes ply%2 only). TT
    // entries therefore store NODE-relative win/loss values (as if the node were the
    // root); convert at the boundary — the standard mate-distance-scoring treatment.
    let tt_adj = if cfg.win_dist { DIST_SLOPE * dist_from_root(st, cfg) } else { 0.0 };
    let from_tt = |v: f64| -> f64 {
        if v > WIN_BAND {
            (v - tt_adj).max(WIN_BAND)
        } else if v < -WIN_BAND {
            (v + tt_adj).min(-WIN_BAND)
        } else {
            v
        }
    };
    let to_tt = |v: f64| -> f64 {
        if v > WIN_BAND {
            (v + tt_adj).min(VMAX)
        } else if v < -WIN_BAND {
            (v - tt_adj).max(VMIN)
        } else {
            v
        }
    };
    if let Some(&(ed, ev, ef, eb)) = ctx.tt.get(&key) {
        let ev = from_tt(ev);
        if ed >= depth {
            match ef {
                TT_EXACT => return Ok(ev),
                TT_LOWER => {
                    if ev > alpha {
                        alpha = ev;
                    }
                }
                _ => {
                    if ev < beta {
                        beta = ev;
                    }
                }
            }
            if alpha >= beta {
                return Ok(ev);
            }
        }
        tt_best = Some(eb);
    }

    // Mark this position as on the line, so any descendant that returns here scores a
    // repetition draw. (Children below `?`-error out on budget without popping; that only
    // happens on the final aborted iteration, which best_move discards — so it's harmless.)
    if cfg.rep_detect {
        ctx.path.push(rk);
    }
    let kdi = (depth as usize).min(ctx.killers.len() - 1);
    let killers_d = ctx.killers[kdi];
    let mut best = -INF;
    let mut best_move = (255u8, 255u8);
    for m in ordered_moves(st, &cfg.values, tt_best, &killers_d, &ctx.history) {
        let v = move_value(st, m, depth, alpha, beta, cfg, ctx)?;
        if v > best {
            best = v;
            best_move = m;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            // β-cutoff: reward this move so siblings/cousins try it earlier. Quiet moves
            // only — captures are already MVV-ordered, flips are chance nodes.
            if cfg.order_heur && m.0 != m.1 && !is_piece(st.sq[m.1 as usize]) {
                let kd = &mut ctx.killers[kdi];
                if kd[0] != m {
                    kd[1] = kd[0];
                    kd[0] = m;
                }
                ctx.history[m.0 as usize][m.1 as usize] += (depth * depth) as u32;
            }
            break;
        }
    }
    if cfg.rep_detect {
        ctx.path.pop();
    }

    let flag = if best <= alpha_orig {
        TT_UPPER
    } else if best >= beta_orig {
        TT_LOWER
    } else {
        TT_EXACT
    };
    let replace = match ctx.tt.get(&key) {
        Some(&(ed, _, _, _)) => ed <= depth,
        None => true,
    };
    if replace {
        ctx.tt.insert(key, (depth, to_tt(best), flag, best_move));
    }
    Ok(best)
}

/// Unpruned-search value (no TT, no quiescence) — for the Star1 equivalence test.
pub fn search_value(st: &State, depth: i32, w_mob: f64, values: [f64; 8]) -> f64 {
    let cfg = Cfg {
        w_mob, values, contempt: 0.0, root: st.mover_color(),
        quiesce: false, quiesce_max: 0, db: None, db_max: 0, dom_term: false, rep_detect: false,
        win_dist: false, root_ply: st.ply, rep_seed: &[],
        order_heur: false,
    };
    let mut ctx = Ctx { nodes: 0, budget: u64::MAX, tt: std::collections::HashMap::new(), path: Vec::new(),
        killers: vec![[(255u8, 255u8); 2]; 128], history: [[0u32; NSQ]; NSQ] };
    negamax(st, depth, VMIN, VMAX, &cfg, &mut ctx).unwrap_or(0.0)
}

/// Nodes visited to complete a FIXED-depth search (no budget cap) — for measuring move-
/// ordering efficiency (`JF_NO_ORDER_HEUR` toggles killers/history). Use on fully-revealed
/// positions so it's pure αβ (no chance-tree blow-up).
pub fn search_nodes(st: &State, depth: i32, w_mob: f64, values: [f64; 8]) -> u64 {
    let cfg = Cfg {
        w_mob, values, contempt: 0.0, root: st.mover_color(),
        quiesce: true, quiesce_max: 8, db: None, db_max: 0, dom_term: false, rep_detect: false,
        win_dist: false, root_ply: st.ply, rep_seed: &[],
        order_heur: std::env::var("JF_NO_ORDER_HEUR").is_err(),
    };
    let mut ctx = Ctx { nodes: 0, budget: u64::MAX, tt: std::collections::HashMap::new(), path: Vec::new(),
        killers: vec![[(255u8, 255u8); 2]; 128], history: [[0u32; NSQ]; NSQ] };
    let _ = best_at_depth(st, depth, &cfg, &mut ctx, None);
    ctx.nodes
}

fn best_at_depth(st: &State, depth: i32, cfg: &Cfg, ctx: &mut Ctx, hint: Option<(u8, u8)>) -> Result<(Option<(u8, u8)>, f64), ()> {
    let kdi = (depth as usize).min(ctx.killers.len() - 1);
    let killers_d = ctx.killers[kdi];
    let moves = ordered_moves(st, &cfg.values, hint, &killers_d, &ctx.history);
    let mut best_val = -INF;
    let mut best: Option<(u8, u8)> = None;
    let mut alpha = VMIN;
    if cfg.rep_detect {
        ctx.path.clear();
        ctx.path.push(st.rep_key()); // the root is an ancestor of every line
        // Game-history positions already seen twice: re-entering one is the threefold draw.
        ctx.path.extend_from_slice(cfg.rep_seed);
    }
    for m in moves {
        let v = move_value(st, m, depth, alpha, VMAX, cfg, ctx)?;
        if v > best_val {
            best_val = v;
            best = Some(m);
            if v > alpha {
                alpha = v;
            }
        }
    }
    Ok((best, best_val))
}

/// Root search VALUE (stm-perspective) at the deepest completed depth under the budget —
/// same αβ+Star1+TT+quiesce search as `best_move`. For position analysis.
#[allow(clippy::too_many_arguments)]
pub fn search_eval(
    st: &State, node_budget: u64, contempt: f64, w_mob: f64, values: [f64; 8], max_depth: i32,
    db: Option<DbRef>, db_max: usize, dom_term: bool, rep_detect: bool, win_dist: bool,
) -> f64 {
    let cfg = Cfg {
        w_mob, values, contempt, root: st.mover_color(),
        quiesce: true, quiesce_max: 8, db, db_max, dom_term, rep_detect,
        win_dist, root_ply: st.ply, rep_seed: &[],
        order_heur: std::env::var("JF_NO_ORDER_HEUR").is_err(),
    };
    let mut scratch: Vec<(u8, u8)> = Vec::new();
    let res = st.result_with(&mut scratch);
    if res != RES_ONGOING {
        return terminal_value(st, res, &cfg);
    }
    let mut ctx = Ctx { nodes: 0, budget: node_budget, tt: std::collections::HashMap::new(), path: Vec::new(),
        killers: vec![[(255u8, 255u8); 2]; 128], history: [[0u32; NSQ]; NSQ] };
    let mut val = 0.0;
    let mut hint = None;
    for depth in 1..=max_depth {
        match best_at_depth(st, depth, &cfg, &mut ctx, hint) {
            Ok((Some(m), v)) => {
                val = v;
                hint = Some(m);
            }
            _ => break,
        }
    }
    val
}

/// Deterministic scalar hash — turns `rng_seed` into a uniform index for tie-breaking.
fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// All root moves whose value equals the best, at a fixed depth, searched with the
/// FULL window so exact ties are visible (the narrowed root search collapses ties to
/// α and hides them). Meant to run AFTER the strength-determining deepening, over its
/// warm TT, so it is cheap and does not steal search depth. `ctx.nodes` is reset to
/// give the pass its own headroom; a mid-pass budget-out falls back to `primary`.
#[allow(clippy::float_cmp)] // exact equality is intentional: only bit-identical evals tie
fn root_ties_at_depth(
    st: &State, depth: i32, cfg: &Cfg, ctx: &mut Ctx, hint: Option<(u8, u8)>, primary: (u8, u8),
) -> Vec<(u8, u8)> {
    let kdi = (depth as usize).min(ctx.killers.len() - 1);
    let killers_d = ctx.killers[kdi];
    let moves = ordered_moves(st, &cfg.values, hint, &killers_d, &ctx.history);
    if cfg.rep_detect {
        ctx.path.clear();
        ctx.path.push(st.rep_key());
        ctx.path.extend_from_slice(cfg.rep_seed);
    }
    ctx.nodes = 0; // headroom for the post-search pass (mostly TT hits)
    let mut best = -INF;
    let mut ties: Vec<(u8, u8)> = Vec::new();
    for m in moves {
        match move_value(st, m, depth, VMIN, VMAX, cfg, ctx) {
            Ok(v) => {
                if v > best {
                    best = v;
                    ties.clear();
                    ties.push(m);
                } else if v == best {
                    ties.push(m);
                }
            }
            Err(()) => return vec![primary], // budget-out: keep the proven best
        }
    }
    if ties.is_empty() { vec![primary] } else { ties }
}

/// Node-budgeted iterative deepening. Returns the best move (255,255 if none).
/// Among moves the search rates EXACTLY equal-best, picks one via `rng_seed` (so ties
/// vary game to game instead of always taking the first-ordered move). Exact-tie only,
/// so this never prefers a worse move — zero strength cost. A fixed `rng_seed` is fully
/// deterministic; the caller supplies a per-game/per-move seed for variety.
/// `rng_seed == 0` is reserved as "off": legacy first-ordered behavior, unchanged.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn best_move(
    st: &State, node_budget: u64, contempt: f64, w_mob: f64, values: [f64; 8], max_depth: i32,
    db: Option<DbRef>, db_max: usize, dom_term: bool, rep_detect: bool, win_dist: bool,
    rep_seed: &[u64], rng_seed: u64,
) -> (u8, u8) {
    best_move_scored(
        st, node_budget, contempt, w_mob, values, max_depth, db, db_max, dom_term, rep_detect,
        win_dist, rep_seed, rng_seed,
    )
    .0
}

/// Like `best_move` but also returns the root value (side-to-move perspective, ~[-1, 1]) from
/// the deepest completed depth, so the UCI front-end can emit an `info … score` line for
/// whole-game analysis. One search; move selection (incl. the tie-break) is identical to
/// `best_move`. The value is the exact score of the tie set, so it holds for the tie-broken move.
#[allow(clippy::too_many_arguments)]
pub fn best_move_scored(
    st: &State, node_budget: u64, contempt: f64, w_mob: f64, values: [f64; 8], max_depth: i32,
    db: Option<DbRef>, db_max: usize, dom_term: bool, rep_detect: bool, win_dist: bool,
    rep_seed: &[u64], rng_seed: u64,
) -> ((u8, u8), f64) {
    let cfg = Cfg {
        w_mob, values, contempt, root: st.mover_color(),
        quiesce: true, quiesce_max: 8, db, db_max, dom_term, rep_detect,
        win_dist, root_ply: st.ply, rep_seed,
        order_heur: std::env::var("JF_NO_ORDER_HEUR").is_err(),
    };
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    if mv.is_empty() {
        return ((255, 255), 0.0);
    }
    let mut ctx = Ctx { nodes: 0, budget: node_budget, tt: std::collections::HashMap::new(), path: Vec::new(),
        killers: vec![[(255u8, 255u8); 2]; 128], history: [[0u32; NSQ]; NSQ] };
    let mut best = mv[0];
    let mut best_score = 0.0f64;
    let mut hint = None;
    let mut last_depth = 0;
    for depth in 1..=max_depth {
        match best_at_depth(st, depth, &cfg, &mut ctx, hint) {
            Ok((Some(m), v)) => {
                best = m;
                best_score = v;
                hint = Some(m);
                last_depth = depth;
            }
            _ => break, // budget exceeded (or no move)
        }
    }
    if rng_seed == 0 || last_depth == 0 {
        // rng_seed==0 is the reserved "off" value: legacy deterministic behavior
        // (first-ordered best). last_depth==0 means depth 1 never completed.
        return (best, best_score);
    }
    // Break exact ties among equal-best root moves. `best` is always in the tie set.
    let ties = root_ties_at_depth(st, last_depth, &cfg, &mut ctx, Some(best), best);
    if ties.len() <= 1 {
        return (best, best_score);
    }
    let idx = (splitmix64(rng_seed) % ties.len() as u64) as usize;
    (ties[idx], best_score)
}
