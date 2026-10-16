#!/usr/bin/env python3
"""Startup time of the release binary: p50/p90 wall ms over N runs per command, offline, in a scratch home.

posix_spawn + waitpid (subprocess.run adds ~5 ms of its own on macOS and hides the binary). `/usr/bin/true` is
timed the same way as the floor.
Usage: python3 scripts/startup.py [--bin target/release/fats] [--runs 50]
"""

import argparse
import os
import statistics
import tempfile
import time

COMMANDS = [["guide"], ["config"]]
QUIET = [(os.POSIX_SPAWN_OPEN, fd, "/dev/null", os.O_WRONLY, 0) for fd in (1, 2)]


def p50_p90(argv: list[str], env: dict, runs: int) -> tuple[float, float]:
    ms = []
    for _ in range(runs):
        t0 = time.perf_counter()
        os.waitpid(os.posix_spawn(argv[0], argv, env, file_actions=QUIET), 0)
        ms.append((time.perf_counter() - t0) * 1000)
    ms.sort()
    return statistics.median(ms), ms[int(len(ms) * 0.9)]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default="target/release/fats")
    ap.add_argument("--runs", type=int, default=50)
    args = ap.parse_args()
    home = tempfile.mkdtemp(prefix="fats-startup-", dir="/tmp")
    # a cache dir fats makes (marked, swept): `$HOME` itself is refused, which would time the `bad_setting` path
    cache = os.path.join(home, "cache")
    env = {"PATH": "/usr/bin:/bin", "HOME": home, "FATS_CACHE": cache, "FATS_KEY_DIR": home, "FATS_OFFLINE": "1"}
    print(f"bin {args.bin} · {os.path.getsize(args.bin) / 1e6:.2f} MB · {args.runs} runs each")
    for argv in [["/usr/bin/true"]] + [[args.bin, *cmd] for cmd in COMMANDS]:
        p50, p90 = p50_p90(argv, env, args.runs)
        print(f"{' '.join(argv[1:]) or argv[0]:14} p50 {p50:.2f} ms · p90 {p90:.2f} ms")


if __name__ == "__main__":
    main()
