use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "../../jungle_flip_rust/tests/common/mod.rs"]
mod common;
use common::*;

fn run(commands: &str, tie_seed: u64) -> String {
    run_with_timeout(commands, tie_seed, Duration::from_secs(30))
}

fn run_with_timeout(commands: &str, tie_seed: u64, timeout: Duration) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_jungle-flip-engine"))
        .env("JF_TIE_SEED", tie_seed.to_string())
        .env_remove("JF_NO_ORDER_HEUR")
        .env_remove("JUNGLE_FLIP_TB")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(commands.as_bytes())
        .unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "UCI engine exceeded {timeout:?}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let words: Vec<_> = line.split_whitespace().collect();
    words
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1])
}

#[test]
fn default_and_explicit_empty_match_unmodified_v054_moves_scores_depths_and_nodes() {
    // Captured from the unmodified v0.5.4 executable before porting NNUE, 5000 nodes,
    // JF_TIE_SEED=0 and the built-in <=2 tablebase. Wall clock intentionally absent.
    let cases = [
        (OPENING, "1", "5001", "228", "a0a0"),
        ("4/4/4/C1d1 r - 0 0", "24", "48", "-992", "a0a1"),
        ("1XXe/1X2/1X2/C3 r D2r2 0 0", "4", "5001", "-394", "a0b0"),
        ("3E/1r2/3t/4 b - 0 47", "10", "5001", "997", "b2c2"),
        ("r2C/1W2/2p1/L2e b - 3 13", "6", "5001", "221", "d0c0"),
    ];
    for option in [
        "",
        "setoption name EvalFile value\n",
        "setoption name EvalFile value <empty>\n",
    ] {
        let mut commands = option.to_string();
        for (fen, ..) in cases {
            commands.push_str(&format!("position fen {fen}\ngo nodes 5000\n"));
        }
        commands.push_str("quit\n");
        let output = run(&commands, 0);
        let lines: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
        assert_eq!(lines.len(), cases.len(), "{output}");
        for (line, (_, depth, nodes, cp, pv)) in lines.iter().zip(cases) {
            for (key, expected) in [("depth", depth), ("nodes", nodes), ("cp", cp), ("pv", pv)] {
                assert_eq!(field(line, key), Some(expected), "{line}");
            }
        }
        assert_eq!(
            output
                .lines()
                .filter(|l| l.starts_with("bestmove "))
                .collect::<Vec<_>>(),
            cases
                .iter()
                .map(|c| format!("bestmove {}", c.4))
                .collect::<Vec<_>>()
        );
    }
}

fn scores(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|l| l.starts_with("info ") && !l.starts_with("info string"))
        .map(|l| field(l, "cp").unwrap())
        .collect()
}
#[test]
fn paths_with_spaces_loading_clearing_and_newgame_persistence() {
    let file = TestFile::new(&constant_bytes(0.5));
    let load = format!("setoption name eVaLfIlE value {}\n", file.0.display());
    let search = format!("position fen {OPENING}\ngo nodes 4000\n");
    let output=run(&format!("{search}{load}{search}ucinewgame\n{search}setoption name EvalFile value <empty>\n{search}{load}setoption name EvalFile value\n{search}quit\n"),0);
    let cp = scores(&output);
    assert_eq!(cp, ["228", "-245", "-245", "228", "228"], "{output}");
}

