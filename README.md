<p align="center">
  <strong><code>fats</code></strong>
</p>

<h3 align="center">The <code>rg</code> of web fetching.</h3>

<p align="center">
  One binary. Your keys. Zero tracking.<br>
  Reads pages your agent's built-in fetch can't — JS, paywalls, big PDFs — through your own provider key,<br>
  kept on disk as clean Markdown, navigated by outline and ids.
</p>

<p align="center">
  <a href="LICENSE-MIT"><img alt="License" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=flat-square"></a>
  <a href="CHANGELOG.md"><img alt="Version" src="https://img.shields.io/badge/version-0.1.2-green?style=flat-square"></a>
</p>

---

## Every page your agent reads costs you this

```
Page                          Raw fetch          fats outline     You save
─────────────────────────────────────────────────────────────────────────────
Wikipedia "Rust" (the lang)   40,900 tokens      ~320 tokens       99%
Hacker News (370 comments)    143,600 bytes        358 tokens       96%
Rust Book (Installation)       30,700 bytes        352 tokens       82%
TikTok video page             ~41,000 visible      349 tokens       99%
Mastodon post (JS-rendered)     n/a (blocked)      330 tokens       ∞
```

Your agent gets a short outline. It reads **one section** when it needs depth — not the whole page.
Menus, cookie banners, navigation, footer links, tracking scripts — gone.
Prices, dates, numbers — kept (loss guard counts every one).

```
raw HTML  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━  40,900 tokens
fats      ━━                                          ~320 tokens
          └── agent reads one section: ~200-2k tokens
```

## Why

Tools that fetch pages dump the entire HTML-to-markdown conversion into your agent's context.
**That's the navigation, the sidebar, the cookie notice, the footer, the ads — all of it.**
Or the page is JS-rendered and they get nothing at all.

`fats` fixes both:

```
URL → fetch (plain → Oxylabs → Bright Data, automatic) → clean → outline → cache
```

The agent sees a short outline with section ids. It picks what it needs.
Every page is cached for up to an hour (30 min after its last read) — later reads are free and take milliseconds.

