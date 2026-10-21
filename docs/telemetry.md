# Telemetry

fats counts how it is used. It does not look at what you use it for.

We see something like "fats 0.2.0 on macOS, 52 reads this week, 3 pages hit a captcha".
We don't see which pages, which searches, or who you are.

## Why
fats is free and has no accounts. Without a few numbers we can't tell which version breaks,
which provider fails, or which feature nobody uses. That's all this is for.
Low usage alone never decides that a feature goes.

## Never sent
- URLs, or anything you fetch or search
- search words
- page text, files, PDFs
- API keys (only the names of the providers you set up, like `oxy`)
- your IP address
- any id: no user id, no install id, no machine id, no hostname, no file paths

## Sent
- fatsearch version (and the previous one, once, after an upgrade)
- OS and CPU type
- which package you installed (npm, Homebrew, cargo, install script)
- the month you first ran fats, and how many of the last 7 days you used it
- the telemetry level (`full` or `minimal`)
- which tool runs fats (Claude Code, Codex, pi, a terminal, MCP; anything else counts as unknown)
- commands used, counted
- errors, counted by type
- which providers you set up, and how often each one worked (with or without page rendering) and was charged
- typical page size and speed
- crashes in our code, as a file and line (like `outline.rs:412`)

The first run prints this notice on stderr and sends nothing:

```text
~ anonymous usage stats on · what's sent: fats telemetry show · off: fats setup
```

It sends nothing, and so does every process already running when it
was printed (an MCP server, a batch). The update check goes at most once a day; it also tells you when
a new version is out. A check that fails is tried again at most 3 times that day, an hour apart.
The counts are only kept at level `full`, and go at most once a week, for the week before.

## Check it yourself
`fats telemetry show` prints what will be sent next and what was sent before.
`FATS_TELEMETRY_DEBUG=1` prints it instead of sending it. The code is open: src/telemetry.rs
(the update check and the sending) and `weekly_of` in src/stats.rs (the weekly counts). Everything
it keeps is in one folder, `telemetry/` in the fats cache: delete it and fats starts counting afresh.
If a field is ever added or changed, it's listed in the CHANGELOG.

## Turn it off
On a terminal: `fats setup → Settings → Telemetry` (or `fats setup telemetry`) and pick
`On (anonymous usage stats)` (= `full`), `Minimal (update check only)` or `Off`.
`fats setup telemetry minimal` keeps only the update check.
`fats setup telemetry off`, `FATS_TELEMETRY=0` or `DO_NOT_TRACK=1` turns everything off.
`FATS_OFFLINE=1` sends nothing either. Nothing is sent from CI (`CI`, `GITHUB_ACTIONS`, `BUILD_NUMBER`,
`TF_BUILD`, `TEAMCITY_VERSION`). `DO_NOT_TRACK`, `FATS_OFFLINE` and the CI names count as set with any
value except an empty one, `0`, `false`, `no` or `off`: `DO_NOT_TRACK=0` and `CI=false` leave it on.
`fats config` shows the level in effect and where it came from.

## Who sees what
We see the counts above, nothing you search. Pages fats opens on its own go straight from your
machine to the site. Pages it opens through Oxylabs, Bright Data or another provider you set up
are fetched by that provider, the same as using them directly. We don't sell or share anything.

Data is kept for a year, then deleted. The totals will be public at fatsearch.com/stats.

## The exact fields (schema 1)
The update check is one request. Its answer is kept only as a version `X.Y.Z` and one short line of
text (at most 200 characters, no control characters); anything else is dropped.

```text
GET https://fatsearch.com/v1/latest?s=1&v=0.2.0&os=macos&arch=arm64&via=brew&first=2026-10&d7=3&t=full
```

`s` schema · `v` version · `from` the previous version, only until the first check after an upgrade ·
`os` `macos`/`linux`/`windows` · `arch` `arm64`/`x86_64` · `via` `npm`/`brew`/`cargo`/`sh`/`unknown` ·
`first` the month of the first run (left out if the notice file holds no `YYYY-MM`) · `d7` days used of the last 7 (0–7) · `t` the level.

The weekly counts are one `POST https://fatsearch.com/v1/l2` with a JSON body:

| field | holds |
|---|---|
| `s` `v` `os` `arch` | as above |
| `week` | the ISO week counted, like `2026-W43` |
| `keys` | the providers with a key set, by name (`oxy`, `bd`, `decodo`) |
| `provider` | per provider: `calls`, `served` pages, `search` calls |
| `host` | commands per tool: `claude_code`, `codex`, `pi`, `mcp`, `shell`, `unknown` |
| `cmd` | commands per kind (`read`, `search`, `filter`, `outline`, `pdf`, …) |
| `tier` | per fetch tier (`plain`, `oxy`, `oxyjs`, `bd`, …): `ok`, `fail`, `billed`, `upto` (calls cut short, maybe charged) |
| `codes` | error codes from fats' own list, counted; any other code counts as `other` |
| `tok` | page and read size in tokens, `p50` and `p90` |
| `ms` | median fetch time, free and paid |
| `crash` | `file.rs:line kind` in our code, counted |

`tok` and `ms` are rounded to 2 significant digits (1,234 is sent as 1,200). No count goes over
1,000,000, no map over 64 entries, no more than 32 crash spots, and the body is at most 8,192 bytes. A week with no command sends nothing.
