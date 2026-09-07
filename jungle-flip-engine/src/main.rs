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
//!   go [movetime <ms>] [nodes <n>]   -> search, emit
//!                                       "info depth <d> nodes <n> nps <x> time <ms> score cp
//!                                        <n> pv <uci>" then "bestmove <uci>" (or
//!                                       "bestmove (none)")
//!   quit                             -> exit
//!
//! The `depth`/`nodes`/`nps`/`time` fields in front of the score are what the search CONSUMED,
//! and they exist so the host's per-move decision artifact can compare consumption against the
//! budget it granted. Without them wall time is the only signal, and wall time cannot tell "the
//! search got slower" from "the host got slower" — a sibling engine measured the same binary on
//! the same position at 2,227 ms under loadavg 14.9 and 3,587 ms under loadavg 86.3. `nodes` is
//! therefore the real visited count, never the requested budget echoed back: a field that
//! repeats its own input looks like measurement and answers nothing. `depth` is the last
//! iterative-deepening iteration that COMPLETED, and is omitted (rather than reported as 0) when
//! the search was cut short before finishing one; `nps` is omitted when elapsed rounds to 0 ms.
//! Omitting a field we do not have beats inventing one.
//!
//! ── THE TWO SEARCH LIMITS ────────────────────────────────────────────────────────
//! `nodes` and `movetime` are INDEPENDENT limits and both are honored: whichever is reached
//! first ends the search.
//!
//!   go nodes N              work-bounded. No wall clock, so it is REPRODUCIBLE run to run
//!                           and reproduces the Python engine's `node_budget` exactly (see
//!                           fen_vectors.json + the golden test). `nodes` remains the
//!                           authoritative strength knob.
//!   go movetime X           time-bounded: the search samples a monotonic clock and stops
//!                           within a node-sampling interval of X ms. No work limit is
//!                           imposed, because the caller asked for a time-bounded search.
//!                           NOT reproducible run to run, by construction.
//!   go movetime X nodes N   both; the first limit reached wins.
//!   go                      the default node budget, no wall clock.
//!
//! `movetime` used to be converted into a node ceiling via a `NODES_PER_MS = 1_000_000`
//! constant — a claim of one billion nodes per second against a real 0.6M-8.2M, so the
//! ceiling only bound below about half a millisecond and `movetime` was inert: `go movetime
//! 300` and `go movetime 1200` searched the identical 512,001 nodes. The fix is not a better
//! constant. Any fixed nodes-per-ms figure is an assumption about how fast the host is, and
//! throughput here varies ~1.5x with host load and ~6x across position types, so it would be
//! wrong in one direction or the other essentially always — too loose on a busy host (blows
//! the deadline the caller is waiting on), too tight on a quiet one (wastes the budget).
//! A clock read costs nothing amortized over a sampling interval and makes no such claim.
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
use std::time::{Duration, Instant};

const ENGINE_NAME: &str = "MistyJungleFlip 0.5.2";
/// Node budget for a `go` that names neither limit. Deliberately NOT a time budget: a bare
/// `go` is the reproducible path the golden vectors depend on, and a wall clock would make it
/// host-dependent.
const DEFAULT_NODES: u64 = 512_000;

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
// FEN parse + UCI-coord helpers now live in the engine core (jungle_flip_rust::engine), so the
// wasm client build shares the SAME redacted-FEN parser as this UCI binary (no drift between the
// client engine and the server engine). Imported unqualified so existing call sites are unchanged.
#[allow(unused_imports)]
use engine::{
    code_to_letter, fen_from_state, letter_to_code, move_to_uci, square_to_uci, state_from_fen,
    state_of, uci_to_square, Parsed, ROLE_LETTERS,
};

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

/// Build the standard UCI search-info line, in the standard field order, so a generic parser
/// picks it up. Fields the search did not actually produce are omitted, never estimated.
fn search_info_line(stats: &engine::SearchStats, elapsed_ms: u64, cp: i64, pv: &str) -> String {
    let mut line = String::from("info");
    if stats.depth > 0 {
        line.push_str(&format!(" depth {}", stats.depth));
    }
    line.push_str(&format!(" nodes {}", stats.nodes));
    if elapsed_ms > 0 {
        line.push_str(&format!(" nps {}", stats.nodes.saturating_mul(1000) / elapsed_ms));
    }
    line.push_str(&format!(" time {elapsed_ms} score cp {cp} pv {pv}"));
    line
}

