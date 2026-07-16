//! WASM stub of the endgame tablebase.
//!
//! The real `flatdb` (in `jungle_flip_rust`) pulls in `rayon`, which does not compile to
//! `wasm32-unknown-unknown` (no threads). The in-browser client engine never uses a
//! tablebase — the search `db` is always `None` — so this stub only needs to satisfy
//! `engine::DbRef`'s reference to `FlatDB` plus the single method the search would call.
//! Both are unreachable at runtime in the wasm build.

pub struct FlatDB;

impl FlatDB {
    /// Never called: `DbRef::Flat` is only constructed from a loaded tablebase, and the
    /// wasm build always searches with `db == None`.
    pub fn value_dtm(&self, _board: &[i8; crate::game::NSQ], _stm: i8) -> (i8, u16) {
        unreachable!("wasm build never constructs a FlatDB (search db is always None)")
    }
}