**Measured on 40 hard pages** (social, news, shops, travel, classifieds, search: pages a built-in fetch can't read), `fats` 0.1.0, 2026-10-16, one run each ([method](docs/benchmarks.md#other-readers)):

| | fats | Firecrawl | Tavily | Jina Reader (no key) | Built-in fetch |
|---|---|---|---|---|---|
| Pages read correctly | **92.5%** | 60.0% | 47.5% | 20.0% | 47.5% |
| Agent answered correctly | **27/40** | 24/40 | — | — | 18/40 |
| Input tokens per question | **2.6k** | 2.8k | — | — | 9.1k |

One static binary. **17 MB.** Starts in **4 ms.** No daemon, no browser, no telemetry.

## Get started

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/fatsearch/fatsearch/releases/latest/download/fatsearch-installer.sh | sh
npm i -g fatsearch                 # or npm ❓ (not published yet): the same release binary
cargo install --locked --path .    # or from a clone: Rust 1.99, cmake + C compiler + libclang (BoringSSL)
fats guide                         # the one-screen cheat sheet
fats setup                         # finds, checks, and saves your provider keys
```

No account needed. Without a provider key, `fats` reads what a plain request gets.
With **any one key** — [Oxylabs](https://dashboard.oxylabs.io/en/registration) or [Bright Data](https://brightdata.com/cp/start) — it renders and unblocks pages through that provider.

**CLI:** `fats <url>` · `fats <words>` · `fats u1#3` · `fats u1 price` · `fats stats`
**MCP:** `claude mcp add fats -- fats mcp` (same commands, one tool, ~60 token card)

Every host (Claude Code, Claude Desktop, Codex, Cursor, VS Code, Windsurf, Zed, Gemini CLI), prerequisites per OS,
keys, uninstall, troubleshooting: [docs/installation.md](docs/installation.md). An agent can follow it on its own.

## MCP

One tool. One string. Same commands.

```sh
fats mcp                           # stdio MCP server
```

```sh
# Claude Code
claude mcp add fats -- fats mcp

# Codex
codex mcp add fats -- fats mcp

# Cursor (.cursor/mcp.json)
{"mcpServers": {"fats": {"command": "fats", "args": ["mcp"]}}}
```

The tool is `fats(cmd)` — same commands as the CLI. The card is ~60 tokens.
An agent that knows `fats u1#3` knows the whole tool.

MCP Registry name: `mcp-name: io.github.fatsearch/fats`

## What it reads

| Format | How |
|---|---|
| **Web pages** | Plain HTTP → Oxylabs → Bright Data, automatic routing with hedge timing |
| **JS-rendered pages** | Provider-side rendering (Oxylabs or Bright Data) |
| **PDFs** | `pdf_oxide`, one section per page, text layer extraction |
| **Text files** | Plain text, CSV, Markdown, JSON, XML, RSS — served as-is with line ids |
| **Local files** | `fats ./file.html`, `fats ./report.pdf`, or stdin |

## Numbers

| Metric | Value |
|---|---|
| Binary | 17 MB (macOS arm64, stripped) |
| Startup | 4.1 ms (p50, `fats guide`) |
| Cached read | 5 ms |
| Cost per page | $0.001–0.004 (provider), $0 (plain/cached) |
| Token estimate | ±10% of Claude's count (measured on 42 reads) |
| 400-URL proof | 87.5% success, p50 1.3s, $0.32 total |

Method, every run, and the pages: [docs/benchmarks.md](docs/benchmarks.md).
What changed in each version: [CHANGELOG.md](CHANGELOG.md).

## Build

Prebuilt binaries (macOS, Linux, Windows; Windows and other ways in
[docs/installation.md](docs/installation.md#2-install-a-prebuilt-binary)), no toolchain needed:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/fatsearch/fatsearch/releases/latest/download/fatsearch-installer.sh | sh
```

From source:

```sh
cargo build --release --locked
./check.sh                         # fmt, clippy, tests, line budget (~15s warm)
./check.sh full                    # + cargo-deny, release build, binary size gate (~100s warm)
```

## All commands

```sh
fats https://example.com           # fetch → clean → outline, cached as u1
fats u1                            # outline again
fats u1#3                          # read section 3
fats u1 price                      # filter inside the page
fats u1 links                      # numbered links
fats l4                            # follow a link

fats rust async runtime            # web search → results as unfetched ids
fats gh rust markdown              # GitHub repos by stars

fats ./report.pdf                  # local PDF, one section per page
fats ./page.html                   # local HTML file

fats                               # status: session, cache, keys, provider health
fats stats                         # every fetch: provider, ms, cost, bytes → tokens, loss
fats config                        # every setting and where it came from
fats dry:true                      # what would run, what it would cost
fats mcp                           # stdio MCP server
```

## How it works

→ [docs/reference.md](docs/reference.md) — the full technical reference: fetch ladder,
content checking, cleaning rules, loss guard, islands, billing, settings, and every edge case.

## How it looks

```
$ fats https://doc.rust-lang.org/book/ch01-01-installation.html

= u1 Installation - The Rust Programming Language · 11 sections · 1.9ktok · plain 312ms $0
u1#1 Keyboard shortcuts (65) | Press ← or → to navigate…
u1#2 The Rust Programming Language (1.9k, 9 sub) | → Installation · Command Line…
u1#3 Installation (1.9k, 8 sub) | The first step is to install Rust…
u1#4 Command Line Notation (151) | In this chapter and throughout the book…
u1#5 Installing rustup on Linux or macOS (364) | If you're using Linux or macOS…
… +6 sections → fats u1 all
read: fats u1#2 · filter: fats u1 <words> · links: fats u1 links

$ fats u1#3

= u1#3 Installation · part 1/1 · cache 0s
The first step is to install Rust. We'll download Rust through rustup, a
command line tool for managing Rust versions and associated tools…

$ fats u1 rustc --version

= u1 filter "rustc version" · 1 hit · cache 0s
u1#7 $ rustc --version → You should see the version number, commit hash…
```

An agent calls `fats <url>`, scans the outline (~350 tokens), then reads one section (~200–2k tokens).
Not the whole page. **That's the point.**

### On hard pages

```
$ fats https://www.nytimes.com/ dry:true

nytimes.com/ fetch plain → oxy → bd ∥ oxyjs after 5s → new u1 · up to $0.0040
```

`fats` tries plain HTTP first (free). If the page is a JS shell, a captcha, or a wall,
it climbs to your providers — cheapest first, hedged, with cost tracking on every call.

```
$ fats stats

u1 plain  312ms $0.0000 raw 30.7kB → clean 1.9k~tok → outline 352~tok · 0 of 3 numbers pruned
u2 bd✓    4.2s  $0.0015 raw 89.1kB → clean 3.1k~tok → outline 401~tok · 0 of 12 numbers pruned
u3 oxyjs✓ 18.3s $0.0015 raw 156kB  → clean 4.8k~tok → outline 389~tok · 2 of 95 numbers pruned
```

Every fetch shows: which provider, how long, what it cost, how much was cleaned, how many
numbers were lost. `fats stats` is the marketing — real numbers, not claims.

## What it doesn't do (yet)

- `match:near` / `match:topic` — needs a local model → `! not_in_v1`
- OCR for scanned PDFs → `! no_text`
- DOCX / XLSX → `! not_in_v1 zip`
- OS keychain — keys come from env or key files
- Remote MCP — stdio only, no HTTP/OAuth
- Logins — a page behind a login stays behind it

Each prints a `!` row explaining what's missing and what to do. Never silence.

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

[SECURITY.md](SECURITY.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [CHANGELOG.md](CHANGELOG.md)
