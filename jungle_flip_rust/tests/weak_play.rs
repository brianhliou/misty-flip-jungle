use jungle_flip_rust::engine::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VALUES: [f64; 8] = [6., 2., 3., 4., 5., 7., 8., 10.];

fn position(pieces: &[(usize, i16)]) -> State {
    let mut st = State {
        sq: [EMPTY; 16],
        bag: [0; 16],
        first_color: 0,
        ply: 0,
        no_progress: 0,
    };
    for &(sq, code) in pieces {
        st.sq[sq] = code;
    }
    st
}

fn select(st: &State, play: PlayOptions, seed: u64) -> ((u8, u8), f64, SearchStats) {
    select_with_limits(st, play, seed, 400, None)
}

fn select_with_limits(
    st: &State,
    play: PlayOptions,
    seed: u64,
    nodes: u64,
    deadline: Option<Instant>,
) -> ((u8, u8), f64, SearchStats) {
    select_move_scored_stats(
        st,
        nodes,
        deadline,
        0.05,
        0.8,
        VALUES,
        4,
        None,
        0,
        false,
        true,
        true,
        &[],
        seed,
        &play,
        &mut PlayRng::new(seed),
    )
}

#[test]
fn win_in_one_overrides_noise_and_both_mix_branches() {
    let st = position(&[(0, 1), (1, 8)]); // cat captures the last black piece
    for seed in 0..64 {
        for play in [
            PlayOptions {
                noise: 100.,
                mix: 0.,
                ..Default::default()
            },
            PlayOptions {
                noise: 0.,
                mix: 1.,
                ..Default::default()
            },
            PlayOptions {
                noise: 0.,
                mix: 0.01,
                ..Default::default()
            },
        ] {
            let r = select(&st, play, seed);
            assert_eq!(r.0, (0, 1));
            assert_eq!(r.2.nodes, 0);
        }
    }
    // No-move wins are also recognized, with no capture required.
    let trapped = position(&[(10, 1), (11, 2), (15, 8)]);
    assert!(guarded_root_moves(&trapped).contains(&(10, 14)));
    let mut won = trapped.clone();
    won.push(10, 14, -1);
    assert_eq!(won.result(), RES_RED);
}

#[test]
fn loss_guard_applies_to_noise_random_and_search_branch_of_mix() {
    let st = position(&[(0, 1), (2, 10)]); // cat b0 loses to dog c0; a1 is safe
    assert_eq!(guarded_root_moves(&st), vec![(0, 4)]);
    for seed in 0..64 {
        for play in [
            PlayOptions {
                noise: 100.,
                mix: 0.,
                ..Default::default()
            },
            PlayOptions {
                noise: 0.,
                mix: 1.,
                ..Default::default()
            },
            PlayOptions {
                noise: 0.,
                mix: 0.01,
                ..Default::default()
            },
        ] {
            assert_eq!(select(&st, play, seed).0, (0, 4));
        }
    }
}

#[test]
fn flip_is_unsafe_if_even_one_reveal_allows_a_winning_reply() {
    let mut st = position(&[(0, 1), (1, DOWN), (15, DOWN)]);
    st.bag[8] = 1; // rat cannot capture cat
    st.bag[10] = 1; // dog can capture the last red piece
    let allowed = guarded_root_moves(&st);
    assert!(!allowed.contains(&(1, 1)));
    assert!(allowed.contains(&(15, 15)));
    let mut dog = st.clone();
    dog.push(1, 1, 10);
    dog.push(1, 0, -1);
    assert_eq!(dog.result(), RES_BLACK);
    let mut rat = st.clone();
    rat.push(1, 1, 8);
    assert_eq!(rat.result(), RES_ONGOING);
    for seed in 0..32 {
        assert_ne!(
            select(
                &st,
                PlayOptions {
                    noise: 10.,
                    mix: 0.,
                    ..Default::default()
                },
                seed
            )
            .0,
            (1, 1)
        );
        assert_ne!(
            select(
                &st,
                PlayOptions {
                    noise: 0.,
                    mix: 1.,
                    ..Default::default()
                },
                seed
            )
            .0,
            (1, 1)
        );
    }
}