#[test]
fn malformed_or_missing_net_blocks_search_until_replaced_or_cleared() {
    let valid = TestFile::new(&constant_bytes(0.5));
    let good = random_bytes(7);
    let mut bads = vec![b"invalid JFNN".to_vec(), good[..good.len() - 1].to_vec()];
    for (offset, value) in [(4, 1u32), (8, 383), (12, 0), (16, 15), (20, 31)] {
        let mut b = good.clone();
        b[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        bads.push(b);
    }
    let mut unsafe_net = constant_bytes(1.);
    for i in 0..384 {
        unsafe_net[24 + 4 * i..28 + 4 * i].copy_from_slice(&2f32.to_le_bytes());
    }
    bads.push(unsafe_net);
    let mut nan = good.clone();
    nan[24..28].copy_from_slice(&f32::NAN.to_le_bytes());
    bads.push(nan);
    for bad in bads {
        let file = TestFile::new(&bad);
        assert_blocked(&valid.0, &file.0);
    }
    assert_blocked(&valid.0, &valid.0.with_extension("missing"));
}
fn assert_blocked(valid: &std::path::Path, bad: &std::path::Path) {
    let output=run(&format!("setoption name EvalFile value {}\nsetoption name EvalFile value {}\nisready\nposition fen {OPENING}\ngo nodes 4000\nucinewgame\nposition fen {OPENING}\ngo nodes 4000\nsetoption name EvalFile value {}\ngo nodes 4000\nsetoption name EvalFile value <empty>\ngo nodes 4000\nquit\n",valid.display(),bad.display(),valid.display()),0);
    assert_eq!(
        output
            .lines()
            .filter(|l| l.starts_with("info string error EvalFile:"))
            .count(),
        3,
        "{output}"
    );
    assert!(output.contains("readyok\n"));
    let moves: Vec<_> = output
        .lines()
        .filter(|l| l.starts_with("bestmove "))
        .collect();
    assert_eq!(moves.len(), 4, "{output}");
    assert_eq!(&moves[..2], ["bestmove (none)", "bestmove (none)"]);
    assert_eq!(scores(&output), ["-245", "228"], "{output}");
}

#[test]
fn both_evaluators_honor_node_and_wall_clock_limits() {
    let file = TestFile::new(&random_bytes(31));
    for option in [
        String::new(),
        format!("setoption name EvalFile value {}\n", file.0.display()),
    ] {
        let output=run(&format!("{option}position fen {OPENING}\ngo nodes 5000\ngo nodes 100000000 movetime 10\nquit\n"),0);
        let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
        assert_eq!(field(info[0], "nodes"), Some("5001"));
        assert!(field(info[1], "nodes").unwrap().parse::<u64>().unwrap() < 100000000);
        assert!(field(info[1], "time").unwrap().parse::<u64>().unwrap() < 1000);
    }
}

#[test]
fn noisy_net_uses_net_scores_seed_depth_and_guards_across_switches() {
    let file = TestFile::new(&constant_bytes(0.5));
    let mut commands =
        String::from("setoption name Noise value 0.3\nsetoption name NoiseDepth value 3\n");
    for seed in 1..=8 {
        commands.push_str(&format!("setoption name Seed value {seed}\nsetoption name EvalFile value {}\nposition fen {OPENING}\ngo nodes 1\nsetoption name EvalFile value <empty>\nucinewgame\nsetoption name EvalFile value {}\nposition fen {OPENING}\ngo nodes 1\n",file.0.display(),file.0.display()));
    }
    commands.push_str("position fen 4/4/4/Cr2 r - 0 0\ngo nodes 1\nquit\n");
    let output = run(&commands, 0);
    let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
    assert_eq!(info.len(), 17, "{output}");
    let mut choices = std::collections::HashSet::new();
    for pair in info[..16].chunks_exact(2) {
        for key in ["nodes", "depth", "cp", "pv"] {
            assert_eq!(field(pair[0], key), field(pair[1], key));
        }
        assert_eq!(field(pair[0], "depth"), Some("1"));
        assert_eq!(field(pair[0], "cp"), Some("-245"));
        assert_eq!(field(pair[0], "nodes"), Some("513"));
        choices.insert(field(pair[0], "pv").unwrap());
    }
    assert!(choices.len() > 1);
    assert_eq!(field(info[16], "pv"), Some("a0b0"));
    assert_eq!(field(info[16], "cp"), Some("1000"));
    // The same net also completes later noise passes when the budget allows.
    let output=run(&format!("setoption name EvalFile value {}\nsetoption name Noise value 0.3\nsetoption name NoiseDepth value 2\nposition fen {OPENING}\ngo nodes 10000000\nquit\n",file.0.display()),0);
    assert_eq!(field(output.lines().next().unwrap(), "depth"), Some("2"));
}

#[test]
fn external_net_search_is_deterministic_legal_and_deadline_bounded() {
    let Some(path) = external_net() else { return };
    let output=run(&format!("setoption name EvalFile value {}\nposition fen {OPENING}\ngo nodes 50000\ngo nodes 50000\ngo nodes 100000000 movetime 10\nquit\n",path.display()),0);
    assert!(!output.contains("info string error"), "{output}");
    let info: Vec<_> = output.lines().filter(|l| l.starts_with("info ")).collect();
    assert_eq!(info.len(), 3);
    for key in ["nodes", "depth", "cp", "pv"] {
        assert_eq!(field(info[0], key), field(info[1], key));
    }
    let mv = field(info[0], "pv").unwrap().as_bytes();
    assert_eq!(mv.len(), 4);
    assert_eq!(&mv[..2], &mv[2..]);
    assert!((b'a'..=b'd').contains(&mv[0]) && (b'0'..=b'3').contains(&mv[1]));
    assert_eq!(field(info[0], "nodes"), Some("50001"));
    assert!(field(info[2], "nodes").unwrap().parse::<u64>().unwrap() < 100000000);
}
