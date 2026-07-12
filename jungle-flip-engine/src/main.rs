//! MistyJungleFlip — standalone Flip Jungle (兽棋/翻翻棋, 4×4) UCI engine.
//!
//! Tier-B sibling of `banqi-engine`: a tiny UCI front-end over the SAME αβ + Star1
//! chance-node + TT + quiescence search the PyO3 lib + Python bakeoffs use
//! (`jungle_flip_rust/src/engine.rs`, included via `#[path]` together with its
//! `game`/`endgame` deps), so the mistboard platform spawns + FEN-drives it each ply
//! exactly like the jieqi Pikafish / banqi binaries and reads `bestmove`.
//!
//! ── REDACTION ARGUMENT (the contract with the server) ────────────────────────────
//! Flip Jungle hides BOTH a face-down tile's role AND its ink (colour), like banqi.
//! The engine therefore NEVER receives a hidden tile's identity. The redacted FEN
//! exposes only:
//!   • `X` for every face-down tile (no role, no colour), and
//!   • the public unrevealed multiset (the "pool") as per-(ink,role) PUBLIC counts —
//!     these are derivable by either player from the full piece set minus what is
//!     revealed on the board (symmetric information; identical to banqi).
//! The engine integrates the unseen deal out as CHANCE at flip time (Star1 over the
//! pool); it can never read an individual face-down tile's role/ink off the wire.
//!
//! ── REDACTED FEN GRAMMAR ─────────────────────────────────────────────────────────
//!   <board> <turn> <pool> <clock> <movenum>
//!
//!   board   4 ranks separated by '/', TOP rank (rank 4) first; within a rank, files
//!           left→right (a,b,c,d). Empty runs collapse to a digit. A revealed piece is
//!           a role char: rat=R cat=C dog=D wolf=W leopard=P tiger=T lion=L elephant=E,
//!           UPPER = red ink, lower = black ink. A face-down tile is `X` (NO colour).
//!   turn    'r' red-ink to move | 'b' black-ink to move | '-' unbound (opening, before
//!           the first flip binds an ink to the moving seat).
//!   pool    the unrevealed multiset as <char><count> pairs (red UPPER then black
//!           lower), non-zero only; '-' if empty. Σpool MUST equal the on-board `X`
//!           count (enforced by the parser — a mismatch is a server-side encoding bug).
//!   clock   no_progress ply counter (plies since the last flip/capture/trade).
//!   movenum the absolute ply (mover_color is reconstructed from turn + ply parity).
//!
//! ── SQUARE INDEX ↔ (file,rank) ↔ UCI COORD ───────────────────────────────────────
//! Square index matches the Python model (`board.py`): `index = file + (rank-1)*4`,
//! file a..d = 0..3, rank 1..4. So a1=0, b1=1, c1=2, d1=3, a2=4, … d4=15.
//! UCI coord is `<file><rankdigit>` with file a..d and rankdigit 0..3 (= rank-1,
//! 0-indexed, mirroring banqi), so square index = file + rankdigit*4:
//!   a0=0  b0=1  c0=2  d0=3   (rank 1)
//!   a1=4  …               …  (rank 2)
//!   a3=12 …            d3=15 (rank 4)
//! A board move is `<from><to>` (e.g. "a0a1"); a FLIP is `<sq><sq>` (from==to, e.g.
//! "a0a0"). Note the UCI rankdigit is 0-indexed; it equals the Python a1..d4 rank − 1.
//!
//! Protocol (subset of UCI):
//!   uci                              -> id name/author, uciok
//!   isready                          -> readyok
//!   ucinewgame                       -> clear position
//!   position fen <FEN> [moves ...]   -> store the redacted position (the trailing
//!                                       "moves ..." token, if any, is ignored: the
//!                                       no-progress clock + masked state are carried in
//!                                       the FEN itself)
//!   go [movetime <ms>] [nodes <n>]   -> search, emit "bestmove <uci>" (or "(none)")
//!   quit                             -> exit
//!
//! `nodes` is the AUTHORITATIVE strength knob and reproduces the Python engine's
//! `node_budget` exactly (see fen_vectors.json + the golden test). `movetime` is honored
//! as a derived node-budget ceiling (the existing search exposes only a node budget, not
//! an in-search wall-clock check); whichever bound is tighter wins, mirroring banqi's
//! "honor both" semantics.
//!
//! Environment:
//!   JF_TIE_SEED  Seeds the tie-break among exactly-equal-value root moves (e.g. the
//!                opening flip, where all 16 tiles are equal by symmetry). Unset -> fresh
//!                per-search entropy, so openings vary. Set to 0 for legacy deterministic
//!                play, or a nonzero value for reproducible variety (same seed -> same
//!                game). Only tied choices are affected; unique best moves stay
//!                deterministic, so `nodes`-budget reproducibility is otherwise intact.

