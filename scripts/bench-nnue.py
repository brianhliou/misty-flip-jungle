#!/usr/bin/env python3
"""UCI search-only timings; set MISTY_FLIP_JUNGLE_TEST_NET to external weights."""
import os
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
POSITIONS = {
    "opening": "XXXX/XXXX/XXXX/XXXX - R1C1D1W1P1T1L1E1r1c1d1w1p1t1l1e1 0 0",
    "midgame": "1XXe/1X2/1X2/C3 r D2r2 0 0",
    "revealed": "r2C/1W2/2p1/L2e b - 3 13",
}
net = os.environ.get("MISTY_FLIP_JUNGLE_TEST_NET")
if not net:
    raise SystemExit("Set MISTY_FLIP_JUNGLE_TEST_NET to an external JFNN v2 file")
env = os.environ.copy()
env["JF_TIE_SEED"] = "0"
env.pop("JF_NO_ORDER_HEUR", None)
env.pop("JUNGLE_FLIP_TB", None)
# The executable's directory must not contain a prebuilt tablebase. The documented
# results use its built-in <=2 tablebase, excluding startup and model-load time.
if (ROOT / "target/release/jungle_flip_tb_4.bin").exists():
    raise SystemExit("Benchmark expects the built-in <=2 tablebase only")
with subprocess.Popen(
    [ROOT / "target/release/jungle-flip-engine"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    text=True, env=env,
) as child:
    def send(command):
        child.stdin.write(command + "\n")
        child.stdin.flush()

    def search(fen, nodes):
        send(f"position fen {fen}")
        send(f"go nodes {nodes}")
        info = None
        while True:
            line = child.stdout.readline()
            if not line:
                raise RuntimeError("Engine exited during benchmark")
            if line.startswith("info string error"):
                raise RuntimeError(line.strip())
            if line.startswith("info "):
                words = line.split()
                info = {key: words[words.index(key) + 1] for key in ["nodes", "time", "pv"]}
            if line.startswith("bestmove "):
                break
        assert info and int(info["nodes"]) == nodes + 1, info
        return int(info["time"])

    print("position,mode,nodes,median_ms,nodes_per_second")
    for nodes in [50_000, 1_000_000, 2_500_000]:
        for mode in (["classical", "net"] if nodes == 50_000 else ["net"]):
            send(f"setoption name EvalFile value {net if mode == 'net' else '<empty>'}")
            for name, fen in POSITIONS.items():
                search(fen, nodes)  # Warmup, not included in the median.
                elapsed = statistics.median(search(fen, nodes) for _ in range(5))
                assert elapsed > 0
                print(f"{name},{mode},{nodes},{elapsed},{(nodes + 1) * 1000 / elapsed:.0f}", flush=True)
    send("quit")
    child.wait(timeout=10)
    assert child.returncode == 0
