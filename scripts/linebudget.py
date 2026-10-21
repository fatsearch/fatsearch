#!/usr/bin/env python3
"""Line budget gate for src/: hand-written, non-test Rust code only.

Counted: non-blank lines of `src/**/*.rs` that still hold code once comments are removed.
Not counted: `#[cfg(test)]` items (inline modules, and the files of `#[cfg(test)] mod x;`),
`tests/`, and the generated `src/tables.rs`. String and char literals are blanked before
brace matching, so braces or `//` inside a string never count. A multi-line string counts
one line per source line, like any other code.

Shown, not counted: `src/data/*.json`, the built-in tables (data, not logic; `settings::TABLES`), as one `data`
line of non-blank JSON lines under TOTAL, so nothing moved out of Rust is hidden.

Exit 1 when a module exceeds its budget x 1.3, the total exceeds TOTAL_MAX, or counted code
carries `rustfmt::skip` (rustfmt is a gate; joined lines are not a cut).

Usage: python3 scripts/linebudget.py [--quiet]
"""

import re
import sys
from pathlib import Path

TOTAL_MAX = 10000  # raised only after a delete pass
SLACK = 1.3
GENERATED = {"tables.rs"}
# Per-module budgets (counted lines). They sum to <= TOTAL_MAX; a module may run to budget x SLACK
# as long as the total holds. TOTAL_MAX = the ceiling; it rises only after a delete pass. BUDGETS = module sizes
# when set; one grows 30% only if another shrinks.
BUDGETS = {
    "cache.rs": 477,
    "clean.rs": 901,
    "cli.rs": 1133,
    "fetch.rs": 786,
    "html.rs": 311,
    "lang.rs": 690,
    "lib.rs": 16,
    "main.rs": 13,
    "mcp.rs": 103,
    "outline.rs": 606,
    "pdf.rs": 98,
    "provider.rs": 708,
    "search.rs": 143,
    "settings.rs": 268,
    "setup.rs": 400,
    "stats.rs": 632,
    "telemetry.rs": 240,
    "text.rs": 98,
}

SRC = Path(__file__).resolve().parent.parent / "src"


def blank_literals(text: str) -> str:
    """Comments → removed (newlines kept); string/char literal bodies → 'x' (newlines kept)."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
        elif text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    if text[i] == "\n":
                        out.append("\n")
                    i += 1
        elif m := re.compile(r'b?r(#*)"').match(text, i):
            if i and (text[i - 1].isalnum() or text[i - 1] == "_"):
                out.append(c)
                i += 1
                continue
            end = text.find('"' + m.group(1), m.end())
            end = n if end < 0 else end + 1 + len(m.group(1))
            out.append(blank(text[i:end]))
            i = end
        elif c == '"' or text.startswith('b"', i) and not (i and text[i - 1].isalnum()):
            j = i + (2 if c == "b" else 1)
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(blank(text[i : j + 1]))
            i = j + 1
        elif c == "'" and (m := re.compile(r"'(\\(x..|u\{[0-9a-fA-F]+\}|.)|[^\\'])'").match(text, i)):
            out.append(blank(m.group(0)))
            i = m.end()
        else:
            out.append(c)
            i += 1
    return "".join(out)


def blank(s: str) -> str:
    return "".join("\n" if ch == "\n" else "x" for ch in s)


def item_end(code: str, start: int) -> int:
    """End offset of the item that starts at `start`: its matching `}`, or a `;` before any `{`."""
    depth = 0
    for k in range(start, len(code)):
        ch = code[k]
        if ch == ";" and depth == 0:
            return k + 1
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                return k + 1
    return len(code)


CFG_TEST = re.compile(r"#\[cfg\(test\)\]")
MOD_FILE = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")


def count(path: Path, skipped: set) -> tuple[int, bool]:
    """(counted lines, has rustfmt::skip in counted code); adds `#[cfg(test)] mod x;` files to `skipped`."""
    code = blank_literals(path.read_text(encoding="utf-8"))
    keep = list(code)
    for m in CFG_TEST.finditer(code):
        if mf := MOD_FILE.match(code, m.end()):
            skipped.add(path.parent / f"{mf.group(1)}.rs")
            skipped.add(path.parent / path.stem / f"{mf.group(1)}.rs")
        for k in range(m.start(), item_end(code, m.end())):
            if keep[k] != "\n":
                keep[k] = " "
    kept = "".join(keep)
    lines = [ln for ln in kept.split("\n") if ln.strip()]
    return len(lines), "rustfmt::skip" in kept


def main() -> int:
    quiet = "--quiet" in sys.argv
    skipped: set = set()
    files = sorted(p for p in SRC.rglob("*.rs") if p.name not in GENERATED)
    counts = {}
    fmt_skip = []
    for p in files:
        n, has_skip = count(p, skipped)
        counts[p] = n
        if has_skip:
            fmt_skip.append(p)
    fail = []
    total = 0
    rows = []
    for p in files:
        if p in skipped:
            continue
        name = str(p.relative_to(SRC))
        n = counts[p]
        total += n
        budget = BUDGETS.get(name)
        status = "ok"
        if budget is None:
            status = "NO BUDGET"
            fail.append(f"{name}: no budget row")
        elif n > budget * SLACK:
            status = f"OVER x{SLACK}"
            fail.append(f"{name}: {n} > {budget} x {SLACK} = {int(budget * SLACK)}")
        elif n > budget:
            status = "over (slack)"
        rows.append((name, n, budget, status))
    if sum(BUDGETS.values()) > TOTAL_MAX:
        fail.append(f"budgets sum {sum(BUDGETS.values())} > TOTAL_MAX {TOTAL_MAX}")
    if total > TOTAL_MAX:
        fail.append(f"TOTAL {total} > {TOTAL_MAX}")
    fail.extend(f"{p.relative_to(SRC)}: rustfmt::skip in counted code" for p in fmt_skip)
    if not quiet:
        for name, n, budget, status in rows:
            print(f"{name:14} {n:5} / {budget if budget is not None else '-':>5}  {status}")
    print(f"{'TOTAL':14} {total:5} / {TOTAL_MAX:5}  {'FAIL' if fail else 'ok'}")
    tables = sorted((SRC / "data").glob("*.json"))
    data = sum(1 for p in tables for ln in p.read_text(encoding="utf-8").splitlines() if ln.strip())
    print(f"{'data':14} {data:5}  {len(tables)} JSON tables in src/data, not in TOTAL")
    for f in fail:
        print(f"! {f}")
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main())
