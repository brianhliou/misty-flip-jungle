mod common;
use common::*;
use jungle_flip_rust::{
    engine::*,
    nnue::{network::Net, Nnue},
};
use std::cell::Cell;

fn state(fen: &str) -> State {
    state_of(&state_from_fen(fen).unwrap())
}
fn synthetic() -> Nnue {
    Nnue::new(Net::from_bytes(&random_bytes(31)).unwrap()).unwrap()
}
fn select(
    st: &State,
    net: &dyn LeafEvaluator,
    nodes: u64,
    seed: u64,
) -> ((u8, u8), f64, SearchStats) {
    select_move_with_evaluator(
        st,
        nodes,
        None,
        0.05,
        0.8,
        VALUES,
        24,
        None,
        0,
        false,
        true,
        true,
        &[],
        seed,
        &PlayOptions::default(),
        &mut PlayRng::new(seed),
        Some(net),
    )
}

// Generate legal public trajectories; no training crate or private data required.
fn positions() -> Vec<State> {
    let mut out = Vec::new();
    for seed in 0..100 {
        let mut rng = Rng(seed + 100003);
        let mut st = state(OPENING);
        for _ in 0..240 {
            if st.result() != RES_ONGOING {
                break;
            }
            out.push(st.clone());
            if out.len() == 600 {
                return out;
            }
            let mut moves = Vec::new();
            st.legal_moves(&mut moves);
            let m = moves[rng.index(moves.len())];
            let reveal = if m.0 == m.1 {
                let codes: Vec<_> = (0..16).filter(|&c| st.bag[c] > 0).collect();
                codes[rng.index(codes.len())] as i16
            } else {
                -1
            };
            st.push(m.0 as usize, m.1 as usize, reveal);
        }
    }
    panic!("not enough positions")
}

fn drift(net: &Nnue) {
    let mut worst = 0f32;
    for (i, st) in positions().iter().enumerate() {
        let error = (net.logit(st) - net.logit_reference(st)).abs();
        worst = worst.max(error);
        assert!(error <= 0.25, "position {i}: logit error {error}");
    }
    eprintln!("600 public positions; max quantized logit error {worst}");
}
#[test]
fn synthetic_fast_and_float_reference_agree_on_600_positions() {
    drift(&synthetic());
}
#[test]
fn external_fast_and_float_reference_agree_on_600_positions() {
    let Some(path) = external_net() else { return };
    drift(&Nnue::load(path).unwrap());
}

fn hidden_swap(net: &Nnue) {
    let mut private: [i16; 16] = std::array::from_fn(|i| i as i16);
    let mask = |deal: &[i16; 16]| {
        let mut st = state(OPENING);
        for sq in [0, 5, 10, 15] {
            st.push(sq, sq, deal[sq]);
        }
        st
    };
    let before = mask(&private);
    private.swap(1, 2);
    let after = mask(&private);
    assert_eq!(
        net.evaluate(&before).to_bits(),
        net.evaluate(&after).to_bits()
    );
    let a = select(&before, net, 5000, 0);
    let b = select(&after, net, 5000, 0);
    assert_eq!(
        (a.0, a.1.to_bits(), a.2.nodes),
        (b.0, b.1.to_bits(), b.2.nodes)
    );
    // Negative control: accidentally exposing these identities changes nonconstant inference.
    let mut leaked_a = before.clone();
    let mut leaked_b = after.clone();
    leaked_a.sq[1] = 1;
    leaked_a.sq[2] = 2;
    leaked_b.sq[1] = 2;
    leaked_b.sq[2] = 1;
    assert_ne!(
        net.logit(&leaked_a).to_bits(),
        net.logit(&leaked_b).to_bits()
    );
}
#[test]
fn synthetic_hidden_swap_preserves_eval_and_fixed_node_move() {
    hidden_swap(&synthetic());
}
#[test]
fn external_hidden_swap_preserves_eval_and_fixed_node_move() {
    let Some(path) = external_net() else { return };
    hidden_swap(&Nnue::load(path).unwrap());
}