#[path = "../../jungle_flip_rust/src/game.rs"]
#[allow(dead_code)] // game.rs exposes the full revealed-board model; only EMPTY/NSQ are reached here
mod game;

#[path = "../../jungle_flip_rust/src/endgame.rs"]
#[allow(dead_code)] // endgame.rs also exposes builder internals unused here
mod endgame;

#[path = "../../jungle_flip_rust/src/flatdb.rs"]
#[allow(dead_code)] // flatdb.rs also exposes builder internals unused here
mod flatdb;

#[path = "../../jungle_flip_rust/src/engine.rs"]
#[allow(dead_code)] // engine.rs also exposes PyO3-facing helpers, unused here
mod engine;

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

const ENGINE_NAME: &str = "MistyJungleFlip 0.5.0";
const DEFAULT_MOVETIME_MS: u64 = 1000;
const DEFAULT_NODES: u64 = 512_000;
/// Derived node ceiling per millisecond of `movetime` (the search has no in-line wall
/// clock). Generous: the 4×4 search runs millions of nodes/sec, so this only ever binds
/// when the caller asks for a very short think; `nodes` otherwise dominates.
const NODES_PER_MS: u64 = 1_000_000;

// ── Engine knobs — pinned to RustJungleFlipStrategy's defaults so the binary reproduces
// the Python engine's choice byte-for-byte (see strategy.py / search.py).
const DEFAULT_VALUES: [f64; 8] = [6.0, 2.0, 3.0, 4.0, 5.0, 7.0, 8.0, 10.0]; // rat..elephant
const W_MOB: f64 = 0.8;
const CONTEMPT: f64 = 0.05;
const MAX_DEPTH: i32 = 24;
const DOM_TERM: bool = false;
const REP_DETECT: bool = true;
// Exact retrograde endgame tablebase, used as a search leaf for fully-revealed positions
// with ≤ this many pieces. Without it the heuristic eval can't tell a dead-drawn endgame
// (e.g. two equal lions) from a live one, so contempt makes the engine FLEE the trade and
// shuffle to the repetition/no-progress draw instead of securing it. The DB scores those
// leaves as exact draws so all endgame moves tie and move-ordering takes the trade.
//
// The preferred source is a PREBUILT flat artifact (WLD + distance-to-mate, format v2,
// built by `build_tb`): the binary looks for it at `$JUNGLE_FLIP_TB`, then next to the
// executable as `jungle_flip_tb_4.bin`. A ≤4 artifact is ~97 MB and loads in ~20 ms, so
// the per-move spawn carries it free. When no artifact is found the binary falls back to
// building the ≤2 table at startup (~instant), which still covers the dead-drawn
// 2-piece case. 0 disables the fallback leaf entirely.
const DB_FALLBACK_PIECES: usize = 2;
// Distance-aware win/loss scoring: a win D plies away scores 1.0 − 0.001·min(D, 100), at
// true terminals and tablebase leaves alike, so the shortest forced win strictly outranks
// longer ones (and forced losses are dragged out). Without it every winning move ties at
// the same value and the engine can hold a won endgame forever without finishing it —
// it shuffles until the repetition rule draws the game.
const WIN_DIST: bool = true;
const TB_FILENAME: &str = "jungle_flip_tb_4.bin";

/// The search-leaf tablebase resolved at startup: a prebuilt flat artifact when found,
/// else the ≤2 HashMap built in-process.
enum LeafDb {
    Flat(flatdb::FlatDB),
    Map(HashMap<u128, (i8, u16)>),
}

impl LeafDb {
    fn as_ref(&self) -> engine::DbRef<'_> {
        match self {
            LeafDb::Flat(f) => engine::DbRef::Flat(f),
            LeafDb::Map(m) => engine::DbRef::Map(m),
        }
    }
}