#[test]
fn all_unsafe_falls_back_to_all_moves_and_mutual_elimination_is_a_draw() {
    let st = position(&[(0, 1), (5, 10)]);
    let mut legal = Vec::new();
    st.legal_moves(&mut legal);
    assert_eq!(guarded_root_moves(&st), legal);
    let mut seen = std::collections::HashSet::new();
    for seed in 0..32 {
        let r = select(
            &st,
            PlayOptions {
                noise: 0.,
                mix: 1.,
                ..Default::default()
            },
            seed,
        );
        assert!(legal.contains(&r.0));
        seen.insert(r.0);
    }
    assert_eq!(seen.len(), legal.len());
    let equal = position(&[(0, 1), (1, 9)]);
    assert!(guarded_root_moves(&equal).contains(&(0, 1)));
    let mut draw = equal.clone();
    draw.push(0, 1, -1);
    assert_eq!(draw.result(), RES_DRAW);
    // The guard must not mistake the trade for an immediate win.
    assert!(
        select(
            &equal,
            PlayOptions {
                noise: 1.,
                mix: 0.,
                ..Default::default()
            },
            1
        )
        .2
        .nodes
            > 0
    );
}

#[test]
fn zero_options_preserve_values_moves_depth_nodes_and_rng_streams() {
    let mut masked = position(&[(0, 1), (5, DOWN), (10, DOWN), (15, DOWN), (14, 15)]);
    masked.bag[2] = 2;
    masked.bag[8] = 1;
    for st in [
        State {
            sq: [DOWN; 16],
            bag: [1; 16],
            first_color: -1,
            ply: 0,
            no_progress: 0,
        },
        position(&[(0, 1), (2, 10)]),
        position(&[(0, 1), (1, 8)]),
        masked,
    ] {
        for seed in [0, 1, 123] {
            let old = best_move_scored_stats(
                &st,
                400,
                None,
                0.05,
                0.8,
                VALUES,
                4,
                None,
                0,
                false,
                true,
                true,
                &[],
                seed,
            );
            let r = select(
                &st,
                PlayOptions {
                    noise_depth: 8,
                    ..Default::default()
                },
                seed,
            );
            assert_eq!(
                (r.0, r.1.to_bits(), r.2.depth, r.2.nodes),
                (old.0, old.1.to_bits(), old.2.depth, old.2.nodes)
            );
            let mut rng = PlayRng::new(seed);
            let before = format!("{rng:?}");
            select_move_scored_stats(
                &st,
                400,
                None,
                0.05,
                0.8,
                VALUES,
                4,
                None,
                0,
                false,
                true,
                true,
                &[],
                seed,
                &PlayOptions::default(),
                &mut rng,
            );
            assert_eq!(before, format!("{rng:?}"));
        }
    }
}

#[test]
fn noisy_scores_are_full_window_depth_one_including_weighted_flips() {
    let mut st = position(&[(0, 1), (5, DOWN), (10, DOWN), (15, DOWN), (14, 15)]);
    st.bag[2] = 2;
    st.bag[8] = 1;
    let roots = root_move_values(
        &st,
        u64::MAX,
        0.05,
        0.8,
        VALUES,
        1,
        None,
        0,
        false,
        true,
        true,
        &[],
    );
    let play = PlayOptions {
        noise: 0.3,
        mix: 0.,
        ..Default::default()
    };
    let mut seen = std::collections::HashSet::new();
    for seed in 0..64 {
        let a = select(&st, play, seed);
        let b = select(&st, play, seed);
        assert_eq!((a.0, a.1, a.2.nodes), (b.0, b.1, b.2.nodes));
        assert_eq!(a.2.depth, 1);
        let v = roots.iter().find(|&&(f, t, _, _)| (f, t) == a.0).unwrap().2;
        assert!((a.1 - v).abs() < 1e-12);
        seen.insert(a.0);
    }
    assert!(seen.len() > 1);
}

