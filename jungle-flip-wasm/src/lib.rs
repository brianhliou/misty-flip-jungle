//! MistyJungleFlip WebAssembly shim — the in-browser client engine for Mistboard's Flip
//! Jungle review/analysis panel. Unlike the UCI binary (`jungle-flip-engine`), which emits a
//! single `bestmove` for PvE play, this build exposes the search core's **per-root-move exact
//! values** (`root_move_values`) so the browser panel can render live MultiPV: the top-K legal
//! moves, each with its own eval, single-shot.
//!
//! Redaction contract is identical to the UCI binary: the caller feeds a REDACTED Flip Jungle
//! FEN (face-down tiles as `X`, pool as public per-(ink,role) counts) — the engine never learns
//! a hidden tile's identity. The FEN parser (`engine::state_from_fen`) is the SAME one the UCI
//! binary uses, so the client and server build byte-identical masked states.
//!
//! Time source: `wasm32-unknown-unknown` has no monotonic clock, but the Flip Jungle engine
//! core drives search by NODE BUDGET only (no `Instant` in engine.rs), so nothing to shim.

// The engine core references `crate::{game, endgame, flatdb}`, so we mirror the crate module
// tree here. `flatdb` is a LOCAL STUB (the real one uses rayon → no wasm); the browser engine
// never uses a tablebase.
#[path = "../../jungle_flip_rust/src/game.rs"]
#[allow(dead_code)]
mod game;
#[path = "../../jungle_flip_rust/src/endgame.rs"]
#[allow(dead_code)]
mod endgame;
mod flatdb;
#[path = "../../jungle_flip_rust/src/engine.rs"]
#[allow(dead_code)] // engine.rs also exposes PyO3/UCI-facing entry points, unused here
mod engine;

use wasm_bindgen::prelude::*;

// Mirror the UCI binary's shipped search config (jungle-flip-engine/src/main.rs) so the client
// engine and the server engine evaluate positions identically.
const DEFAULT_VALUES: [f64; 8] = [6.0, 2.0, 3.0, 4.0, 5.0, 7.0, 8.0, 10.0]; // rat..elephant
const W_MOB: f64 = 0.8;
const CONTEMPT: f64 = 0.05;
const MAX_DEPTH: i32 = 24;
const DOM_TERM: bool = false;
const REP_DETECT: bool = true;
const WIN_DIST: bool = true;

/// Evaluate a redacted Flip Jungle FEN and return the top-`multipv` legal moves as JSON,
/// ranked best-first, each with an exact side-to-move centipawn score.
///
/// Returns `{"lines":[{"uci":"c1c1","cp":123,"depth":6},...]}` (a flip is `from==to`, e.g.
/// `"c1c1"`), or `{"error":"bad_fen"}` on a malformed FEN, or `{"lines":[]}` when there is no
/// legal move (terminal). `cp` is side-to-move POV (the browser normalizes to Red).
#[wasm_bindgen]
pub fn analyze(fen: &str, nodes: u32, multipv: u32) -> String {
    let parsed = match engine::state_from_fen(fen) {
        Some(p) => p,
        None => return "{\"error\":\"bad_fen\"}".to_string(),
    };
    let st = engine::state_of(&parsed);
    let ranked = engine::root_move_values(
        &st,
        nodes as u64,
        CONTEMPT,
        W_MOB,
        DEFAULT_VALUES,
        MAX_DEPTH,
        None, // no tablebase in the browser
        0,
        DOM_TERM,
        REP_DETECT,
        WIN_DIST,
        &[], // no repetition seed for a single-position analysis
    );
    // ranked: Vec<(from, to, value, depth_reached)>, already sorted descending by value.
    let take = (multipv.max(1) as usize).min(ranked.len());
    let mut out = String::from("{\"lines\":[");
    for (i, &(from, to, value, depth)) in ranked.iter().take(take).enumerate() {
        if i > 0 {
            out.push(',');
        }
        let uci = engine::move_to_uci((from, to));
        // Root value is side-to-move win-ness in ~[-1, 1]; ×1000 maps onto the platform's
        // centipawn win% curve (±1 ≈ decisive ≈ ±1000 cp), same as the UCI binary.
        let cp = (value.clamp(-1.0, 1.0) * 1000.0).round() as i64;
        out.push_str(&format!("{{\"uci\":\"{uci}\",\"cp\":{cp},\"depth\":{depth}}}"));
    }
    out.push_str("]}");
    out
}