/// Locate + load the leaf tablebase. Returns `(db, db_max)`. Diagnostics go to stderr
/// (stdout is the UCI channel).
fn init_db() -> (Option<LeafDb>, usize) {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("JUNGLE_FLIP_TB") {
        candidates.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(TB_FILENAME));
        }
    }
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        match flatdb::FlatDB::load(&path.to_string_lossy()) {
            Ok(db) => {
                let mp = db.max_pieces();
                eprintln!("info tablebase {} (<= {mp} pieces, WLD+DTM)", path.display());
                return (Some(LeafDb::Flat(db)), mp);
            }
            Err(e) => eprintln!("info tablebase {} unusable: {e}", path.display()),
        }
    }
    if DB_FALLBACK_PIECES > 0 {
        eprintln!("info tablebase none found; building <= {DB_FALLBACK_PIECES} in-process");
        (Some(LeafDb::Map(endgame::build(DB_FALLBACK_PIECES).0)), DB_FALLBACK_PIECES)
    } else {
        (None, 0)
    }
}

// ── Role-char codec ──────────────────────────────────────────────────────────────
// Index 0..7 = rat,cat,dog,wolf,leopard,tiger,lion,elephant (role IS rank-1).
const ROLE_LETTERS: [u8; 8] = [b'R', b'C', b'D', b'W', b'P', b'T', b'L', b'E'];

/// Role char (case-insensitive) -> masked code `color*8 + role`. UPPER=red(0), lower=black(1).
fn letter_to_code(ch: u8) -> Option<i16> {
    let upper = ch.to_ascii_uppercase();
    let role = ROLE_LETTERS.iter().position(|&c| c == upper)? as i16;
    let color = if ch.is_ascii_uppercase() { 0 } else { 1 };
    Some(color * 8 + role)
}

/// Masked code `color*8 + role` -> role char (UPPER red / lower black).
#[allow(dead_code)] // used by fen_from_state (tests + vector generation)
fn code_to_letter(code: i16) -> u8 {
    let l = ROLE_LETTERS[(code % 8) as usize];
    if code / 8 == 0 {
        l
    } else {
        l.to_ascii_lowercase()
    }
}

/// Square index (0..15) -> UCI token "<file><rankdigit>" (file a..d, rankdigit 0..3).
fn square_to_uci(i: usize) -> String {
    let file = (b'a' + (i % 4) as u8) as char;
    let rank = (b'0' + (i / 4) as u8) as char;
    format!("{file}{rank}")
}

#[allow(dead_code)] // exercised by the UCI-coord unit test; kept for symmetry with move parsing
fn uci_to_square(s: &[u8]) -> Option<u8> {
    if s.len() != 2 {
        return None;
    }
    let file = s[0].to_ascii_lowercase().checked_sub(b'a')?;
    let rank = s[1].checked_sub(b'0')?;
    if file >= 4 || rank >= 4 {
        return None;
    }
    Some(file + rank * 4)
}

/// (from, to) -> UCI; a FLIP is from==to (e.g. "a0a0").
fn move_to_uci(m: (u8, u8)) -> String {
    format!("{}{}", square_to_uci(m.0 as usize), square_to_uci(m.1 as usize))
}

/// Parsed redacted position for the search. `first_color`/`ply` are reconstructed from
/// the FEN's `turn` + `movenum` so the masked state is bit-identical to the Python
/// serialization (`strategy._serialize`), guaranteeing an identical search.
pub struct Parsed {
    pub squares: [i16; 16],
    pub bag: [u32; 16],
    pub first_color: i16,
    pub ply: u32,
    pub no_progress: u32,
}