struct Checked<'a> {
    net: &'a Nnue,
    pushes: Cell<usize>,
    reveals: Cell<usize>,
    captures: Cell<usize>,
}
struct Line<'a> {
    owner: &'a Checked<'a>,
    inner: Box<dyn IncrementalEvaluator + 'a>,
    states: Vec<State>,
}
impl Line<'_> {
    fn check(&self) {
        let st = self.states.last().unwrap();
        assert_eq!(
            self.inner.evaluate(st).to_bits(),
            self.owner.net.evaluate(st).to_bits()
        );
    }
}
impl IncrementalEvaluator for Line<'_> {
    fn evaluate(&self, st: &State) -> f64 {
        let expected = self.states.last().unwrap();
        assert_eq!(
            (st.sq, st.bag, st.first_color, st.ply, st.no_progress),
            (
                expected.sq,
                expected.bag,
                expected.first_color,
                expected.ply,
                expected.no_progress
            )
        );
        self.check();
        self.inner.evaluate(st)
    }
    fn push(&mut self, child: &State) {
        let parent = self.states.last().unwrap();
        let count = |s: &State, down: bool| {
            s.sq.iter()
                .filter(|&&c| if down { c == DOWN } else { c >= 0 })
                .count()
        };
        self.owner.pushes.set(self.owner.pushes.get() + 1);
        if count(child, true) < count(parent, true) {
            self.owner.reveals.set(self.owner.reveals.get() + 1);
        }
        if count(child, false) < count(parent, false) {
            self.owner.captures.set(self.owner.captures.get() + 1);
        }
        self.inner.push(child);
        self.states.push(child.clone());
        self.check();
    }
    fn pop(&mut self) {
        self.inner.pop();
        self.states.pop();
        self.check();
    }
}
impl Drop for Line<'_> {
    fn drop(&mut self) {
        assert_eq!(self.states.len(), 1, "abort must restore root");
    }
}
impl LeafEvaluator for Checked<'_> {
    fn evaluate(&self, _: &State) -> f64 {
        panic!("must use incremental session")
    }
    fn start(&self, st: &State) -> Option<Box<dyn IncrementalEvaluator + '_>> {
        Some(Box::new(Line {
            owner: self,
            inner: self.net.start(st).unwrap(),
            states: vec![st.clone()],
        }))
    }
}
#[test]
fn every_search_push_pop_matches_refresh_including_aborts_and_noise() {
    let net = synthetic();
    let checked = Checked {
        net: &net,
        pushes: Cell::new(0),
        reveals: Cell::new(0),
        captures: Cell::new(0),
    };
    for st in positions().iter().step_by(40) {
        select(st, &checked, 500, 42);
        select_move_with_evaluator(
            st,
            500,
            None,
            0.05,
            0.8,
            VALUES,
            24,
            None,
            0,
            false,
            true,
            true,
            &[],
            0,
            &PlayOptions {
                noise: 0.3,
                noise_depth: 3,
                ..Default::default()
            },
            &mut PlayRng::new(42),
            Some(&checked),
        );
    }
    let st = state(OPENING);
    select_move_with_evaluator(
        &st,
        u64::MAX,
        Some(std::time::Instant::now()),
        0.05,
        0.8,
        VALUES,
        24,
        None,
        0,
        false,
        true,
        true,
        &[],
        42,
        &PlayOptions::default(),
        &mut PlayRng::new(42),
        Some(&checked),
    );
    assert!(checked.pushes.get() > 1000);
    assert!(checked.reveals.get() > 0);
    assert!(checked.captures.get() > 0);
}

