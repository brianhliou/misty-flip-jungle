pub const NSQ: usize = crate::engine::NSQ;
pub const WIDTH: usize = crate::engine::W as usize;
pub const HEIGHT: usize = crate::engine::H as usize;
pub const DOWN: i16 = crate::engine::DOWN;
pub const START_COUNTS: [u32; 8] = [1; 8];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    Red,
    Black,
}
impl Seat {
    pub fn other(self) -> Self {
        match self {
            Self::Red => Self::Black,
            Self::Black => Self::Red,
        }
    }
}
#[derive(Clone, Debug)]
pub struct State {
    pub board: [i16; NSQ],
    pub bag: [u32; 16],
    pub first_color: Option<u8>,
    pub ply: u32,
    pub no_progress: u32,
    pub repeat_count: u32,
    /// Position keys since the last flip/capture, excluding the current state.
    pub history: Vec<u64>,
}
impl State {
    pub fn turn(&self) -> Seat {
        if self.ply.is_multiple_of(2) {
            Seat::Red
        } else {
            Seat::Black
        }
    }
}
