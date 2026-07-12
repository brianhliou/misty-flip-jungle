//! Flip Jungle endgame tablebases + (with the `pyext` feature, default) PyO3 bindings.
//!
//! The tablebase builder (`flatdb`, `game`) is pure Rust and also drives the standalone
//! `build_tb` binary (`--no-default-features`, no Python). The PyO3 surface — engine
//! search, parity hooks, HashMap DB — lives behind the `pyext` feature.
//!
//! Square encoding across the boundary matches the Python reference `endgame.poskey`:
//! a length-16 list of i8, -1 empty else color*8+role. `result`/`value` return
//! 1 (stm win) / 0 (draw) / -1 (stm loss) / 2 (ongoing or out-of-DB).

pub mod flatdb;
pub mod game;

#[cfg(feature = "pyext")]
mod endgame;
#[cfg(feature = "pyext")]
mod engine;

#[cfg(feature = "pyext")]
mod pyext {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use pyo3::prelude::*;

    use crate::game::{self, NSQ};
    use crate::{endgame, engine, flatdb};

    fn mk_masked(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> engine::State {
        let mut sq = [engine::EMPTY; engine::NSQ];
        for i in 0..engine::NSQ.min(squares.len()) {
            sq[i] = squares[i];
        }
        let mut b = [0u32; 16];
        for i in 0..16.min(bag.len()) {
            b[i] = bag[i];
        }
        engine::State { sq, bag: b, first_color, ply, no_progress }
    }

    static DB: Mutex<Option<HashMap<u128, (i8, u16)>>> = Mutex::new(None);
    static FLAT: Mutex<Option<flatdb::FlatDB>> = Mutex::new(None);

    /// Pick the search-leaf tablebase: the flat artifact when `db_flat`, else the HashMap.
    fn db_ref<'a>(
        db_max: usize, db_flat: bool,
        map_guard: &'a Option<HashMap<u128, (i8, u16)>>,
        flat_guard: &'a Option<flatdb::FlatDB>,
    ) -> Option<engine::DbRef<'a>> {
        if db_max == 0 {
            return None;
        }
        if db_flat {
            flat_guard.as_ref().map(engine::DbRef::Flat)
        } else {
            map_guard.as_ref().map(engine::DbRef::Map)
        }
    }

    fn to_board(v: &[i8]) -> [i8; NSQ] {
        let mut b = [game::EMPTY; NSQ];
        for i in 0..NSQ.min(v.len()) {
            b[i] = v[i];
        }
        b
    }

    fn vals8(values: Vec<f64>) -> [f64; 8] {
        let mut v = [0.0f64; 8];
        for i in 0..8.min(values.len()) {
            v[i] = values[i];
        }
        v
    }

    #[pyfunction]
    fn legal_moves(board: Vec<i8>, stm: i8) -> Vec<(u8, u8)> {
        let b = to_board(&board);
        let mut out = Vec::new();
        game::gen_moves(&b, stm, &mut out);
        out
    }

    #[pyfunction]
    fn apply_move(board: Vec<i8>, frm: u8, to: u8) -> Vec<i8> {
        let mut b = to_board(&board);
        game::apply(&mut b, frm as usize, to as usize);
        b.to_vec()
    }

    #[pyfunction]
    fn result_of(board: Vec<i8>, stm: i8) -> i8 {
        game::result(&to_board(&board), stm)
    }

    /// Build the retrograde tablebase up to `max_pieces`, store it globally, and return
    /// per-level stats `[(n, wins, losses, draws)]`.
    #[pyfunction]
    fn build_db(max_pieces: usize) -> Vec<(usize, usize, usize, usize)> {
        let (db, stats) = endgame::build(max_pieces);
        *DB.lock().unwrap() = Some(db);
        stats
    }

    /// Query the built DB: 1 / 0 / -1, or 2 if the position is out of range (or no DB).
    #[pyfunction]
    fn value(board: Vec<i8>, stm: i8) -> i8 {
        value_dtm(board, stm).0
    }

    /// Query the built DB with distance-to-terminal: `(wld, dtm)` — dtm in plies (0 for
    /// draws); `(2, 0)` if out of range or no DB.
    #[pyfunction]
    fn value_dtm(board: Vec<i8>, stm: i8) -> (i8, u16) {
        let key = endgame::key_of(&to_board(&board), stm);
        match &*DB.lock().unwrap() {
            Some(db) => *db.get(&key).unwrap_or(&(2, 0)),
            None => (2, 0),
        }
    }

    /// Build the memory-lean FLAT (perfect-index, 2-bit) tablebase up to `max_pieces`,
    /// store it globally, and return per-level stats `[(n, wins, losses, draws)]`.
    #[pyfunction]
    fn build_flat_db(max_pieces: usize) -> Vec<(u64, u64, u64, u64)> {
        let (db, stats) = flatdb::build(max_pieces);
        *FLAT.lock().unwrap() = Some(db);
        stats
    }

    /// Query the flat DB: 1 / 0 / -1, or 2 if out of range (or no DB built).
    #[pyfunction]
    fn flat_value(board: Vec<i8>, stm: i8) -> i8 {
        match &*FLAT.lock().unwrap() {
            Some(db) => db.value(&to_board(&board), stm),
            None => 2,
        }
    }

    /// Query the flat DB with distance-to-terminal: `(wld, dtm)`; `(2, 0)` if unavailable.
    #[pyfunction]
    fn flat_value_dtm(board: Vec<i8>, stm: i8) -> (i8, u16) {
        match &*FLAT.lock().unwrap() {
            Some(db) => db.value_dtm(&to_board(&board), stm),
            None => (2, 0),
        }
    }

    /// Per-level `(max_win_dtm, max_loss_dtm)` of the loaded flat DB (index = piece count).
    #[pyfunction]
    fn flat_dtm_stats() -> Vec<(u16, u16)> {
        match &*FLAT.lock().unwrap() {
            Some(db) => db.dtm_stats(),
            None => Vec::new(),
        }
    }

    /// Serialize the in-memory flat DB to disk (the format `load_flat_db` / the `build_tb`
    /// binary read). Errors if no flat DB has been built/loaded.
    #[pyfunction]
    fn save_flat_db(path: String) -> PyResult<()> {
        match &*FLAT.lock().unwrap() {
            Some(db) => db
                .save(&path)
                .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string())),
            None => Err(pyo3::exceptions::PyValueError::new_err("no flat DB to save")),
        }
    }

    /// Load a flat tablebase from disk into the global FLAT (e.g. a ≤5/≤6 artifact built on
    /// a server by `build_tb`). Returns the max piece count it covers.
    #[pyfunction]
    fn load_flat_db(path: String) -> PyResult<usize> {
        let db = flatdb::FlatDB::load(&path)
            .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
        let mp = db.max_pieces();
        *FLAT.lock().unwrap() = Some(db);
        Ok(mp)
    }

    // ── Masked-model parity surface (the playing engine's game model) ──────────────
    #[pyfunction]
    fn masked_legal_moves(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> Vec<(u8, u8)> {
        let st = mk_masked(squares, bag, first_color, ply, no_progress);
        let mut out = Vec::new();
        st.legal_moves(&mut out);
        out
    }

    #[pyfunction]
    #[allow(clippy::too_many_arguments)]
    fn masked_apply(
        squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32,
        frm: u8, to: u8, reveal: i16,
    ) -> (Vec<i16>, Vec<u32>, i16, u32, u32) {
        let mut st = mk_masked(squares, bag, first_color, ply, no_progress);
        st.push(frm as usize, to as usize, reveal);
        (st.sq.to_vec(), st.bag.to_vec(), st.first_color, st.ply, st.no_progress)
    }

    #[pyfunction]
    fn masked_result(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> i16 {
        mk_masked(squares, bag, first_color, ply, no_progress).result()
    }

    /// Unpruned-search value (no TT / no quiescence) for the Star1 equivalence test.
    #[pyfunction]
    #[allow(clippy::too_many_arguments)]
    fn search_value(
        squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32,
        depth: i32, w_mob: f64, values: Vec<f64>,
    ) -> f64 {
        let st = mk_masked(squares, bag, first_color, ply, no_progress);
        engine::search_value(&st, depth, w_mob, vals8(values))
    }

    /// Node-budgeted αβ + Star1 + TT search. Returns (from, to); (255,255) if no move.
    /// When `db_max > 0` and the global DB (built via `build_db`) covers it, fully-revealed
    /// ≤`db_max`-piece positions are scored from the exact tablebase instead of the heuristic.
    #[pyfunction]
    #[pyo3(signature = (squares, bag, first_color, ply, no_progress, node_budget, contempt,
                        w_mob, values, max_depth, db_max, dom_term, rep_detect,
                        win_dist=false, db_flat=false, rng_seed=0))]
    #[allow(clippy::too_many_arguments)]
    fn engine_best_move(
        squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32,
        node_budget: u64, contempt: f64, w_mob: f64, values: Vec<f64>, max_depth: i32,
        db_max: usize, dom_term: bool, rep_detect: bool, win_dist: bool, db_flat: bool,
        rng_seed: u64,
    ) -> (u8, u8) {
        let st = mk_masked(squares, bag, first_color, ply, no_progress);
        let map_guard = DB.lock().unwrap();
        let flat_guard = FLAT.lock().unwrap();
        let db = db_ref(db_max, db_flat, &map_guard, &flat_guard);
        engine::best_move(&st, node_budget, contempt, w_mob, vals8(values), max_depth, db, db_max, dom_term, rep_detect, win_dist, &[], rng_seed)
    }

    /// Root search VALUE (stm-perspective, in (-1,1)) at the deepest depth under the budget.
    /// Same search as `engine_best_move`; for position analysis (e.g. comparing candidate moves).
    #[pyfunction]
    #[pyo3(signature = (squares, bag, first_color, ply, no_progress, node_budget, contempt,
                        w_mob, values, max_depth, db_max, dom_term, rep_detect,
                        win_dist=false, db_flat=false))]
    #[allow(clippy::too_many_arguments)]
    fn engine_eval(
        squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32,
        node_budget: u64, contempt: f64, w_mob: f64, values: Vec<f64>, max_depth: i32,
        db_max: usize, dom_term: bool, rep_detect: bool, win_dist: bool, db_flat: bool,
    ) -> f64 {
        let st = mk_masked(squares, bag, first_color, ply, no_progress);
        let map_guard = DB.lock().unwrap();
        let flat_guard = FLAT.lock().unwrap();
        let db = db_ref(db_max, db_flat, &map_guard, &flat_guard);
        engine::search_eval(&st, node_budget, contempt, w_mob, vals8(values), max_depth, db, db_max, dom_term, rep_detect, win_dist)
    }

    /// Nodes to complete a fixed-depth search (for the move-ordering efficiency probe).
    #[pyfunction]
    fn search_nodes(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32,
                    depth: i32, w_mob: f64, values: Vec<f64>) -> u64 {
        let st = mk_masked(squares, bag, first_color, ply, no_progress);
        engine::search_nodes(&st, depth, w_mob, vals8(values))
    }

    #[pymodule]
    fn jungle_flip_rust(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_function(wrap_pyfunction!(masked_legal_moves, m)?)?;
        m.add_function(wrap_pyfunction!(masked_apply, m)?)?;
        m.add_function(wrap_pyfunction!(masked_result, m)?)?;
        m.add_function(wrap_pyfunction!(search_value, m)?)?;
        m.add_function(wrap_pyfunction!(engine_best_move, m)?)?;
        m.add_function(wrap_pyfunction!(engine_eval, m)?)?;
        m.add_function(wrap_pyfunction!(legal_moves, m)?)?;
        m.add_function(wrap_pyfunction!(apply_move, m)?)?;
        m.add_function(wrap_pyfunction!(result_of, m)?)?;
        m.add_function(wrap_pyfunction!(build_db, m)?)?;
        m.add_function(wrap_pyfunction!(value, m)?)?;
        m.add_function(wrap_pyfunction!(value_dtm, m)?)?;
        m.add_function(wrap_pyfunction!(flat_value_dtm, m)?)?;
        m.add_function(wrap_pyfunction!(flat_dtm_stats, m)?)?;
        m.add_function(wrap_pyfunction!(build_flat_db, m)?)?;
        m.add_function(wrap_pyfunction!(flat_value, m)?)?;
        m.add_function(wrap_pyfunction!(save_flat_db, m)?)?;
        m.add_function(wrap_pyfunction!(load_flat_db, m)?)?;
        m.add_function(wrap_pyfunction!(search_nodes, m)?)?;
        Ok(())
    }
}
