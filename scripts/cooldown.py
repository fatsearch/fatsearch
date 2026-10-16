#!/usr/bin/env python3
"""Dependency cooldown gate: every crates.io package that Cargo.lock gained since the base commit must have
been published at least 7 days ago. A fresh release is when a hijacked or typosquatted version does its harm,
and when it gets yanked (the arrayref compromise was caught within days). New file, replaces nothing: the
check did not exist.

    python3 scripts/cooldown.py            # base = HEAD~1 (the previous commit), lock = the working tree's
    COOLDOWN_BASE=main python3 scripts/cooldown.py

A package younger than 7 days fails unless a commit message in base..HEAD has the line
`cooldown-waiver: <crate> <why>`. The publish time is the registry index's `pubtime` (stable since cargo
#16369; not every version has one yet), else crates.io's `created_at`. An HTTP error (429, 5xx, 404) fails
the package like a young one: the gate fails closed. No network, or no base commit: a notice and exit 0. Prints one row per new package and a summary line.
"""

import datetime as dt
import json
import os
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request

DAYS = 7
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
UA = {"User-Agent": "fats-cooldown (dependency gate, local)"}


def packages(text):
    """Cargo.lock text -> {(name, version)} of crates.io packages."""
    lock = tomllib.loads(text)
    return {(p["name"], p["version"]) for p in lock.get("package", []) if p.get("source") == REGISTRY}


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=10) as r:
        return r.read().decode()


def index_path(name):
    """The sparse index path of a crate (doc.rust-lang.org/cargo/reference/registry-index.html, "Index files")."""
    n = name.lower()
    if len(n) <= 2:
        return f"{len(n)}/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


def published(name, version):
    """When name@version was published (aware datetime); raises URLError when offline."""
    for line in get(f"https://index.crates.io/{index_path(name)}").splitlines():
        row = json.loads(line)
        if row.get("vers") == version and row.get("pubtime"):
            return dt.datetime.fromisoformat(row["pubtime"].replace("Z", "+00:00"))
    meta = json.loads(get(f"https://crates.io/api/v1/crates/{name}/{version}"))
    return dt.datetime.fromisoformat(meta["version"]["created_at"].replace("Z", "+00:00"))


def git(*args):
    r = subprocess.run(["git", *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def main():
    base = os.environ.get("COOLDOWN_BASE", "HEAD~1")
    old = git("show", f"{base}:./Cargo.lock")
    if old is None:
        print(f"cooldown: no Cargo.lock at {base} (first commit or shallow clone): not checked")
        return 0
    with open("Cargo.lock") as f:
        new = sorted(packages(f.read()) - packages(old))
    log = git("log", "--format=%B", f"{base}..HEAD") or ""
    waived = {w.split()[0] for w in (l.split("cooldown-waiver:", 1)[1] for l in log.splitlines()
                                     if "cooldown-waiver:" in l) if w.split()}
    now, bad = dt.datetime.now(dt.timezone.utc), []
    for name, version in new:
        try:
            when = published(name, version)
        except urllib.error.HTTPError as e:  # crates.io answered: a 429, 5xx or 404 proves no age (fail closed)
            verdict = "waived" if name in waived else "FAIL"
            print(f"{verdict:6} {name} {version} crates.io http {e.code} {e.reason}: age unknown")
            bad.extend([f"{name}@{version}"] if verdict == "FAIL" else [])
            continue
        except (urllib.error.URLError, TimeoutError, OSError) as e:  # no network: a notice, CI runs it online
            print(f"cooldown: offline ({e.__class__.__name__}), {len(new)} new packages since {base}: not checked")
            return 0
        age = (now - when).total_seconds() / 86400
        verdict = "ok" if age >= DAYS else "waived" if name in waived else "FAIL"
        print(f"{verdict:6} {name} {version} published {when:%Y-%m-%d %H:%M}Z, {age:.1f} days ago")
        if verdict == "FAIL":
            bad.append(f"{name}@{version}")
    print(f"cooldown: {len(new)} new packages since {base}, {len(bad)} younger than {DAYS} days or of unknown age"
          + (f": {' '.join(bad)} (or a commit line `cooldown-waiver: <crate> <why>`)" if bad else ""))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