#[test]
fn noise_depth_zero_is_depth_one_and_deeper_scores_match_a_full_root_pass() {
    let mut st = position(&[(0, 1), (5, DOWN), (10, DOWN), (15, DOWN), (14, 15)]);
    st.bag[2] = 2;
    st.bag[8] = 1;
    let at = |noise_depth| PlayOptions {
        noise: 0.3,
        noise_depth,
        ..Default::default()
    };
    for seed in 0..32 {
        let (zero, one) = (select(&st, at(0), seed), select(&st, at(1), seed));
        assert_eq!(
            (zero.0, zero.1, zero.2.nodes, zero.2.depth),
            (one.0, one.1, one.2.nodes, one.2.depth)
        );
    }
    let roots = root_move_values(
        &st,
        u64::MAX,
        0.05,
        0.8,
        VALUES,
        3,
        None,
        0,
        false,
        true,
        true,
        &[],
    );
    for seed in 0..32 {
        let (one, three) = (
            select(&st, at(1), seed),
            select_with_limits(&st, at(3), seed, u64::MAX, None),
        );
        assert_eq!(three.2.depth, 3);
        assert!(three.2.nodes > one.2.nodes);
        let v = roots
            .iter()
            .find(|&&(f, t, _, _)| (f, t) == three.0)
            .unwrap()
            .2;
        assert!((three.1 - v).abs() < 1e-12);
    }
    assert!(PlayOptions {
        noise_depth: 9,
        ..Default::default()
    }
    .validate()
    .is_err());
}

// Keep a runaway search from hanging the test process if either limit regresses.
fn opening_with_timeout(nodes: u64, movetime: Option<Duration>) -> ((u8, u8), f64, SearchStats) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let opening = State {
            sq: [DOWN; 16],
            bag: [1; 16],
            first_color: -1,
            ply: 0,
            no_progress: 0,
        };
        let result = select_with_limits(
            &opening,
            PlayOptions {
                noise: 0.01,
                noise_depth: 4,
                ..Default::default()
            },
            123,
            nodes,
            movetime.map(|duration| Instant::now() + duration),
        );
        let _ = tx.send(result);
    });
    rx.recv_timeout(Duration::from_secs(2))
        .expect("noisy opening search exceeded 2 seconds")
}

#[test]
fn noisy_opening_depth_four_obeys_movetime() {
    let started = Instant::now();
    let (m, value, stats) = opening_with_timeout(u64::MAX, Some(Duration::from_millis(1000)));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(m.0, m.1);
    assert!(m.0 < 16);
    assert!(value.is_finite());
    assert!((1..4).contains(&stats.depth));
    assert!(stats.nodes > 0);
}

#[test]
fn noisy_opening_depth_four_obeys_nodes_without_a_deadline() {
    let (m, value, stats) = opening_with_timeout(10_000, None);
    assert_eq!(m.0, m.1);
    assert!(m.0 < 16);
    assert!(value.is_finite());
    assert!((1..4).contains(&stats.depth));
    assert_eq!(stats.nodes, 10_001);
}

#[test]
fn nonbinding_limits_replay_the_same_noisy_move_and_value() {
    let mut st = position(&[(0, 1), (5, DOWN), (10, DOWN), (15, DOWN), (14, 15)]);
    st.bag[2] = 2;
    st.bag[8] = 1;
    let play = PlayOptions {
        noise: 0.3,
        noise_depth: 3,
        ..Default::default()
    };
    for seed in [1, 123, 456] {
        let deadline = Some(Instant::now() + Duration::from_secs(60));
        let a = select_with_limits(&st, play, seed, 10_000_000, deadline);
        let b = select_with_limits(&st, play, seed, 10_000_000, deadline);
        assert_eq!(
            (a.0, a.1.to_bits(), a.2.depth, a.2.nodes),
            (b.0, b.1.to_bits(), b.2.depth, b.2.nodes)
        );
        assert_eq!(a.2.depth, 3);
    }
}
