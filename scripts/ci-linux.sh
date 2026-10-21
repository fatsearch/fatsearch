#!/usr/bin/env bash
# The full gate on Linux in a local container (podman), wider than the public ci.yml test job: the steps (`cargo fetch --locked && cargo deny fetch`, then `scripts/nonet.sh ./check.sh full`
# with CARGO_NET_OFFLINE=true), the same tool versions, as a non-root user with passwordless sudo like the runner.
# Differences: this machine's architecture (arm64 on Apple silicon, CI is x86_64; no test depends on it, and
# scripts/size-gate has no Linux-aarch64 record, so the size gate reports "not checked"); Debian (the official
# rust image), not Ubuntu; no rust-cache, no git checkout (the files git lists, edits included, piped in).
# Cache: the volume fats-ci-linux (cargo home + target/); `podman volume rm fats-ci-linux` starts cold.
# Usage: scripts/ci-linux.sh   (a running podman machine; the image build needs the network once)
set -euo pipefail
cd "$(dirname "$0")/.."
t0=$SECONDS
image=localhost/fats-ci-linux:1.99.0
gh=https://github.com
podman build -q -t "$image" - > /dev/null << EOF
FROM docker.io/library/rust:1.99.0-trixie
RUN apt-get update && apt-get install -y --no-install-recommends sudo iproute2 cmake clang libclang-dev python3 \
 && rm -rf /var/lib/apt/lists/* && rustup component add rustfmt clippy && cd /usr/local/bin \
 && curl -fsSLo n.tgz $gh/nextest-rs/nextest/releases/download/cargo-nextest-0.9.148/cargo-nextest-0.9.148-aarch64-unknown-linux-gnu.tar.gz \
 && curl -fsSLo d.tgz $gh/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-aarch64-unknown-linux-musl.tar.gz \
 && printf '%s\n' '81469eb963ef883fd590e60189e77d6aba3d91b418591d3391f50fe3e9480d2b  n.tgz' \
    '995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d  d.tgz' | sha256sum -c \
 && tar -xzf n.tgz && tar -xzf d.tgz --strip-components 1 --wildcards '*/cargo-deny' && rm n.tgz d.tgz && useradd -m runner && echo 'runner ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/runner \
 && install -d -o runner -g runner /home/runner/w
ENV CARGO_HOME=/home/runner/w/cargo
EOF
# SYS_ADMIN + NET_ADMIN: nonet.sh makes a network namespace and brings its loopback up
git ls-files -z -co --exclude-standard | COPYFILE_DISABLE=1 tar --null -T - --no-xattrs -cf - |
    podman run --rm -i --user runner --cap-add SYS_ADMIN,NET_ADMIN -v fats-ci-linux:/home/runner/w "$image" bash -c '
        set -eu && mkdir -p ~/w/crate && cd ~/w/crate
        find . -mindepth 1 -maxdepth 1 ! -name target -exec rm -rf {} + && tar -xf - --no-same-owner
        cargo fetch --locked && cargo deny fetch
        CARGO_NET_OFFLINE=true scripts/nonet.sh ./check.sh full' && rc=0 || rc=$?
verdict=green && [ "$rc" = 0 ] || verdict="FAILED (rc $rc)"
echo "ci-linux: check.sh full on $image ($(uname -m)): $verdict in $((SECONDS - t0)) s"
exit "$rc"