/// Parse a redacted Flip Jungle FEN. Returns None on any malformed field OR if the pool
/// count disagrees with the on-board face-down count (a fail-closed encoding-bug guard).
fn state_from_fen(fen: &str) -> Option<Parsed> {
    let parts: Vec<&str> = fen.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    // board: parts[0], 4 ranks, top (rank 4) first.
    let ranks: Vec<&str> = parts[0].split('/').collect();
    if ranks.len() != 4 {
        return None;
    }
    let mut squares = [engine::EMPTY; 16];
    let mut down_count = 0u32;
    for (i, rank_str) in ranks.iter().enumerate() {
        let rank = 4 - i; // parts[0] is rank 4 (top)
        let mut file = 0usize;
        for ch in rank_str.bytes() {
            if file >= 4 {
                return None;
            }
            let idx = file + (rank - 1) * 4;
            if ch.is_ascii_digit() {
                file += (ch - b'0') as usize;
                continue;
            } else if ch == b'X' {
                squares[idx] = engine::DOWN;
                down_count += 1;
            } else {
                squares[idx] = letter_to_code(ch)?;
            }
            file += 1;
        }
        if file != 4 {
            return None;
        }
    }
    // turn -> moving seat (0 red / 1 black / -1 unbound)
    let mover: i16 = match parts[1] {
        "r" => 0,
        "b" => 1,
        "-" => -1,
        _ => return None,
    };
    // pool -> bag[16]
    let mut bag = [0u32; 16];
    if parts[2] != "-" {
        let bytes = parts[2].as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            let code = letter_to_code(bytes[j])? as usize;
            j += 1;
            let mut n = 0u32;
            let mut saw_digit = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                n = n * 10 + (bytes[j] - b'0') as u32;
                j += 1;
                saw_digit = true;
            }
            if !saw_digit {
                return None;
            }
            bag[code] = n;
        }
    }
    // Σpool MUST equal the on-board face-down count (fail-closed).
    if bag.iter().sum::<u32>() != down_count {
        return None;
    }
    let no_progress: u32 = parts[3].parse().ok()?;
    let ply: u32 = if parts.len() >= 5 { parts[4].parse().ok()? } else { 0 };
    // Reconstruct first_color so that mover_color() == `mover` at this ply parity.
    let first_color: i16 = if mover < 0 {
        -1
    } else if ply % 2 == 0 {
        mover
    } else {
        1 - mover
    };
    Some(Parsed { squares, bag, first_color, ply, no_progress })
}

/// Re-encode a masked state to a redacted FEN (for tests + fen_vectors; the platform owns
/// the canonical encoder). Mirrors `state_from_fen` exactly.
#[allow(dead_code)] // round-trip helper for tests + fen_vectors generation
fn fen_from_state(p: &Parsed) -> String {
    let mut board = String::new();
    for i in 0..4usize {
        let rank = 4 - i; // top (rank 4) first
        let mut empties = 0;
        for file in 0..4usize {
            let c = p.squares[file + (rank - 1) * 4];
            if c == engine::EMPTY {
                empties += 1;
                continue;
            }
            if empties > 0 {
                board.push_str(&empties.to_string());
                empties = 0;
            }
            if c == engine::DOWN {
                board.push('X');
            } else {
                board.push(code_to_letter(c) as char);
            }
        }
        if empties > 0 {
            board.push_str(&empties.to_string());
        }
        if i + 1 < 4 {
            board.push('/');
        }
    }
    let mover = if p.first_color < 0 {
        -1
    } else if p.ply % 2 == 0 {
        p.first_color
    } else {
        1 - p.first_color
    };
    let turn = match mover {
        0 => "r",
        1 => "b",
        _ => "-",
    };
    let mut pool = String::new();
    for code in 0..16usize {
        if p.bag[code] > 0 {
            pool.push(code_to_letter(code as i16) as char);
            pool.push_str(&p.bag[code].to_string());
        }
    }
    if pool.is_empty() {
        pool.push('-');
    }
    format!("{board} {turn} {pool} {} {}", p.no_progress, p.ply)
}

fn state_of(p: &Parsed) -> engine::State {
    engine::State {
        sq: p.squares,
        bag: p.bag,
        first_color: p.first_color,
        ply: p.ply,
        no_progress: p.no_progress,
    }
}

