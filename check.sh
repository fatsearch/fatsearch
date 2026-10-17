#!/usr/bin/env bash
# THE gate: every check runs, each prints ok/FAIL, exit 1 if any failed. Logs: target/check/<name>.log.
#   ./check.sh         quick, per commit: fmt · clippy (fake) · tests (fake, nextest) · line budget; one debug
#                      compile, no release
#   ./check.sh full    at task end and in CI: quick + clippy (shipped features) · deny · cooldown (skipped
#                      offline: CI runs it in its own job, with the network) · the release build · no fake hook in
#                      it · its size
# Parity (tests/parity.rs) needs a reference output that is not in this repository: it skips without FATS_PARITY.
set -u
cd "$(dirname "$0")" || exit 2
mode=${1:-quick}
case $mode in quick | full) ;; *) echo "usage: ./check.sh [quick|full]" >&2 && exit 2 ;; esac
# nextest runs every test binary at once, each test its own process (tests/wait.rs relies on it for speed)
if ! cargo nextest --version > /dev/null 2>&1; then
    echo "check.sh: cargo-nextest missing: cargo install --locked cargo-nextest" >&2
    exit 2
fi
# every warning (rustc, clippy, cached replays too) fails the build here, not in Cargo.toml [lints]
export CARGO_BUILD_WARNINGS=deny
# A new worktree's first gate builds every dependency (≈ 90 s quick, ≈ 6 min full). Instead: clone the main
# worktree's target/ (APFS clonefile / reflink: no copy, no extra disk), then touch every source file. Without the
# touch, the clone's newer `fats` artifacts count as fresh for this checkout's older files: stale binaries pass.
# Never one target/ shared by worktrees: cargo names a path crate's artifacts relative to its workspace, so every
# worktree maps to the same `fats` files and runs another worktree's build.
seed() {
    local from
    from="$(git worktree list --porcelain | awk 'NR == 1 { print $2 }')/$(git rev-parse --show-prefix)target"
    if [ -d target ] || [ ! -d "$from/debug" ]; then
        return 0
    fi
    echo "seed target/ ← $from (clone)"
    case $(uname -s) in
        Darwin) cp -cR "$from" target ;;
        *) cp -R --reflink=auto "$from" target ;;
    esac && find src tests -type f -exec touch {} +
}

t0=$SECONDS
seed
mkdir -p target/check
failed=()

gate() {
    local name=$1
    shift
    if "$@" > "target/check/$name.log" 2>&1; then
        echo "ok   $name"
    else
        echo "FAIL $name (target/check/$name.log)"
        tail -n 30 "target/check/$name.log" | sed 's/^/     | /' # CI keeps no files: the reason has to be in the output
        failed+=("$name")
    fi
}

no_fake_hook() {
    # neither the shipped binary nor its library (the release build above) carries the test hook: its symbol
    # (`provider4fake`; the rlib is bitcode, nm cannot read it, grep can) or, in the stripped binary, its header
    # literal (`x-fake-delay-ms`, src/provider.rs); a missing file fails, it is not a pass
    local f
    for f in target/release/fats target/release/libfats.rlib; do
        [ -f "$f" ] || { echo "missing $f" && return 1; }
        if LC_ALL=C grep -aq -e provider4fake -e x-fake-delay-ms "$f"; then
            echo "test hook in $f"
            return 1
        fi
    done
    echo "no test hook in target/release/fats or libfats.rlib"
}

online() {
    # CI runs this gate inside scripts/nonet.sh with CARGO_NET_OFFLINE set; locally, a 3 s probe of the index
    case ${CARGO_NET_OFFLINE:-false} in false | "") ;; *) return 1 ;; esac
    python3 -c 'import socket; socket.create_connection(("index.crates.io", 443), 3).close()' 2> /dev/null
}

binary_size() {
    # the shipped binary grows at most 5% past the size scripts/size-gate records for this platform
    # (`<bytes> <os>-<arch>` per line); a change worth more bytes re-records it, with the reason, in its commit
    local os rec size
    os=$(uname -sm | tr ' ' '-')
    rec=$(awk -v os="$os" '$2 == os { print $1 }' scripts/size-gate 2> /dev/null)
    size=$(wc -c < target/release/fats | tr -d ' ')
    if [ -z "$rec" ]; then
        echo "size $size B · no record for $os in scripts/size-gate: not checked"
        return 0
    fi
    echo "size $size B · recorded $rec B · limit $((rec * 105 / 100)) B ($os)"
    [ "$size" -le $((rec * 105 / 100)) ]
}

server_json() {
    # server.json (the MCP registry entry) names the crate's version at the top and in every package: a release
    # bumps it with Cargo.toml (nothing generates it)
    local v
    v=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
    echo "Cargo.toml $v · server.json: $(grep -o '"version": "[^"]*"' server.json | tr '\n' ' ')"
    [ -n "$v" ] && ! grep -o '"version": "[^"]*"' server.json | grep -vqx "\"version\": \"$v\""
}

gate fmt cargo fmt --check
gate clippy-fake cargo clippy --locked --all-targets --features fake
gate test cargo nextest run --locked --features fake --no-fail-fast
gate linebudget python3 scripts/linebudget.py
gate server-json server_json
if [ "$mode" = full ]; then
    gate clippy cargo clippy --locked --all-targets
    # CARGO_NET_OFFLINE set (CI, after `cargo deny fetch`): the advisory database already on disk
    gate deny cargo deny ${CARGO_NET_OFFLINE:+"--offline"} check
    if online; then
        gate cooldown python3 scripts/cooldown.py
    else
        echo "cooldown: skipped (offline; CI runs it in its own job)" > target/check/cooldown.log
        echo "skip cooldown (offline)"
    fi
    # one release build, then every check on it; a failed build skips them (they would check the previous build)
    gate release cargo build --locked --release
    if [[ " ${failed[*]-} " == *" release "* ]]; then
        rm -f target/check/no-fake-hook.log target/check/binary-size.log
        echo "skip no-fake-hook binary-size (no release build)"
    else
        gate no-fake-hook no_fake_hook
        gate binary-size binary_size
    fi
fi

grep -E "^(TOTAL|data)" target/check/linebudget.log
[ "$mode" = full ] && grep -shE "^(size|cooldown)" target/check/binary-size.log target/check/cooldown.log
if [ ${#failed[@]} -gt 0 ]; then
    echo "check.sh $mode: FAILED: ${failed[*]} ($((SECONDS - t0)) s)"
    exit 1
fi
echo "check.sh $mode: all green ($((SECONDS - t0)) s)"
