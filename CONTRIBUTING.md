# Contributing

Thanks for helping. Bug reports with the exact command, its output and the URL (or a trimmed fixture) are
the most useful thing you can send.

## Before you open a pull request

Every commit must pass `./check.sh` (fmt, clippy, tests, line budget; needs `cargo install --locked cargo-nextest`).
Before a pull request run `./check.sh full` (CI runs the test suite on Linux; the full gate runs here): it adds `cargo deny check`, the release build and its
size check. The toolchain is pinned in `rust-toolchain.toml`; any warning fails (`CARGO_BUILD_WARNINGS=deny`).
The line budget is the maintainer's gate, not a bar for contributors: a pull request that goes over it gets a
delete pass together with you, not a rejection.

Faster first build: in a new git worktree, `./check.sh` clones the main worktree's `target/` (APFS clonefile or
reflink: no copy, no extra disk) and touches every source file, so it skips a cold build of every dependency.
Never share one `target/` between worktrees: cargo names a path crate's artifacts relative to its workspace, so
every worktree would run another worktree's `fats` build.

The parity test (`tests/parity.rs`) compares the output with a private reference and skips unless `FATS_PARITY`
points at that reference's output. You do not need it to contribute.

- A wrong output is a wrong rule in the function that owns it: fix the rule and add the failing case as a
  test. Tests never touch the network.
- Errors are records (`!` rows with a code and a fix command), never panics. No `unwrap` on user input.
- Anything the tool does must be visible from the CLI (`fats`, `fats stats`, `fats config`, `dry:true`).
- Keep it small: before adding code, look for what can be deleted or merged.

## Dependencies

A new dependency (or a new direct use of one already in the tree) needs a maintainer's explicit go-ahead
before the pull request: open an issue with the crate name, version, licence and why it is needed.
`Cargo.lock` is committed; build with `--locked`.
The cooldown check in `./check.sh full` (`scripts/cooldown.py`) fails on a crate version under 7 days old unless it has a line in
`scripts/cooldown-waivers.txt` (`<crate> <version> <why>`; normally a version that fixes a security advisory).

## Commits

- [Conventional Commits](https://www.conventionalcommits.org/): `fix(pdf): keep table rows in order`.
  Imperative, at most 72 characters; the body explains why.
- One logical change per commit; the checks above pass at every commit.
- Every commit carries a `Signed-off-by` line (`git commit -s`, see Licence below).
- No AI-tool trailers or footers (`Co-authored-by` a tool, "Generated with …").

## Licence

Unless you say otherwise, anything you contribute is licensed under MIT OR Apache-2.0, like the rest of the
project (Apache-2.0 §5), and you grant the maintainer and their successors the right to sublicense and relicense
it. Sign off every commit (`git commit -s`) to certify that you have the right to submit it
([Developer Certificate of Origin](https://developercertificate.org)).