/// Base seed for root tie-breaking. Unset `JF_TIE_SEED` means fresh per-search entropy,
/// so tied choices (e.g. the opening flip) vary out of the box. Set it to pin behavior:
/// `0` = legacy deterministic (first-ordered move), nonzero = reproducible variety.
fn tie_base_seed() -> u64 {
    if let Ok(s) = std::env::var("JF_TIE_SEED") {
        return s.parse::<u64>().unwrap_or(0);
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        .max(1)
}

fn search_best(
    p: &Parsed,
    node_budget: u64,
    rep_seed: &[u64],
    db: Option<engine::DbRef>,
    db_max: usize,
    base_seed: u64,
) -> String {
    let st = state_of(p);
    // Tie-break: exact-value ties among root moves (e.g. the opening flip, where all 16
    // tiles are equal) resolve via `base_seed` instead of always taking the first-ordered
    // move. `base_seed == 0` keeps legacy deterministic play (used by tests); otherwise it
    // is mixed with the position key so each position varies independently. The `go`
    // handler chooses the base (per-search entropy, or `JF_TIE_SEED` when pinned).
    let rng_seed = if base_seed == 0 {
        0
    } else {
        let mixed = base_seed ^ st.rep_key();
        if mixed == 0 { base_seed } else { mixed }
    };
    let (frm, to) = engine::best_move(
        &st,
        node_budget,
        CONTEMPT,
        W_MOB,
        DEFAULT_VALUES,
        MAX_DEPTH,
        db,
        db_max,
        DOM_TERM,
        REP_DETECT,
        WIN_DIST,
        rep_seed,
        rng_seed,
    );
    if frm == 255 {
        "(none)".to_string()
    } else {
        move_to_uci((frm, to))
    }
}

/// Parse the `reps` tail of a `position` line: ';'-delimited redacted FENs of positions
/// already seen twice this game. Each is hashed to its `rep_key`; unparseable entries are
/// skipped (fail-open — a bad seed only weakens repetition awareness, never the move).
fn parse_rep_seed(reps: &str) -> Vec<u64> {
    reps.split(';')
        .filter_map(|f| state_from_fen(f.trim()).map(|p| state_of(&p).rep_key()))
        .collect()
}

fn main() {
    // Resolve the exact-tablebase search leaf once at startup (prebuilt flat artifact
    // preferred; ≤2 in-process fallback). See DB_FALLBACK_PIECES / WIN_DIST.
    let (db, db_max) = init_db();
    let stdin = io::stdin();
    let mut current: Option<Parsed> = None;
    let mut current_reps: Vec<u64> = Vec::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        let cmd = line.split_whitespace().next().unwrap_or("");
        match cmd {
            "uci" => {
                println!("id name {ENGINE_NAME}");
                println!("id author Mistboard");
                println!("uciok");
            }
            "isready" => println!("readyok"),
            "ucinewgame" => {
                current = None;
                current_reps.clear();
            }
            "position" => {
                // "position fen <board> <turn> <pool> <clock> <movenum> [reps <fen>;<fen>;...]"
                // The `reps` tail (threefold game history) is optional; legacy `moves` is ignored.
                if let Some(rest) = line.strip_prefix("position") {
                    if let Some(fenpart) = rest.trim().strip_prefix("fen") {
                        let body = fenpart.split(" moves ").next().unwrap_or(fenpart);
                        let mut it = body.splitn(2, " reps ");
                        let fenstr = it.next().unwrap_or(body).trim();
                        current_reps = it.next().map(parse_rep_seed).unwrap_or_default();
                        current = state_from_fen(fenstr);
                    }
                }
            }
            "go" => {
                let mut movetime: Option<u64> = None;
                let mut nodes: Option<u64> = None;
                let mut t = line.split_whitespace().skip(1);
                while let Some(k) = t.next() {
                    match k {
                        "movetime" => movetime = t.next().and_then(|v| v.parse().ok()),
                        "nodes" => nodes = t.next().and_then(|v| v.parse().ok()),
                        _ => {}
                    }
                }
                // `nodes` is authoritative; `movetime` only tightens the ceiling.
                let mut budget = nodes.unwrap_or(DEFAULT_NODES);
                let mt = movetime.unwrap_or(DEFAULT_MOVETIME_MS);
                if movetime.is_some() || nodes.is_none() {
                    budget = budget.min(mt.saturating_mul(NODES_PER_MS).max(1));
                }
                let mv = match &current {
                    Some(p) => search_best(p, budget, &current_reps, db.as_ref().map(LeafDb::as_ref), db_max, tie_base_seed()),
                    None => "(none)".to_string(),
                };
                println!("bestmove {mv}");
            }
            "quit" => break,
            _ => {}
        }
        io::stdout().flush().ok(); // UCI drivers read line-by-line; flush each response
    }
}

#[cfg(test)]
mod fen_tests {
    use super::*;

    fn down(p: &Parsed) -> u32 {
        p.squares.iter().filter(|&&c| c == engine::DOWN).count() as u32
    }

    #[test]
    fn opening_all_face_down_round_trips() {
        // 16 face-down, unbound, full pool (one of each animal per ink), clock 0, ply 0.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let fen = format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0");
        let p = state_from_fen(&fen).expect("parse");
        assert_eq!(p.first_color, -1);
        assert_eq!(p.ply, 0);
        assert_eq!(p.no_progress, 0);
        assert!(p.squares.iter().all(|&c| c == engine::DOWN));
        assert_eq!(p.bag.iter().sum::<u32>(), 16);
        assert_eq!(down(&p), 16);
        assert_eq!(fen_from_state(&p), fen);
    }

