//! Sparse, public, relative-color features for JFNN v2.
use super::public::{Seat, State, DOWN, HEIGHT, NSQ, START_COUNTS, WIDTH};
pub const COVERED: usize = 256;
pub const BAG: usize = 272;
pub const CLOCK: usize = 288;
pub const REPEAT: usize = 327;
pub const TURN: usize = 328;
pub const UNBOUND: usize = 329;
pub const N_FEATURES: usize = 384; // 330..383 reserved
/// Conservative even for individually valid but unreachable snapshots:
/// 16 occupied points + 16 bag thresholds + 39 clock + repeat + turn + unbound.
pub const MAX_ACTIVE: usize = 74;
const WORDS: usize = N_FEATURES / 64;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureSet(pub(crate) [u64; WORDS]);
impl FeatureSet {
    fn insert(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    pub fn for_seat(s: &State, seat: Seat) -> Self {
        Self::from_public(
            &s.board,
            &s.bag,
            s.first_color,
            s.turn(),
            s.no_progress,
            s.repeat_count,
            seat,
        )
    }
    pub fn from_public(
        board: &[i16; NSQ],
        bag: &[u32; 16],
        first: Option<u8>,
        mover: Seat,
        clock: u32,
        repeat: u32,
        seat: Seat,
    ) -> Self {
        let own = first.map_or(0, |c| c ^ seat as u8) as usize;
        let mut out = Self::default();
        for (sq, &c) in board.iter().enumerate() {
            if c >= 0 {
                let c = c as usize;
                out.insert((c % 8 + 8 * usize::from(c / 8 != own)) * NSQ + sq);
            } else if c == DOWN {
                out.insert(COVERED + sq);
            }
        }
        let mut offset = BAG;
        for c in 0..16 {
            let absolute = (own ^ (c / 8)) * 8 + c % 8;
            for t in 1..=START_COUNTS[c % 8] {
                if bag[absolute] >= t {
                    out.insert(offset);
                }
                offset += 1;
            }
        }
        for t in 1..=39 {
            if clock >= t {
                out.insert(CLOCK + t as usize - 1);
            }
        }
        if repeat >= 2 {
            out.insert(REPEAT);
        }
        if mover == seat {
            out.insert(TURN);
        }
        if first.is_none() {
            out.insert(UNBOUND);
        }
        out
    }
    pub fn both(s: &State) -> [Self; 2] {
        [Seat::Red, Seat::Black].map(|seat| Self::for_seat(s, seat))
    }
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word, &bits)| {
            let mut b = bits;
            std::iter::from_fn(move || {
                if b == 0 {
                    return None;
                }
                let bit = b.trailing_zeros();
                b &= b - 1;
                Some(word * 64 + bit as usize)
            })
        })
    }
}
pub fn active(s: &State, seat: Seat) -> Vec<u16> {
    FeatureSet::for_seat(s, seat)
        .indices()
        .map(|x| x as u16)
        .collect()
}
/// D4: bit 2 transposes, then bit 0 mirrors files and bit 1 mirrors ranks.
pub fn reflect_point(sq: u8, k: u8) -> u8 {
    assert!(k < 8);
    let (x, y) = (sq as usize % WIDTH, sq as usize / WIDTH);
    let (x, y) = if k & 4 != 0 { (y, x) } else { (x, y) };
    ((if k & 2 != 0 { HEIGHT - 1 - y } else { y }) * WIDTH
        + if k & 1 != 0 { WIDTH - 1 - x } else { x }) as u8
}
/// Feature snapshot only: hashed repetition history cannot be reflected. The
/// public repeat count is preserved; never search this reflected snapshot.
pub fn reflect(s: &State, k: u8) -> State {
    let mut out = s.clone();
    out.history.clear();
    for sq in 0..NSQ {
        out.board[reflect_point(sq as u8, k) as usize] = s.board[sq];
    }
    out
}
