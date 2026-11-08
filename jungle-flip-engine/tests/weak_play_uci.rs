use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OPENING: &str = "XXXX/XXXX/XXXX/XXXX - R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1 0 0";

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
fn advertises_exact_options_and_release_version() {
    let output = run("uci\nisready\nquit\n", 0);
    assert!(output.contains("id name MistyJungleFlip 0.6.0\n"));
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("option "))
            .collect::<Vec<_>>(),
        [
            "option name Noise type string default 0",
            "option name NoiseDepth type spin default 1 min 1 max 8",
            "option name Seed type string default 0",
            "option name EvalFile type string default <empty>",
        ]
    );
    assert!(output.ends_with("uciok\nreadyok\n"));
}

#[test]
fn noisy_seed_replays_varies_and_options_survive_newgame() {
    let mut commands = String::new();
    for seed in 1..=8 {
        commands.push_str(&format!(
            "setoption name Noise value 0.3\nsetoption name NoiseDepth value 2\n\
             setoption name Seed value {seed}\nucinewgame\nposition fen {OPENING}\n\
             go nodes 1 movetime 0\ngo nodes 1 movetime 0\n"
        ));
    }
    commands.push_str("quit\n");
    let output = run(&commands, 999);
    assert!(!output.contains("invalid"), "{output}");
    let best: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("bestmove "))
        .collect();
    assert_eq!(best.len(), 16);
    for pair in best.chunks_exact(2) {
        assert_eq!(pair[0], pair[1]);
    }
    assert!(best.iter().collect::<std::collections::HashSet<_>>().len() > 1);
    for info in output.lines().filter(|line| line.starts_with("info ")) {
        assert_eq!(field(info, "depth"), Some("1"));
        assert!(field(info, "nodes").unwrap().parse::<u64>().unwrap() > 1);
    }
}

#[test]
fn zero_noise_matches_v052_moves_scores_depths_and_nodes() {
    // Recorded before the weak-play port, with the built-in two-piece tablebase.
    let positions = [
        (OPENING, None, "401", "0", "a0a0"),
        ("4/4/4/C1d1 r - 0 0", Some("24"), "48", "-992", "a0a1"),
        (
            "1XXe/1X2/1X2/C3 r D2r2 0 0",
            Some("2"),
            "401",
            "-351",
            "a0b0",
        ),
        ("3E/1r2/3t/4 b - 0 47", Some("4"), "401", "997", "b2c2"),
    ];
    for seed in [0, 1, 123] {
        let mut commands = String::from(
            "setoption name Noise value 0\nsetoption name NoiseDepth value 8\nucinewgame\n",
        );
        for (fen, ..) in positions {
            commands.push_str(&format!("position fen {fen}\ngo nodes 400\n"));
        }
        // A completed opening depth also exercises the existing exact-tie pass.
        commands.push_str(&format!("position fen {OPENING}\ngo nodes 4000\nquit\n"));
        let output = run(&commands, seed);
        let info: Vec<_> = output
            .lines()
            .filter(|line| line.starts_with("info "))
            .collect();
        assert_eq!(info.len(), 5, "{output}");
        for (line, (_, depth, nodes, cp, pv)) in info.iter().zip(positions) {
            assert_eq!(field(line, "depth"), depth);
            assert_eq!(field(line, "nodes"), Some(nodes));
            assert_eq!(field(line, "cp"), Some(cp));
            assert_eq!(field(line, "pv"), Some(pv));
        }
        assert_eq!(field(info[4], "depth"), Some("1"));
        assert_eq!(field(info[4], "nodes"), Some("4001"));
        assert_eq!(field(info[4], "cp"), Some("228"));
        let tied_move = if seed == 1 { "d1d1" } else { "a0a0" };
        assert_eq!(field(info[4], "pv"), Some(tied_move));
        let expected: Vec<_> = positions
            .iter()
            .map(|p| format!("bestmove {}", p.4))
            .chain(std::iter::once(format!("bestmove {tied_move}")))
            .collect();
        assert_eq!(
            output
                .lines()
                .filter(|line| line.starts_with("bestmove "))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn noisy_opening_depth_four_obeys_movetime() {
    let commands = format!(
        "setoption name Noise value 0.01\nsetoption name NoiseDepth value 4\n\
         setoption name Seed value 123\nposition fen {OPENING}\ngo movetime 1000\nquit\n"
    );
    let started = Instant::now();
    let output = run_with_timeout(&commands, 0, Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(2));
    let info = output
        .lines()
        .find(|line| line.starts_with("info "))
        .unwrap();
    let depth = field(info, "depth").unwrap().parse::<u32>().unwrap();
    assert!((1..4).contains(&depth), "{output}");
    assert!(field(info, "nodes").unwrap().parse::<u64>().unwrap() > 0);
    assert!(output
        .lines()
        .any(|line| line.starts_with("bestmove ") && line != "bestmove (none)"));
}

#[test]
fn noisy_opening_depth_four_obeys_nodes_without_movetime() {
    let commands = format!(
        "setoption name Noise value 0.01\nsetoption name NoiseDepth value 4\n\
         setoption name Seed value 123\nposition fen {OPENING}\ngo nodes 10000\nquit\n"
    );
    let output = run_with_timeout(&commands, 0, Duration::from_secs(2));
    let info = output
        .lines()
        .find(|line| line.starts_with("info "))
        .unwrap();
    let depth = field(info, "depth").unwrap().parse::<u32>().unwrap();
    assert!((1..4).contains(&depth), "{output}");
    assert_eq!(field(info, "nodes"), Some("10001"));
    assert!(output
        .lines()
        .any(|line| line.starts_with("bestmove ") && line != "bestmove (none)"));
}

#[test]
fn nonbinding_limits_preserve_seeded_noisy_replay() {
    let output = run(
        &format!(
            "setoption name Noise value 0.3\nsetoption name NoiseDepth value 2\n\
         setoption name Seed value 123\nposition fen {OPENING}\n\
         go nodes 10000000 movetime 10000\ngo nodes 10000000 movetime 10000\nquit\n"
        ),
        0,
    );
    let info: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("info "))
        .collect();
    assert_eq!(info.len(), 2);
    for name in ["depth", "nodes", "cp", "pv"] {
        assert_eq!(field(info[0], name), field(info[1], name), "{output}");
    }
    assert_eq!(field(info[0], "depth"), Some("2"));
    let best: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with("bestmove "))
        .collect();
    assert_eq!(best.len(), 2);
    assert_eq!(best[0], best[1]);
}