    #[test]
    fn opening_tie_break_off_by_default_varies_with_seed() {
        // Every opening flip is an exact-value tie. rng_seed==0 (default; JF_TIE_SEED unset)
        // must keep legacy deterministic play: the first-ordered move, square 0. Nonzero seeds
        // must spread across tiles, and every pick must be a legal flip (from==to on a face-down
        // square) so tie-breaking never fabricates a move.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let fen = format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0");
        let st = state_of(&state_from_fen(&fen).expect("parse"));
        let pick = |seed: u64| {
            engine::best_move(
                &st, 512_000, CONTEMPT, W_MOB, DEFAULT_VALUES, MAX_DEPTH,
                None, 0, DOM_TERM, REP_DETECT, WIN_DIST, &[], seed,
            )
        };
        // default seed 0 is deterministic and legacy: square 0, repeatably.
        assert_eq!(pick(0), (0, 0));
        assert_eq!(pick(0), (0, 0));
        // nonzero seeds vary and stay legal flips.
        let mut seen = std::collections::HashSet::new();
        for seed in 1..=64u64 {
            let (f, t) = pick(seed);
            assert_eq!(f, t, "tie-break must return a flip (from==to)");
            assert_eq!(st.sq[f as usize], engine::DOWN, "flip must target a face-down tile");
            seen.insert(f);
        }
        assert!(seen.len() >= 2, "nonzero seeds should produce variety, got {seen:?}");
    }

    #[test]
    fn mixed_board_round_trips() {
        // rank4: red lion a4 then 3 empty; rank3: empty; rank2: black tiger b2, 1 face-down c2;
        // rank1: empty. Pool {red rat, black cat} -> 2 face-down (a-hum: only c2 here) ...
        // Keep the pool consistent with the single 'X': one face-down tile, pool size 1.
        // rank4 "L3", rank3 "4", rank2 "1tX1", rank1 "4". One X -> pool must be size 1.
        let fen = "L3/4/1tX1/4 r R1 3 8";
        let p = state_from_fen(fen).expect("parse");
        // a4 = idx 12 = red lion (code 0*8+6 = 6)
        assert_eq!(p.squares[12], 6);
        // b2 = idx 5 = black tiger (code 1*8+5 = 13)
        assert_eq!(p.squares[5], 13);
        // c2 = idx 6 = face-down
        assert_eq!(p.squares[6], engine::DOWN);
        assert_eq!(down(&p), 1);
        assert_eq!(p.bag.iter().sum::<u32>(), 1);
        assert_eq!(p.bag[0], 1); // red rat in the pool
        assert_eq!(p.no_progress, 3);
        assert_eq!(p.ply, 8); // even -> mover == first_color
        assert_eq!(p.first_color, 0); // turn 'r', ply even
        assert_eq!(fen_from_state(&p), fen);
    }

    #[test]
    fn pool_mismatch_is_rejected() {
        // Two 'X' on the board but a pool of size 1 -> fail-closed.
        assert!(state_from_fen("XX2/4/4/4 - R1 0 0").is_none());
    }

    #[test]
    fn odd_ply_reconstructs_first_color() {
        // turn 'b' at an ODD ply -> the FIRST mover (first_color) was red.
        let p = state_from_fen("L3/4/4/l3 b - 0 5").expect("parse");
        assert_eq!(p.ply, 5);
        assert_eq!(p.first_color, 0); // mover black, odd ply -> first_color = red
        // re-encode must round-trip the turn back to 'b'
        assert_eq!(fen_from_state(&p), "L3/4/4/l3 b - 0 5");
    }

    #[test]
    fn uci_coord_mapping() {
        assert_eq!(square_to_uci(0), "a0");
        assert_eq!(square_to_uci(3), "d0");
        assert_eq!(square_to_uci(12), "a3");
        assert_eq!(square_to_uci(15), "d3");
        assert_eq!(uci_to_square(b"a0"), Some(0));
        assert_eq!(uci_to_square(b"d3"), Some(15));
        assert_eq!(move_to_uci((0, 4)), "a0a1"); // a board move
        assert_eq!(move_to_uci((0, 0)), "a0a0"); // a flip
    }

