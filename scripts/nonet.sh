#!/usr/bin/env bash
# Run a command with no network but loopback: CI's build, clippy and test steps after `cargo fetch --locked`, so a
# build script or proc-macro that opens a socket or looks up a name fails. `--offline` alone only stops cargo
# itself from fetching. New file; replaces CI's false "builds offline, so no build script can reach the network".
#   macOS: sandbox-exec, outbound IP denied except localhost, the mDNSResponder socket denied (no name lookups).
#          Verified 2026-10-14 (macOS 26.5): curl and getaddrinfo fail, loopback works, the build and all tests pass.
#   Linux: a fresh network namespace (only `lo`, brought up) made as root, then the command as the calling user.
#          Root because Ubuntu 23.10+ AppArmor restricts unprivileged user namespaces (`unshare -rn`).
#          Not run yet: the first CI run on ubuntu-latest is its test.
# Usage: scripts/nonet.sh <command> [args…]
set -eu
case $(uname -s) in
    Darwin)
        exec sandbox-exec -p '(version 1)(allow default)
            (deny network-outbound (remote ip)) (allow network-outbound (remote ip "localhost:*"))
            (deny network-outbound (remote unix-socket (path-literal "/private/var/run/mDNSResponder")))' "$@"
        ;;
    Linux)
        # sudo resets PATH (secure_path): hand it over explicitly; -E keeps the rest (HOME, CARGO_*, FATS_*)
        exec sudo -E unshare --net -- bash -c 'ip link set lo up && exec sudo -E -u "$0" env PATH="$1" "${@:2}"' \
            "$(id -un)" "$PATH" "$@"
        ;;
    *)
        echo "nonet.sh: no network sandbox on $(uname -s); running with the network" >&2
        exec "$@"
        ;;
esac
