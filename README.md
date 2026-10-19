<div align="center">

<a href="https://fatsearch.com"><img src="docs/assets/banner.png" alt="FATSEARCH: the ripgrep of web searching for AI agents." width="800"></a>

Reads pages your agent's built-in fetch can't — JS, paywalls, big PDFs — through your own provider key,<br>
kept on disk as clean Markdown, navigated by outline and ids.

<br>

[![crates.io](https://img.shields.io/crates/v/fatsearch?style=flat-square&label=crates.io&color=f0883e&labelColor=161b22)](https://crates.io/crates/fatsearch)
[![npm](https://img.shields.io/npm/v/fatsearch?style=flat-square&label=npm&color=f0883e&labelColor=161b22)](https://www.npmjs.com/package/fatsearch)
[![release](https://img.shields.io/github/v/release/fatsearch/fatsearch?style=flat-square&label=release&color=3fb950&labelColor=161b22)](https://github.com/fatsearch/fatsearch/releases/latest)
[![license](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-79c0ff?style=flat-square&labelColor=161b22)](LICENSE-MIT)

<br>

<img src="docs/assets/race.svg" alt="Same question, same answer: fats puts 1,384 tokens in the agent's context, Firecrawl puts the whole page, 112,517." width="900">

same question · same answer · 81× less context<br>
**That's the point.**

</div>

<br>

## Every page your agent reads costs you this

<div align="center">

| <sub>Wikipedia · Rust · model input at $2 / M tokens (Claude Sonnet 5.5, 2026-10-19)</sub> | Tokens in context | Fetch | Model bill |
|:--|--:|--:|--:|
| Firecrawl | 112,517 | 1 credit | $0.23 |
| Bright Data Unlocker | 103,625 | 1 request | $0.21 |
| Oxylabs Web API | 102,609 | 1 request | $0.21 |
| Tavily Extract | 97,504 | 0.2 credit | $0.20 |
| Jina Reader | 94,415 | free | $0.19 |
| **fats** | **918** | **$0** | **$0.002** |

</div>

Your agent gets a short outline. It reads **one section** when it needs depth — not the whole page.<br>
Menus, cookie banners, navigation, footer links, tracking scripts — gone.<br>
Prices, dates, numbers — kept <sub>(loss guard counts every one)</sub>.

> [!TIP]
> The "free" fetch costs **~100×** more at the model. And it stays in context, every turn after.

<br>

## On hard pages

`fats` tries plain HTTP first (free). If the page is a JS shell, a captcha, or a wall,
it climbs to your providers — cheapest first, hedged, with cost tracking on every call.

```console
$ fats https://www.nytimes.com/ dry:true
nytimes.com/ fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s → new u1 · up to $0.0040

$ fats stats
u1 plain 283ms $0.0000 raw 1.1MB  → clean 40.9k~tok → outline 656~tok · 47 of 687 numbers pruned
u2 plain 600ms $0.0000 raw 34.2kB → clean 3.0k~tok  → outline 337~tok · 0 of 70 numbers pruned
```

<sub>`fats stats` is the marketing — real numbers, not claims.</sub>

**40 hard pages** <sub>social · news · shops · travel · classifieds · search — 2026-10-16, one run each</sub>

| | **fats** | Firecrawl | Tavily | Built-in fetch | Jina <sub>(no key)</sub> |
|:--|:-:|:-:|:-:|:-:|:-:|
| Answer in the returned text | **37** <sub>/40</sub> | 24 | 19 | 19 | 8 |
| Agent answered correctly | **27** <sub>/40</sub> | 24 | — | 18 | — |
| Input tokens per question | **2.6k** | 2.8k<sup>¹</sup> | — | 9.1k | — |

<sub><sup>¹</sup> Firecrawl's copy was cut to 1,000 tokens for this run. [Method →](docs/benchmarks.md#other-readers) · the table above: [one page, six readers →](docs/benchmarks.md#one-page-six-readers)</sub>

<br>

## Get started

<div align="center">

| <sub>1 · INSTALL</sub> | <sub>2 · KEYS <i>(optional)</i></sub> | <sub>3 · PLUG IN</sub> |
|:-:|:-:|:-:|
| `brew install fatsearch/tap/fatsearch` | `fats setup` | `claude mcp add fats -- fats mcp` |

<sub>or &nbsp;`npm i -g fatsearch`&nbsp; · &nbsp;`cargo install --locked fatsearch`&nbsp; · &nbsp;[every host →](docs/installation.md)</sub>

</div>

> [!NOTE]
> The package is `fatsearch`; the command is `fats` (like `ripgrep` → `rg`).
> **No account needed.** Without a provider key, `fats` reads what a plain request gets.
> With **any one key** — [Oxylabs](https://dashboard.oxylabs.io/en/registration) or [Bright Data](https://brightdata.com/cp/start) — it renders and unblocks pages through that provider.

<br>

## MCP

**One tool. One string. Same commands.**

```sh
claude mcp add fats -- fats mcp                                   # Claude Code
codex mcp add fats -- fats mcp                                    # Codex
{"mcpServers": {"fats": {"command": "fats", "args": ["mcp"]}}}   # Cursor, Desktop, …
```

<sub>The tool is `fats(cmd)` and its card is ~60 tokens. An agent that knows `fats u1#3` knows the whole tool.</sub>

<br>

## Commands

| Command | Does |
|:--|:--|
| `fats <url>` | fetch → clean → outline, cached as `u1` |
| `fats u1#3` | read section 3 |
| `fats u1 price` | filter inside the page |
| `fats u1 links` · `fats l4` | numbered links · follow one |
| `fats rust async runtime` | web search → results as ids |
| `fats gh rust markdown` | GitHub repos by stars |
| `fats ./report.pdf` | local file, one section per page |
| `fats stats` | every fetch: provider, ms, cost, tokens |
| `… dry:true` | what would run, what it would cost |

<sub>mods &nbsp;<samp>limit:20 &nbsp;country:de &nbsp;js:true &nbsp;cost:0.5 &nbsp;wait:30 &nbsp;fresh:true</samp> &nbsp;· &nbsp;one screen of all of it: `fats guide`</sub>

<br>

## For agents

An agent that can read a URL sets itself up: **`Read and execute https://fatsearch.com/agent-setup/SKILL.md`**.
The usage skill (how to navigate pages without flooding context, the rules learned the hard way) ships in this repo:

```sh
npx skills add fatsearch/fatsearch        # Agent Skills standard: Claude Code, Codex, Cursor, Copilot, Gemini CLI, …
gh skill install fatsearch/fatsearch      # the same through GitHub CLI ≥ 2.90
```

Docs as plain Markdown for agents: [fatsearch.com/llms.txt](https://fatsearch.com/llms.txt).

<br>

<div align="center">

<kbd> 17 MB </kbd> binary &nbsp;&nbsp; <kbd> 4 ms </kbd> start &nbsp;&nbsp; <kbd> 5 ms </kbd> cached read &nbsp;&nbsp; <kbd> $0 </kbd> plain pages &nbsp;&nbsp; <kbd> $0.001–0.004 </kbd> hard pages

<sub>no daemon · no browser · no telemetry · [every run →](docs/benchmarks.md)</sub>

</div>

<br>

<details>
<summary><b>What it doesn't do (yet)</b></summary>
<br>

- `match:near` / `match:topic` — needs a local model
- OCR for scanned PDFs → `! no_text`
- DOCX / XLSX → `! not_in_v1 zip`
- OS keychain — keys come from env or key files
- Remote MCP — stdio only
- Logins — a page behind a login stays behind it

Each prints a `!` row explaining what's missing and what to do. **Never silence.**
</details>

<br>

<div align="center">
<a href="https://fatsearch.com"><b>fatsearch.com</b></a>
<br><br>
<sub>
<a href="docs/reference.md">reference</a> ·
<a href="docs/benchmarks.md">benchmarks</a> ·
<a href="CHANGELOG.md">changelog</a> ·
<a href="SECURITY.md">security</a> ·
<a href="CONTRIBUTING.md">contributing</a>
<br><br>
MIT or Apache-2.0, at your option.
</sub>
</div>