    #[test]
    fn rep_seed_parses_semicolon_delimited_fens_and_skips_garbage() {
        // First FEN: a real endgame position; second: a lone-lions position. The middle entry
        // is junk and the trailing one has a pool/board mismatch (fail-closed) — both dropped.
        let seed = parse_rep_seed(
            "X1XX/3L/dl1X/1X2 r D1P1E1r1e1 13 23 ; not-a-fen ; L3/4/4/l3 b - 0 5 ; XX2/4/4/4 - R1 0 0",
        );
        assert_eq!(seed.len(), 2, "two valid FENs hashed, the junk + mismatch entries dropped");
        // Each key is the rep_key of the parsed position (clock-independent).
        let a = state_of(&state_from_fen("L3/4/4/l3 b - 0 5").unwrap()).rep_key();
        let a_diff_clock = state_of(&state_from_fen("L3/4/4/l3 b - 9 5").unwrap()).rep_key();
        assert_eq!(a, a_diff_clock, "rep_key ignores the no-progress clock");
        assert!(seed.contains(&a));
    }

    #[test]
    fn rep_seed_makes_the_engine_avoid_completing_a_threefold() {
        // A real endgame position from a drawn live game: Black (to move) shuffles a dog with
        // a0a1, re-entering a position the game has already seen. Q is that post-move position.
        let root = "X1XX/3L/1l1X/dX2 b D1P1E1r1e1 12 22";
        let q = "X1XX/3L/dl1X/1X2 r D1P1E1r1e1 13 23";
        let p = state_from_fen(root).expect("root");

        // History-blind (legacy): the engine plays the shuffle, blind to the repetition.
        let blind = search_best(&p, 512_000, &[], None, 0, 0);
        assert_eq!(blind, "a0a1", "without a rep seed the engine repeats");

        // Seed Q as already-seen: a0a1 now scores as a draw, so the engine must deviate.
        let seed = parse_rep_seed(q);
        let aware = search_best(&p, 512_000, &seed, None, 0, 0);
        assert_ne!(aware, "a0a1", "with the rep seed the engine avoids the threefold move");

        // A valid but unrelated seed must not perturb the move (no false positives).
        let unrelated = parse_rep_seed("L3/4/4/l3 b - 0 5");
        assert!(!unrelated.is_empty(), "control seed must actually parse");
        assert_eq!(search_best(&p, 512_000, &unrelated, None, 0, 0), "a0a1");
    }

    #[test]
    fn endgame_db_leaf_takes_the_drawn_trade_instead_of_fleeing() {
        // Two equal lions (red a1 to move, black a2 adjacent) is a dead draw whose only clean
        // finish is the mutual-KO trade a0a1. From a live game: the engine fled this trade
        // 13 times and ran to the repetition cap.
        let p = state_from_fen("4/4/l3/L3 r - 1 60").expect("two-lion endgame");

        // db=None (the shipped v0.2.0 behaviour): contempt makes the engine flee the trade.
        let fled = search_best(&p, 512_000, &[], None, 0, 0);
        assert_ne!(fled, "a0a1", "without the DB leaf the engine flees the trade");

        // ≤2 exact tablebase leaf: the dead draw is recognised, all moves tie, and move
        // ordering takes the trade — securing the draw immediately.
        let db = endgame::build(2).0;
        let secured = search_best(&p, 512_000, &[], Some(engine::DbRef::Map(&db)), 2, 0);
        assert_eq!(secured, "a0a1", "the DB leaf makes the engine take the drawn trade");
    }

    #[test]
    fn win_dist_converts_the_won_endgame() {
        // Black tiger d2 + rat b3 vs a lone red elephant d4, black to move: a forced black
        // win in 3 (rat b3-c3 covers both elephant escape squares; an elephant can't capture
        // a rat). Under flat WLD scoring every winning move tied and the engine shuffled this
        // to a repetition draw in a published self-play game; with WIN_DIST it must play the
        // trap immediately. UCI ranks are 0-indexed: b3-c3 is "b2c2".
        let p = state_from_fen("3E/1r2/3t/4 b - 0 47").expect("won 3-piece endgame");
        let db = endgame::build(2).0;
        assert_eq!(
            search_best(&p, 512_000, &[], Some(engine::DbRef::Map(&db)), 2, 0),
            "b2c2",
            "distance-aware scoring takes the shortest forced win"
        );
    }
}
