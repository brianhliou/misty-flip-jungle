//! Public-state JFNN inference, quantized by default with search-local accumulators.
//! No private identities cross this adapter. The shipping search terminates a
//! line on its first repeated position; live heuristic leaves use repeat_count=1.
use self::{
    network::{Accumulator, Net},
    public::{Seat, State as PublicState},
};
use crate::engine::{IncrementalEvaluator, LeafEvaluator, State};
use std::{io, path::Path};

#[path = "nnue/features.rs"]
pub mod features;
#[path = "nnue/network.rs"]
pub mod network;
#[path = "nnue/public.rs"]
mod public;

pub struct Nnue {
    net: Net,
    float_reference: bool,
}
impl Nnue {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::new(Net::load(path)?)
    }
    pub fn new(net: Net) -> io::Result<Self> {
        if !net.can_quantize() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JFNN weights cannot be safely quantized",
            ));
        }
        Ok(Self {
            net,
            float_reference: false,
        })
    }
    /// Explicit export-oracle mode for checks and throughput comparisons.
    pub fn with_float_reference(mut self, enabled: bool) -> Self {
        self.float_reference = enabled;
        self
    }
    fn public(state: &State) -> PublicState {
        PublicState {
            board: state.sq,
            bag: state.bag,
            first_color: (state.first_color >= 0).then_some(state.first_color as u8),
            ply: state.ply,
            no_progress: state.no_progress,
            repeat_count: 1,
            history: Vec::new(),
        }
    }
    pub fn logit_reference(&self, state: &State) -> f32 {
        self.net.logit_reference(&Self::public(state))
    }
    /// Fresh evaluation; search uses the incremental session below by default.
    pub fn logit(&self, state: &State) -> f32 {
        if self.float_reference {
            return self.logit_reference(state);
        }
        let mut acc = Accumulator::default();
        self.net.refresh(&Self::public(state), &mut acc);
        self.net.logit_accumulator(&acc, mover(state))
    }
    pub fn evaluate(&self, state: &State) -> f64 {
        if self.float_reference {
            return self.net.utility_reference(&Self::public(state));
        }
        let mut acc = Accumulator::default();
        self.net.refresh(&Self::public(state), &mut acc);
        self.net.utility_accumulator(&acc, mover(state))
    }
}
fn mover(state: &State) -> Seat {
    if state.ply.is_multiple_of(2) {
        Seat::Red
    } else {
        Seat::Black
    }
}

/// Slots are allocated once per visited ply and reused for siblings/deepening.
/// Absolute-seat accumulators stay in place on pop, including aborted searches.
struct Line<'a> {
    net: &'a Net,
    stack: Vec<Accumulator>,
    ply: usize,
}
impl IncrementalEvaluator for Line<'_> {
    fn evaluate(&self, state: &State) -> f64 {
        self.net
            .utility_accumulator(&self.stack[self.ply], mover(state))
    }
    fn push(&mut self, child: &State) {
        if self.stack.len() == self.ply + 1 {
            self.stack.push(Accumulator::default());
        }
        let (parents, children) = self.stack.split_at_mut(self.ply + 1);
        self.net
            .update(&parents[self.ply], &Nnue::public(child), &mut children[0]);
        self.ply += 1;
    }
    fn pop(&mut self) {
        assert!(self.ply > 0, "cannot pop the NNUE root");
        self.ply -= 1;
    }
}
impl LeafEvaluator for Nnue {
    fn evaluate(&self, state: &State) -> f64 {
        self.evaluate(state)
    }
    fn start(&self, root: &State) -> Option<Box<dyn IncrementalEvaluator + '_>> {
        if self.float_reference {
            return None;
        }
        let mut acc = Accumulator::default();
        self.net.refresh(&Self::public(root), &mut acc);
        Some(Box::new(Line {
            net: &self.net,
            stack: vec![acc],
            ply: 0,
        }))
    }
}