#[test]
fn rejects_bad_magic_shape_size_weights_and_unsafe_quantization() {
    let good = random_bytes(7);
    for (offset, value) in [
        (0, 0),
        (4, 1),
        (8, 383),
        (12, 0),
        (12, 4097),
        (16, 15),
        (20, 31),
    ] {
        let mut bytes = good.clone();
        bytes[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
        assert!(Net::from_bytes(&bytes).is_err());
    }
    assert!(Net::from_bytes(&good[..good.len() - 1]).is_err());
    let mut extra = good.clone();
    extra.push(0);
    assert!(Net::from_bytes(&extra).is_err());
    for value in [f32::NAN, f32::INFINITY, 1e7] {
        let mut bytes = good.clone();
        bytes[24..28].copy_from_slice(&value.to_le_bytes());
        assert!(Net::from_bytes(&bytes).is_err());
    }
    let mut bytes = constant_bytes(1.);
    for i in 0..384 {
        bytes[24 + 4 * i..28 + 4 * i].copy_from_slice(&2f32.to_le_bytes());
    }
    assert!(Nnue::new(Net::from_bytes(&bytes).unwrap()).is_err());
}

#[test]
fn net_scale_chance_leaves_tablebase_terminals_and_win_clamp() {
    let net = Nnue::new(Net::from_bytes(&constant_bytes(0.5)).unwrap()).unwrap();
    let st = state(OPENING);
    let (_, v, stats) = select_move_with_evaluator(
        &st,
        100000,
        None,
        0.,
        0.8,
        VALUES,
        1,
        None,
        0,
        false,
        true,
        false,
        &[],
        0,
        &PlayOptions::default(),
        &mut PlayRng::new(0),
        Some(&net),
    );
    assert_eq!(stats.depth, 1);
    let expected = -(2. / (1. + (-0.5f64).exp()) - 1.);
    assert!((v - expected).abs() < 1e-12, "{v} != {expected}");
    let net = Nnue::new(Net::from_bytes(&constant_bytes(-8.)).unwrap()).unwrap();
    let (_, v, _) = select(&st, &net, 4000, 0);
    assert!((v - 0.85).abs() < 1e-12, "{v}");
    let st = state("4/4/4/L2c r - 0 0");
    let mut db = std::collections::HashMap::new();
    let mut moves = Vec::new();
    st.legal_moves(&mut moves);
    for m in moves {
        let mut child = st.clone();
        child.push(m.0 as usize, m.1 as usize, -1);
        db.insert(
            jungle_flip_rust::endgame::key_of(&child.sq.map(|c| c as i8), 1),
            (-1, 7),
        );
    }
    let (_, v, _) = select_move_with_evaluator(
        &st,
        10000,
        None,
        0.,
        0.8,
        VALUES,
        1,
        Some(DbRef::Map(&db)),
        2,
        false,
        true,
        true,
        &[],
        0,
        &PlayOptions::default(),
        &mut PlayRng::new(0),
        Some(&net),
    );
    assert!((v - 0.992).abs() < 1e-12, "TB must outrank NNUE: {v}");
    let win = state("4/4/4/Cr2 r - 0 0");
    assert_eq!(select(&win, &net, 1000, 0).0, (0, 1));
}

#[test]
fn reference_mode_is_explicit_and_search_matches_stateless_quantized() {
    struct Fresh<'a>(&'a Nnue);
    impl LeafEvaluator for Fresh<'_> {
        fn evaluate(&self, st: &State) -> f64 {
            self.0.evaluate(st)
        }
    }
    let net = synthetic();
    for st in positions().iter().step_by(60) {
        let a = select(st, &net, 5000, 42);
        let b = select(st, &Fresh(&net), 5000, 42);
        assert_eq!(
            (a.0, a.1.to_bits(), a.2.nodes, a.2.depth),
            (b.0, b.1.to_bits(), b.2.nodes, b.2.depth)
        );
    }
    let net = synthetic().with_float_reference(true);
    let st = state(OPENING);
    assert!(net.start(&st).is_none());
    assert_eq!(net.logit(&st).to_bits(), net.logit_reference(&st).to_bits());
}

#[test]
fn no_progress_and_seeded_repetition_still_override_net_leaves() {
    let net = Nnue::new(Net::from_bytes(&constant_bytes(-8.)).unwrap()).unwrap();
    let mut st = state("4/4/4/L2c r - 0 0");
    let mut moves = Vec::new();
    st.legal_moves(&mut moves);
    let keys: Vec<_> = moves
        .iter()
        .map(|m| {
            let mut child = st.clone();
            child.push(m.0 as usize, m.1 as usize, -1);
            child.rep_key()
        })
        .collect();
    let (_, v, _) = select_move_with_evaluator(
        &st,
        10000,
        None,
        0.05,
        0.8,
        VALUES,
        1,
        None,
        0,
        false,
        true,
        true,
        &keys,
        0,
        &PlayOptions::default(),
        &mut PlayRng::new(0),
        Some(&net),
    );
    assert_eq!(v, -0.05);
    st.no_progress = 39;
    assert_eq!(select(&st, &net, 10000, 0).1, -0.05);
}