/// Parse a `go` line into the two search limits: `(node_budget, movetime_ms)`.
///
/// They are independent, and both are honored by the search — whichever is reached first
/// stops it. The one non-obvious rule is what an OMITTED `nodes` means, and it depends on
/// whether a `movetime` came with it:
///
///   * `movetime` present -> no work limit (`u64::MAX`). The caller asked for a time-bounded
///     search; quietly capping it at the 512k default is what let a "one billion nodes per
///     second" conversion constant hide for so long, since the default cap bound first in
///     every realistic case and the movetime never did.
///   * `movetime` absent  -> `DEFAULT_NODES`, and no deadline at all. This is the reproducible
///     path, and putting a wall clock on it would make a bare `go` host-dependent.
///
/// Unparseable or missing values fall back as if the token were absent (fail-open: a bad
/// `go` line still searches rather than refusing to move).
fn parse_go_limits(line: &str) -> (u64, Option<u64>) {
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
    let budget = nodes.unwrap_or(if movetime.is_some() { u64::MAX } else { DEFAULT_NODES });
    (budget, movetime)
}

#[allow(clippy::too_many_arguments)]
fn search_best(
    p: &Parsed,
    node_budget: u64,
    deadline: Option<Instant>,
    rep_seed: &[u64],
    db: Option<engine::DbRef>,
    db_max: usize,
    base_seed: u64,
) -> (String, i64, engine::SearchStats) {
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
    let ((frm, to), score, stats) = engine::best_move_scored_stats(
        &st,
        node_budget,
        deadline,
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
        ("(none)".to_string(), 0, stats)
    } else {
        // Root value is side-to-move win-ness in ~[-1, 1]; ×1000 maps it onto the platform's
        // centipawn win% curve (±1 ≈ decisive ≈ ±1000 cp). Clamp guards any terminal sentinel.
        let cp = (score.clamp(-1.0, 1.0) * 1000.0).round() as i64;
        (move_to_uci((frm, to)), cp, stats)
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
                // The clock starts when the `go` arrives, not after parsing: the caller's
                // movetime covers everything it is waiting on.
                let started = Instant::now();
                let (budget, movetime) = parse_go_limits(line);
                let deadline = movetime.map(|ms| started + Duration::from_millis(ms));
                match &current {
                    Some(p) => {
                        let (uci, cp, stats) = search_best(p, budget, deadline, &current_reps, db.as_ref().map(LeafDb::as_ref), db_max, tie_base_seed());
                        let elapsed_ms = started.elapsed().as_millis() as u64;
                        if uci == "(none)" {
                            println!("bestmove (none)");
                        } else {
                            // `info … score cp` is what whole-game analysis reads; bestmove
                            // alone drives PvE play. The depth/nodes/nps/time in front of the
                            // score are what the search CONSUMED, so the host can compare it
                            // against the budget it granted. Emit both lines.
                            println!("{}", search_info_line(&stats, elapsed_ms, cp, &uci));
                            println!("bestmove {uci}");
                        }
                    }
                    None => println!("bestmove (none)"),
                }
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
    fn search_info_reports_real_consumption_not_the_budget() {
        // `nodes` must be the search's own visited count. A field that echoes the caller's
        // budget back looks like measurement and answers nothing, so assert it lands NEAR the
        // cap without being derivable from it (tick counts, then aborts, hence budget + 1).
        // The opening is all chance nodes, so no budget this size finishes it.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let p = state_from_fen(&format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0")).expect("parse");
        let budget = 200_000u64;
        let ((frm, to), score, stats) = engine::best_move_scored_stats(
            &state_of(&p), budget, None, CONTEMPT, W_MOB, DEFAULT_VALUES, MAX_DEPTH, None, 0,
            DOM_TERM, REP_DETECT, WIN_DIST, &[], 0,
        );
        assert!(stats.nodes > budget / 2, "{} nodes", stats.nodes);
        assert!(stats.nodes <= budget + 1, "{} nodes", stats.nodes);
        assert!(stats.depth >= 1, "depth {} — at least one iteration completes", stats.depth);
        let cp = (score.clamp(-1.0, 1.0) * 1000.0).round() as i64;
        let line = search_info_line(&stats, 123, cp, &move_to_uci((frm, to)));
        assert!(
            line.starts_with(&format!("info depth {} nodes {} nps ", stats.depth, stats.nodes)),
            "{line}"
        );
        assert!(line.contains(&format!(" time 123 score cp {cp} pv ")), "{line}");
    }

    #[test]
    fn a_bigger_budget_is_actually_spent() {
        // Consumption tracks the budget rather than being a constant: raising the cap raises
        // the reported count. That is what makes the host's allowed-vs-consumed comparison
        // meaningful instead of decorative.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let p = state_from_fen(&format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0")).expect("parse");
        let st = state_of(&p);
        let run = |budget: u64| {
            engine::best_move_scored_stats(
                &st, budget, None, CONTEMPT, W_MOB, DEFAULT_VALUES, MAX_DEPTH, None, 0, DOM_TERM,
                REP_DETECT, WIN_DIST, &[], 0,
            )
            .2
            .nodes
        };
        let small = run(50_000);
        let large = run(400_000);
        assert!(large > small * 2, "{small} -> {large} nodes as the budget grew 8x");
    }

    #[test]
    fn search_info_omits_fields_the_search_did_not_produce() {
        // Nothing completed and no measurable elapsed: depth and nps are absent, not zeroed or
        // estimated. `nodes`, `time`, `score` and `pv` are always present.
        let stats = engine::SearchStats { nodes: 7, depth: 0 };
        assert_eq!(
            search_info_line(&stats, 0, -12, "a0a1"),
            "info nodes 7 time 0 score cp -12 pv a0a1"
        );
    }

    #[test]
    fn reported_nodes_exclude_the_tie_break_pass() {
        // The tie-break pass resets the node counter to give itself headroom, so a snapshot
        // taken after it would report the pass's work (small) instead of the search's. The
        // opening is a 16-way exact tie, so it always runs; both seeds must report the same,
        // budget-shaped count.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let p = state_from_fen(&format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0")).expect("parse");
        let st = state_of(&p);
        let nodes = |seed: u64| {
            engine::best_move_scored_stats(
                &st, 200_000, None, CONTEMPT, W_MOB, DEFAULT_VALUES, MAX_DEPTH, None, 0,
                DOM_TERM, REP_DETECT, WIN_DIST, &[], seed,
            )
            .2
            .nodes
        };
        assert_eq!(nodes(0), nodes(12345), "the tie-break pass must not perturb the count");
        assert!(nodes(12345) > 100_000, "{} nodes", nodes(12345));
    }

    // ── `go` limits ──────────────────────────────────────────────────────────────────

    #[test]
    fn a_bare_go_is_node_bounded_and_has_no_deadline() {
        // The reproducible path: the default node budget is the whole limit, and no wall
        // clock is attached to it (a clock would make a bare `go` host-dependent).
        assert_eq!(parse_go_limits("go"), (DEFAULT_NODES, None));
    }

    #[test]
    fn go_nodes_is_untouched_by_the_deadline_work() {
        assert_eq!(parse_go_limits("go nodes 1000"), (1000, None));
        assert_eq!(parse_go_limits("go nodes 512000"), (512_000, None));
    }

    #[test]
    fn movetime_is_never_converted_into_a_node_ceiling() {
        // THE regression. `movetime` used to be turned into a node budget via a fixed
        // nodes-per-millisecond constant, and 300 and 1200 both collapsed onto the same 512k
        // default cap: the two searches visited an identical 512,001 nodes and differed only
        // in wall time. So the guard is that the node budget must NOT be a function of the
        // requested time — the time is carried out of here as time, and honored as time.
        let (b300, mt300) = parse_go_limits("go movetime 300");
        let (b1200, mt1200) = parse_go_limits("go movetime 1200");
        assert_eq!(mt300, Some(300));
        assert_eq!(mt1200, Some(1200));
        assert_eq!(b300, b1200, "the node budget must not vary with the requested time");
        assert_eq!(b300, u64::MAX, "a time-bounded search gets no work limit of its own");
    }

    #[test]
    fn an_explicit_node_budget_still_binds_alongside_movetime() {
        // "Whichever is reached first": naming both keeps both.
        assert_eq!(parse_go_limits("go movetime 300 nodes 1000"), (1000, Some(300)));
        assert_eq!(parse_go_limits("go nodes 1000 movetime 300"), (1000, Some(300)));
    }

    #[test]
    fn an_unparseable_limit_falls_back_to_the_token_being_absent() {
        // Fail-open: a malformed `go` still searches rather than refusing to move.
        assert_eq!(parse_go_limits("go movetime"), (DEFAULT_NODES, None));
        assert_eq!(parse_go_limits("go nodes abc"), (DEFAULT_NODES, None));
    }

    #[test]
    fn a_movetime_search_stops_near_the_deadline_end_to_end() {
        // The whole path the `go` handler takes, including the post-search tie-break pass
        // (nonzero base seed), against a node budget that could never bind. The opening is
        // all chance nodes, so nothing here terminates on its own.
        let pool = "R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1";
        let p = state_from_fen(&format!("XXXX/XXXX/XXXX/XXXX - {pool} 0 0")).expect("parse");
        let started = Instant::now();
        let (uci, _cp, stats) = search_best(
            &p,
            u64::MAX,
            Some(started + Duration::from_millis(150)),
            &[],
            None,
            0,
            12345,
        );
        let elapsed = started.elapsed();
        assert_ne!(uci, "(none)", "a deadlined search still returns a move");
        // Loose upper bound: this asserts the deadline BINDS, not how precise it is. A
        // node-ceiling conversion would run to 512k nodes and blow straight past it; the
        // machine being busy is the only thing that should put us anywhere near 3 s.
        assert!(elapsed < std::time::Duration::from_secs(3), "elapsed {elapsed:?}");
        assert!(stats.nodes > 0, "the search ran");
        assert!(stats.nodes < u64::MAX, "the node budget did not bind");
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
        let blind = search_best(&p, 512_000, None, &[], None, 0, 0).0;
        assert_eq!(blind, "a0a1", "without a rep seed the engine repeats");

        // Seed Q as already-seen: a0a1 now scores as a draw, so the engine must deviate.
        let seed = parse_rep_seed(q);
        let aware = search_best(&p, 512_000, None, &seed, None, 0, 0).0;
        assert_ne!(aware, "a0a1", "with the rep seed the engine avoids the threefold move");

        // A valid but unrelated seed must not perturb the move (no false positives).
        let unrelated = parse_rep_seed("L3/4/4/l3 b - 0 5");
        assert!(!unrelated.is_empty(), "control seed must actually parse");
        assert_eq!(search_best(&p, 512_000, None, &unrelated, None, 0, 0).0, "a0a1");
    }

    #[test]
    fn endgame_db_leaf_takes_the_drawn_trade_instead_of_fleeing() {
        // Two equal lions (red a1 to move, black a2 adjacent) is a dead draw whose only clean
        // finish is the mutual-KO trade a0a1. From a live game: the engine fled this trade
        // 13 times and ran to the repetition cap.
        let p = state_from_fen("4/4/l3/L3 r - 1 60").expect("two-lion endgame");

        // db=None (the shipped v0.2.0 behaviour): contempt makes the engine flee the trade.
        let fled = search_best(&p, 512_000, None, &[], None, 0, 0).0;
        assert_ne!(fled, "a0a1", "without the DB leaf the engine flees the trade");

        // ≤2 exact tablebase leaf: the dead draw is recognised, all moves tie, and move
        // ordering takes the trade — securing the draw immediately.
        let db = endgame::build(2).0;
        let secured = search_best(&p, 512_000, None, &[], Some(engine::DbRef::Map(&db)), 2, 0).0;
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
            search_best(&p, 512_000, None, &[], Some(engine::DbRef::Map(&db)), 2, 0).0,
            "b2c2",
            "distance-aware scoring takes the shortest forced win"
        );
    }
}
