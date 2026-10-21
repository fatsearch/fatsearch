# fats

Web pages and documents → small, clean, cached Markdown for AI agents.

`fats` turns a page into an outline with numbered sections and links. An agent reads only what it needs
(`u3#2`), filters inside a page (`u3 price`) and follows links (`l4`). Every page is cached, so later reads
cost nothing and take a few milliseconds. A page stays cached for 30 to 60 minutes
([Cache and ledger](#cache-and-ledger)).

> **Status: early development.** This build fetches and reads web pages (HTML), PDFs, text documents (plain
> text, CSV, Markdown, JSON, XML feeds and sitemaps) and local HTML/PDF files, and runs web searches
> (`fats <words>`, with an Oxylabs Web API key or a Bright Data key), on the command line and as a local MCP
> server. DOCX/XLSX conversion and OCR of scanned pages are coming in later releases.

One static binary (17 MB on macOS arm64 to 21 MB on Linux x86_64), `fats guide` starts in about 4.1 ms (p50 of 100
runs, the native binary; the npm shim adds Node's ~40 ms). No daemon,
no browser; anonymous usage counts and a daily update check, on by default, one command off
([Telemetry](#telemetry)). Provider keys are optional: without one, `fats` reads what a plain request gets; with
**any one** key (an Oxylabs Web API key, a Bright Data key **or** a Decodo token) it renders and unblocks pages through that
provider.

## Numbers

Measured on one Apple Silicon Mac (macOS arm64), release build of 0.2.0: the binary is 17.4 MB (17,405,936 bytes,
symbols stripped; Linux x86_64 in CI: 20.7 MB, 20,664,112 bytes) and `fats guide` starts in about 4.1 ms (p50 of
100 runs, three passes 4.07–4.13 ms, 2026-10-16; started through the npm shim, add Node's ~40 ms). On 19 pages a plain request does not get, one key served 12–16 with the Oxylabs key alone and 8–14
with the Bright Data key alone over nine runs on 2026-10-15, each on the pre-release build of that moment; the same
build moves by up to 3 pages between runs a minute apart, so read the band, not one run. The `Ntok` figure `fats`
prints on a whole read came to 1.01 real tokens per `Ntok` (5 whole reads of 42 counted with Anthropic's token counter, pre-release build).
Method, every run and the pages: [docs/benchmarks.md](docs/benchmarks.md). What changed in each version: [CHANGELOG.md](CHANGELOG.md).

## Install

From a clone of this repository (Rust 1.99; building needs `cmake` and a C compiler for BoringSSL):

```sh
cargo install --locked --path .
fats setup     # optional: add, check and switch your provider keys
fats guide
```

This installs one binary, `fats` (the package is `fatsearch`; from crates.io: `cargo install --locked fatsearch`).
Prebuilt binaries and their installers: [installation.md, section 2](installation.md#2-install-a-prebuilt-binary).

## Usage

```text
fats https://example.com    fetch, clean + outline the page, cache it as u1
fats rust async runtime     web search: one id per result, fetched only when opened (fats u1)
fats docs.rs tokio spawn    search within a site (same as site:docs.rs); since:7d adds a date hint
fats gh rust markdown       GitHub repositories by stars: u1 ★2.7k owner/repo · description · pushed date
fats ./page.html            clean + outline a local file (or `-` for stdin)
fats ./report.pdf           a PDF: one section per page (u1#4 = page 4), no network
fats ./report.pdf revenue   read the file, then filter it
fats u1                     outline again        fats u1#2      read section 2 (first part)
fats more                   next part            fats u1 all    every section, one line each
fats u1 links               numbered links       fats u1 price  filter inside the page
fats u1 links from:11       the next page of a list (links or media)
fats u1 media               the page's pictures   fats u1 m1     save picture 1, print its view cost
fats u1#2 vat match:exact   filter one section, exact phrase only
fats l4                     the link a fetch would follow
fats j8                     job 8: its command, when it ran, its pages
fats j8.u3                  a page from another session (job 8)
fats                        status: session, cache, keys (last 4 chars + source, never the key), providers
fats stats                  the session index: one row per search and fetch (host · title · provider ms cost · tok), last 20
fats stats rust limit:5     the rows whose host or title holds the words · `all` every row · `full` every field
fats config                 every setting and every threshold, and where each came from
fats setup                  a terminal: the setup menu; piped: check every saved key, $0, writes nothing
fats setup import-env       copy exported keys into auth.json (CI, SSH), checked, never over a saved key
fats setup oxy              add one key (also bd, decodo, gh): the menu on a terminal, else a local key form
printf %s "$KEY" | fats setup oxy   the same, no browser (SSH, CI): one piped line, checked ($0), saved
fats setup oxy off          turn a provider off, key kept (on = back; free = the plain fetch)
fats config set NAME VALUE  save a setting to settings.json (unset NAME = back to the built-in)
fats account                provider balance, spend this month, usage, zones (free reads, cached 15 min)
fats help                   the one-screen cheat sheet
fats guide                  how to use it well: the rules, the loop, the `=` line (~2.0k tokens)
fats telemetry show         what the update check and the weekly counts send next, and what was sent
fats setup telemetry off    turn them off (minimal = the update check only; full = both, the default)
fats mcp                    the same commands as a local MCP server (stdio)
```

Several inputs in one command (`fats a.html b.pdf https://…`) are one job: a `=` summary line first
(`= j4 3 inputs · 2 ok · 1 ! no_file`), then each input's outline. The command exits 0 when any input worked.

Modifiers are `key:value`: `match:exact|words|near|topic`, `dry:true` (show the plan and its price, spend
nothing), `json:true` (one JSON record per input, so `fats a.html b.pdf json:true` prints two lines carrying
`i`; `FATS_JSON=1` does the same; beside a page id or file a trailing bare `json` is the same mod, after search
words it is a word: use `json:true`), `limit:20` (at most
this many links or filter hits; lists stop at ~300 tokens anyway), `max:1k` (cap the output at about that many
tokens; the last row says what was cut), `fresh:true` (fetch again instead of reading the cache; a new id,
except a search result, which is refilled), `wait:30` (seconds before a running fetch answers `pending`, 0–300).

Fetch modifiers: `provider:auto|plain|bd|oxy|decodo` (`oxy` = Oxylabs without, then with JavaScript; `decodo` the
same on Decodo; `auto` written
out = the whole ladder in table order, ignoring the site's kept `js:true` runs), `js:true` (skip the
plain request and render JavaScript, one provider at a time), `cost:0.05` (most a fetch may spend, in USD;
`cost:0` = the free plain request only; a `budget_hit` row ends with the cap that would do and, with two
provider keys, one provider's ask for a user who asked to pay: `→ fats u1 fresh:true cost:0.004 ≤$0.0040 · if
the user said pay, fats u1 fresh:true provider:oxy ≤$0.0025` (Oxylabs, then Decodo, then Bright Data; the price
is that provider's whole path); a batch's `cost:` is one pool for all its pages: a paid call takes its
price from it before it starts, a page short only of what pages already running will give back waits for them, a
page left without its cheapest rung fails `budget_hit` naming who spent the cap,
`! budget_hit bd oxyjs not tried · $0.0030 > cost:0.002 (spent $0.0015 by u1) → …`, and `dry:true` adds `batch up to
$0.0020 · cap $0.0020`), `country:de` (exit country for the providers; without it, the country
of a European country-code domain, `.de` → DE, `.co.uk` → GB, else `FATS_COUNTRY`, else US for Oxylabs, which
exits in a random country when asked none; Bright Data is sent a country only when one is named that way, a
`FATS_COUNTRY` other than US included, and otherwise picks its best location for the site; `dry:true` shows a
named one, `fats stats` the one sent, `oxylabs@US`), `timeout:150` (seconds for the JavaScript tiers, 1 to 300;
a hard budget: with it set, nothing is retried; the plain request is cut there too, then the provider calls share
one wall of it, so a page costs at most about twice it), `hedge:0` (start both providers at once),
`device:mobile` (Oxylabs and Decodo render the phone page; Bright Data serves desktop and `dry:true` says so),
`max_bytes:32m` (input cap, default 16 MiB, at most 64 MiB).

Search modifiers: `limit:5` (results, 1–20, default 10), `country:de` (results as seen from that country; without
it `FATS_COUNTRY`, else US), `site:docs.rs` (prepended to the query as `site:`), `since:7d` / `since:2026-01` (appended as
`after:YYYY-MM-DD`, a hint to the search engine, not a filter: the APIs have no date parameter),
`provider:oxy|bd`, `timeout:` (seconds per provider, default 15), `hedge:` (default 2.5 s for a search), `cost:`.
A bare name is a site only when it ends in a real top-level domain (`docs.rs`, `example.com`; `opencode.json`,
`main.go` are words). One whose ending is also a file extension (`.rs`, `.md`, `.py`, `.sh`, `.zip`, …) is a site
only as the first word (`fats docs.rs tokio spawn`); later it is a word (`fats cargo build main.rs`); alone it is
a file when one exists here, and asks file or site when not (`fats main.rs`; `fats https://docs.rs` reads the site).
A `/`, a scheme or `site:` always names a site.
The Oxylabs Web API is asked first; when it fails or has not answered within the hedge delay, Bright Data runs
the same query on Google through the Web Unlocker zone, billed as one unlocker request ($0.0015; the zone
counter shows it). A search waits up to `wait:` (30 s); when its first keyed provider usually takes
longer (Bright Data alone: 10 s, so under a `wait:` below 10), it goes on in the background, `pending`, and `fats j1`
lists its results once they land. The list prints every result that fits `read_cap` (1000 tokens; `limit:N` = at
most N rows), and `fats more` goes on past it. A search is never cached; its results are ids like any page, and
`fats stats` lists them as `search→u3`. `fetch:1` (the default; 0–50, `fetch:0` = none) starts fetching the top
result in the background right after the list prints, within `cost:` (the search's own cost counts first), so
`fats u1` a moment later is usually a cache hit.

`fats gh <words>` searches GitHub repositories, most stars first (REST `search/repositories`; free; `limit:` up
to 50). It sends the token saved by `fats setup gh` (which copies `gh auth token` once; `fats setup import-env`
copies an exported `GH_TOKEN`), else none (GitHub allows 30 searches a
minute with a token, 10 without; the `=` line says `no token: 10 searches/min`). A word shaped like a GitHub
login also lists that account's top 100 repositories by stars (a second free search, `user:<login>`; an account
GitHub's search leaves out falls back to its 100 last pushed), merged by stars.
A rate limit prints `! rate_limited gh retry in Ns`. Each repository is an
unfetched id like a web result; `fats u1` opens its page through the usual tiers.

Output is line-shaped. The first line starts with `=` and summarises the result. Rows follow, then the next move.
A line starting with `!` is an error record, a line starting with `?` is a question, a last line starting with
`~` names background runs. Exit codes: `0` ok, `1` failure, `2` usage. A page whose paid try the `cost:` cap
stopped while a copy is shown exits `0`: the `! budget_hit` row carries the move.

On a terminal, `fats`, `fats help`, `fats help setup|mcp|search`, `fats setup` and a page that failed print
short screens for a person instead. Piped output and `fats mcp` are always the lines described here;
`FATS_HUMAN=1` forces the screens, `FATS_HUMAN=0` turns them off.

### What the `=` line tells you

`= id title · place · COUNTS · tok · HOW · ! FLAGS`, in that order:

- **A count** says what is here: `11 sections`, `+2 island` (big data blocks inside the page, listed after the
  sections), `3 photos`, `95 of 424 numbers pruned`, `thin 0 of ~41.1k visible`.
- **`tok`** is what the answer costs a model: an outline's figure is the whole page, a read's figure is what it
  printed.
- **A word** says how it came: `plain`, `og` (the page's card), `md` (the server's Markdown), the providers
  tried with their price (`bd:js_shell✗ oxyjs✓`), `hedge:off`, `cache 5m`, then `spent $0.0015` when the
  answer cost money (a free one ends at its time: `plain 76ms`).
- **`!`** says what you asked for and did not get, with the next step last:
  `! js_shell → fats u1 js:true · bd, then oxyjs ≤$0.0030 ≈25s` (`≤$` = the most it can spend, `dry:true`'s "up to"; `≈25s` = a
  render's usual time).

A filter whose hits are only in the islands says ` · in islands`. An island that repeats text already served is
marked `(dup)` on its outline row and is never the pointer.

### Background runs

A fetch that has not finished after `wait:` (default 30 s, one deadline for the whole command; a batch's URLs
run under it in at most `batch_parallel` (8) processes, the rest `queued after u1`; ids follow the input order,
as `dry:true` prints them, and a URL named twice is one run, its second row `uN (same as above)`)
prints a `pending` line and exits 0; the fetch goes on as a detached process and
`fats u3` reads the page once it is stored. `fats u3`, or the same URL asked again while that fetch runs, waits
for it within its own `wait:` and prints the page the moment it lands (never a second fetch of the URL; a search's
background prefetch is the one exception: asking its URL starts the full fetch). The line's move is free, `fats u3 wait:30 (free, usually ready in
~12s)`: the estimate is the median time of this session's calls to the same host on the same providers, counted
from when the running call started and never past that call's own deadline (its `timeout:` or tier default); with
fewer than 3 such calls it reads `· free, check again in 30 s` instead. Every later CLI answer ends with one
line naming those runs until each has been reported once:

```text
~ u3 u4 u5 pending 0s · next: fats u3 wait:30 · free, check again in 30 s
~ u3 done · u4 done · u5 done
```

The line is printed on the command line only, never through `fats mcp` or with `json:true`. A run is alive
while it holds its lock, taken at launch before the run starts, so a run slow to start on a busy machine is
`pending`; a run whose process died answers `! abandoned`.

While a command waits for a run past 5 s, a terminal shows it working on stderr, at 5 s and every 5 s:

```text
~ still working · bd, oxyjs running · 10 s
```

Only when stderr is a terminal: a pipe gets no byte of it, nor does `fats mcp`. There, a `tools/call` that sends
`_meta.progressToken` gets a `notifications/progress` with that token every 5 s, `progress` = the seconds and
`message` = `still working · 10 s`, so the host can show its own spinner.

## Any one key

`fats` works with no key, with an Oxylabs Web API key, a Bright Data key + Unlocker zone, a Decodo Web Data API
token, or any mix of them. The
ladder is built from the providers whose key is set, each provider doing what it can (page, JavaScript render,
search, PDF bytes). `dry:true` prints it with its worst-case price:

```text
$ fats https://www.nytimes.com/ dry:true                # all three keys
nytimes.com/ fetch plain → oxy (after a captcha) → decodo (after a captcha) → bd ∥ oxyjs after 5s → decodojs → new u1 · up to $0.0065

$ fats https://www.nytimes.com/ dry:true                # Oxylabs + Bright Data
nytimes.com/ fetch plain → oxy (after a captcha) → bd ∥ oxyjs after 5s → new u1 · up to $0.0040

$ fats https://www.nytimes.com/ dry:true                # Oxylabs + Bright Data, free fetch off
nytimes.com/ fetch oxy → bd ∥ oxyjs after 5s → new u1 · up to $0.0040

$ fats https://www.nytimes.com/ dry:true                # Oxylabs key only
nytimes.com/ fetch plain → oxy → oxyjs → new u1 · up to $0.0025

$ fats https://www.nytimes.com/ dry:true                # Bright Data key only
nytimes.com/ fetch plain → bd → new u1 · up to $0.0015

$ fats https://www.nytimes.com/ dry:true                # Decodo token only
nytimes.com/ fetch plain → decodo → decodojs → new u1 · up to $0.0025

$ fats https://rust-lang.org/tools/install dry:true     # no key
rust-lang.org/tools/install fetch plain → new u1 · up to $0.0000 · ! no_key bd · would try unblock+JS ≤$0.0015 → fats setup bd · ! no_key oxy · would try no-JS fetch + JS render ≤$0.0025 → fats setup oxy · ! no_key decodo · …

$ fats rust async runtime dry:true                      # Bright Data key only
search "rust async runtime" via bd → bd retry · limit:10 · wait:30s → new u1-u10 · up to $0.0030
```

Decodo has no web search here: with only a Decodo token a search is `nothing can search · $0` and names the
Oxylabs and Bright Data keys.

- With one key, a call without JavaScript that `fats` cut at its own timeout, or any call lost to a network error,
  gets **one retry**, inside `cost:`, only when what is left of 90 s from the run's first call (`one_key_wall_s` in
  `fats config`) is at least the time the failed call had; never after a provider's own refusal, a site's status, a
  wall or a JavaScript shell. A render (bd, oxyjs, decodojs) cut at its own 40 s is never retried: a second render
  would take up to twice the time at twice the price for the few pages that need more than 40 s. A cut with no
  other keyed provider left is the end, no arrow:
  `! provider_timeout … · oxyjs cut at 40 s · failed 1/1 here` (of 131 saved renders that served, 8 took 40–60 s, none longer).
  Your own `timeout:` is a hard
  budget: each call stops there and nothing is retried; the next step is `→ fats u1 fresh:true` (the default
  time). A Bright Data search running in the background is one call at twice the search timeout, no retry.
- With one key, a JavaScript challenge, shell or empty search page from the plain request skips Oxylabs
  without JavaScript (it served 0 of 33 such pages); a 403 or captcha keeps it.
- A missing key is never a failure: `! no_key` says what that provider would add here and its price, and its
  move is `→ fats setup <provider>`. With no paid key, a dry run lists every provider, cheapest first. With one,
  the others stay quiet unless `provider:` names one; a failed page names every missing provider that could
  still serve it, and its move is `→ fats setup`, never `nothing left to try`.
- The Oxylabs key must be a **Web API** key (dashboard → Web API instance); keys of other Oxylabs products are
  refused with `! bad_key oxy` and a link to the dashboard.

## Pictures

The photos of a page are listed from the bytes already fetched; nothing extra is downloaded. `N photos` on the
`=` line counts content photos only. Logos, icons, avatars, badges, spacers, share cards and pictures that link
to another page are listed as `(not counted)`, and one picture at several sizes is one row. When the page itself
was not served (a JavaScript shell, a wall), no count is printed, so `0 photos` means the page had none.

- `fats u3 media` lists every picture: `m1 photo 2048x1365 webp | <alt> → <url>` (`(no alt)` when there is
  none). A size is printed only when the page declares it.
- `fats u3 m1` saves picture 1 into the page's cache folder with the free plain request (`image/*`, at most
  10 MB) and prints the path and what viewing it would cost, one range from its real size: low = Claude's
  standard 1568 px tier (what `fats mcp` sends), high = the picture unscaled on Claude 4.7 and later. The next
  call reads the saved file; `fresh:true` downloads it again.
- Through `fats mcp`, the same call also returns the picture as one image block (downscaled when Claude would
  resize it or when the file is large). A fetch never returns pixels unless a picture is asked for by id.

## Examples

Real output. Job numbers and times are masked (`jN`, `Nms`); long lines are cut at `…`; `[rc N]` is the exit
code. The first block ran with no provider keys.

```text
$ fats https://doc.rust-lang.org/book/ch01-01-installation.html
= jN.u1 Installation - The Rust Programming Language · doc.rust-lang.org · 11 sections · 0 photos · 1.9ktok · plain Nms
u1#1 Keyboard shortcuts (65) | Press ← or → to navigate between chapters Press S or / to search in the book…
u1#2 The Rust Programming Language (1.9k, 9 sub) | → Installation · Command Line Notation · Installing rustup on Linux or macOS
u1#3 Installation (1.9k, 8 sub) | The first step is to install Rust. We’ll download Rust through rustup, a…
u1#4 Command Line Notation (151) | In this chapter and throughout the book, we’ll show some commands used in the…
u1#5 Installing rustup on Linux or macOS (364) | If you’re using Linux or macOS, open a terminal and enter the following…
… +6 sections → fats u1 all
read: fats u1#2 · filter: fats u1 <words> · links: fats u1 links · all: fats u1 all

$ fats u1#7
= jN.u1#7 Troubleshooting · part 1/1 · up fats u1 · next fats u1#8 Updating and Uninstalling · cache 0s
To check whether you have Rust installed correctly, open a shell and enter this line:
$ rustc --version
…

$ fats u1 rustc --version
= jN.u1 filter "rustc version" · 1 hit · match:all auto · cache 0s
u1#7 $ rustc --version → You should see the version number, commit hash, and commit date for the latest…
read: fats u1#7

$ fats u1 links
= jN.u1 links 5 · open one: fats u1 l<N> · cache 0s
l1 forge.rust-lang.org/infra/other-installation-methods.html
l2 rust-lang.org/tools/install
…
(end of links, 5)

$ fats u1 l2 dry:true
= dry u1 l2 dry:true · spends nothing now
u1 l2 rust-lang.org/tools/install fetch plain → new u2 · up to $0.0000 · ! no_key bd · would try unblock+JS ≤$0.0015 → fats setup bd · ! no_key oxy · … · ! no_key decodo · …
run it: fats u1 l2

$ fats ./report.pdf
= jN.u2 Tiny Test Doc · pdf · 2 pages · read 0ms/page · 139tok · local Nms · ! no_text
u2 ! no_text 1 of 2 pages have no text layer (pages 2) · no OCR in this build → fats u2 all
[u2#1] # page 1
Tiny test doc: tuna tartare 23
served daily
[u2#2] # page 2
filter: fats u2 <words> · links: fats u2 links

$ fats ./hn-thread.html                      # a saved 370-comment Hacker News thread
= jN.u3 German Rheinmetall open-sources its Battlesuite connected weapon… · hn-thread.html · 110 sections · 0 photos · 10.1ktok · local Nms
u3#2 Stitch4223 24 days ago | next [–] “Hi Codex, please (273) | | | | Stitch4223 24 days ago | next [–] “Hi Codex…
u3#3 pstadler 24 days ago | parent | next [–] "The (169) | | | | pstadler 24 days ago | parent | next [–] "The…
…
… +103 sections → fats u3 all

$ fats
= s:demo 3 ids · last u3 file:///…/hn-thread.html
cache … (mode 700) · 229.8kB (0B pictures) · 3 pages · 0 searches · 3 jobs
keys none → fats setup
health (last 10 calls, whole cache): provider ok/n p50ms outcomes (– cancelled)
plain 1/1 Nms ✓
bd no key
oxy no key
oxyjs no key
decodo no key
decodojs no key
account: not read yet · fats account
mcp: ready · fats mcp = stdio server, one tool fats(cmd)
at u3 · outline: fats u3 · stats: fats stats · config: fats config

$ fats stats
= stats s:demo · 0 searches · 3 fetches · Nms spent $0.0000
u1 doc.rust-lang.org · Installation - The Rust Programming… · plain Nms $0.0000 · 1.9ktok
u2 report.pdf · Tiny Test Doc · local Nms $0.0000 · 17tok
u3 hn-thread.html · German Rheinmetall open-sources its… · local Nms $0.0000 · 10.1ktok
spend all sessions: 24h $0.0000 · all $0.0000 · 0 paid records · ledger …/fatsearch/spend.log
last page: fats u3 · settings: fats config
```

The index shows the last 20 rows, newest last, and names the rest (`… 6 more · fats stats all`); a failed fetch's
row is its `!` row with what it tried. `fats stats full` (and `json`) prints every row whole: raw bytes → clean
~tok → outline ~tok, photos, pruned numbers, the tried path with each paid call's country and timeout.

With both keys set (this session spent $0.0055 on Mastodon's render and one search):

```text
$ fats https://mastodon.social/@Gargron/117433450473905580
= jN.u1 Eugen Rochko (@Gargron@mastodon.social) · mastodon.social · 0 sections · 1 photo · 351tok · og plain Nms · ! js_shell → fats u1 js:true · bd, then oxyjs ≤$0.0030 ≈25s
u1#0 info | author Eugen Rochko · published 2026-10-13T11:50:49Z · Attached: 1 image Another discard from Tuscany. …
m1 photo 2225x3356 jpeg | A seagull with his beak agape, standing on top of a lamp post. … → https://files.mastodon.social/…/919c0782bdd56173.jpg
(card only) · filter: fats u1 <words> · links: fats u1 links

$ fats u1 m1
= jN.u1 m1 photo 2225x3356 jpg 897.7kB · saved <cache>/<session>/u1/assets/m1.jpg · ~1.6–4.8k tok if viewed · plain Nms $0

$ fats u1 js:true
= jN.u2 Eugen Rochko (@Gargron@mastodon.social) · mastodon.social · 3 sections · 1 photo · 688tok · bd:js_shell✗ oxyjs✓ Nms spent $0.0030 · hedge:off ← u1 fresh

$ fats u1 js:true dry:true
u1 mastodon.social/@Gargron/117433450473905580 fetch bd → oxyjs → new u3 · up to $0.0030 · ≈25s · hedge:off, one provider at a time

$ fats tokio rust async runtime limit:5
= jN search "tokio rust async runtime" · 5 results · oxylabs · N.Ns · spent $0.0010
u3  tokio.rs/  Tokio - An asynchronous Rust runtime — "Tokio is a runtime for writing reliable asynchronous applications with Rust. It provides …"
u4  github.com/tokio-rs/tokio  GitHub - tokio-rs/tokio: A runtime for writing reliable asy… — "Tokio is an event-driven, non-blocking I/O platform…"
…
prefetching u3 free, plain only (detached; fetch:0 = none) · status: fats
open one: fats u3 · filter: fats u3 <words>
~ u3 pending 0s

```

With only the Oxylabs key, a TikTok video whose render timed out (a cut call is billed as unverified; the
caption sits in the page record):

```text
$ fats u1
= jN.u1 TikTok - Make Your Day · www.tiktok.com · 0 sections +2 island · 349tok · cache 1h · ! provider_timeout · thin 0 of ~41.1k visible → fats u1#2ft (page record) · 982 tok · $0
u1 ! provider_timeout tiktok.com/@hildabaci/video/7557346961225944328 · tried plain:js_shell✗ oxyjs:provider_timeout✗≤$0.0015 (unverified) · showing the best copy · oxyjs cut at 40 s (2m ago) → fats u1#2ft (page record) · 982 tok · $0
u1 ! no_key bd · would try unblock+JS ≤$0.0015 → fats setup
[u1#1] island script#chunk-manifest 73 KB · 66.3k tok → fats u1#1
[u1#2] island script#__UNIVERSAL_DATA_FOR_REHYDRATION__ 294 KB · 194.1k tok → fats u1#2
filter: fats u1 <words> · links: fats u1 links

$ fats u1#2ft
= jN.u1#2ft island script#__UNIVERSAL_DATA_FOR_REHYDRATION__ 294 KB · part 176/206 · up fats u1 · cache 1h · thin 0 of ~41.1k visible · ! provider_timeout
…
l7389      "desc": "Detailed Afang soup recipe video  Was this helpful ?",
…
… 24.6k more → fats u1#2fu

$ fats u1 hilda
= jN.u1 filter "hilda" · 5 hit · match:exact · in islands · cache 1h · thin 0 of ~41.1k visible · ! provider_timeout

$ fats u1                                    # the record is read: nothing left to try
= jN.u1 TikTok - Make Your Day · www.tiktok.com · 0 sections +2 island · 358tok · cache 1h · thin 0 of ~41.1k visible · ! provider_timeout
u1 ! provider_timeout tiktok.com/@hildabaci/video/7557346961225944328 · tried plain:js_shell✗ oxyjs:provider_timeout✗≤$0.0015 (unverified) · showing the best copy · oxyjs cut at 40 s (5m ago) · failed 1/1 here
…
```

## MCP

`fats mcp` is a stdio MCP server with one tool, `fats`, that takes one string, `cmd`: the same command line
as above (`u1#2`, `https://example.com`), answered with the same text the CLI prints (without the `~` line).
The card goes to the client once, in `instructions`; the tool description is about 60 tokens. The tool is
annotated not read-only but non-destructive (a call may spend provider credit and adds to its cache) and
open-world (it fetches the web). Local files are read only from the working directory and the cache, and `-`
(stdin) is refused, since stdin carries the protocol. A key or credential file (`.env`, `id_rsa`, `*.pem`, …) is
never read, over MCP or on the command line: `! secret_file`. A failed command is a `!` row in the text; only a request
that is not a `fats` call is a protocol error. The server exits when the client closes its stdin. Add it to a
client:

- **Claude Code:** `claude mcp add fats -- fats mcp`
- **Codex:** `codex mcp add fats -- fats mcp`
- **Cursor:** `.cursor/mcp.json` in the project (or `~/.cursor/mcp.json` for every project):

```json
{"mcpServers": {"fats": {"command": "fats", "args": ["mcp"]}}}
```

The session is `FATS_SESSION` if the client sets it, else a hash of the server's working directory.

## How a page is fetched

1. **Plain request** first: a browser-like HTTP client (Safari's TLS and HTTP/2 settings and headers),
   redirects followed, 10 s. It costs nothing, and it comes from your own IP, so it is braked: at most one
   plain start per host per second, never two at once to one host, across every `fats` process
   (`plain_host_pace_ms`, 1000). A page that has to wait for its turn waits; the brake never sends it to a
   paid provider. Providers have no pace: their IPs, their limits (`! rate_limited`).
   Before any request the host gets a verdict: a name without a real top-level domain is `! no_host`, a private or
   loopback address `! private_address` (fetched only with `FATS_ALLOW_PRIVATE=1`), and the system resolver gets
   `dns_wait` seconds (5) or the shorter `timeout:`; no answer in time ends the run as `! dns_timeout` at $0 with
   `→ fats uN fresh:true dns_wait:15`, never a paid call. A resolver error back in under a second (no network)
   is `! dns_failed` with the resolver's own words and `→ check the network, then fats uN fresh:true`: a longer
   wait cures nothing there. `dry:true` judges the name only and looks nothing up.
2. **Content check.** A 200 is not enough: the body must not be a JavaScript shell, a captcha, challenge or
   Cloudflare wall, a login or consent page, or too thin to be the page its title names. Search result pages
   must list at least five results. A 404 or 410 is never the page, even when it repeats the URL's words.
3. **Providers**, only for the keys that are set (see [Any one key](#any-one-key)):
   - **Both keys:** Bright Data's Web Unlocker starts; when it has not answered after 5 s, the Oxylabs Web API
     with JavaScript rendering (`oxyjs`) starts beside it (`hedge:0` starts both at once; 40 s each,
     `timeout:` changes these two). The first good page wins and the other call is cancelled as `abandoned`
     (`oxyjs:abandoned✗≤$0.0015 (unverified)`: a provider may still bill a call `fats` hung up on). When the first good page is thin and the other call is
     still running, `fats` waits for it, within the hedge window, and serves the fuller copy.
   - **Both keys, free fetch off** (`fats setup free off`): no plain request to wait for, so `oxy` without
     JavaScript (10 s) runs first, then Bright Data and `oxyjs` as above.
   - **Oxylabs key only:** Oxylabs without JavaScript (`oxy`, 10 s), then with it (`oxyjs`). After a JavaScript
     challenge, shell or empty search page from the plain request, `oxyjs` goes first.
   - **Bright Data key only:** Bright Data.
   - **Decodo token only:** Decodo's Web Data API without JavaScript (`decodo`, 15 s), then with it
     (`decodojs`, 40 s).
   - **Decodo beside other keys:** `decodo` without JavaScript runs after `oxy` (both only after the plain
     request met a captcha or wall), and `decodojs` is the last render: one hedge after Bright Data and
     `oxyjs` with all three keys, beside the other render with two (`dry:true` prints the exact order).
   - With one key, a run ends within 90 s of its first call; only `fats`'s own timeout or a network error is
     retried, and only when the time left is at least what the failed call had; never a provider's refusal
     (see [Any one key](#any-one-key)).
   - A provider is skipped only after it failed in this run; no past run takes it off the next visit's ladder
     (keeping such skips lost more served pages than the ~2 s a fetch they saved).
   - Oxylabs without JavaScript is otherwise used for a PDF's bytes and `provider:oxy`.
     Decodo is not used for PDF bytes or web search.
     A search's prefetch is the free plain request only, whatever the keys; with the free fetch off there is none.
     Government hosts skip Bright Data.
4. **App pages.** A JavaScript shell from the plain request plus shells or timeouts from two different
   providers is an app page: `fats` stops with `! no_copy`, keeps the best copy, and points first at the
   island part that holds the page's own words (its description, a title that is not the site's name, or its
   first picture's caption; free), else at a `js:true` render with its price when a keyed render was not tried
   in this run. One provider's shell alone is only that provider's failure.
5. **`js:true`** skips the plain request and runs one provider at a time (`hedge:off`): Bright Data first, then
   Oxylabs, then Decodo, every time; never the render whose copy is already shown when the other provider's render has a key.
   Its quote, `≈25s`, is a render's usual time; its price is the `up to` of that order, the same `≤$X` every paid
   next step ends with.
6. Every failed step is a `!` row with its code and the next move. A timeout's next move changes the request:
   `js:true` while a keyed render is untried; a render timeout with no keyed tier left is the end, no arrow
   (`failed 1/1 here`); after your own `timeout:`, `fresh:true` at the default time. A thin or failed
   copy gets one next move, free first: a stored paid render of the URL with other text; else, when this copy
   shows none of the page's own words, a stored copy's part that does (`→ fats u1#3fy · page content`); else,
   when nothing names the page's own words, its page record: the data-island part holding the URL's id
   (`→ fats u1#2fv (page record) · 995 tok · $0`; a TikTok shell's caption); else a `<template>` island of 5
   or more paragraphs (`→ fats u1#5 · 3.3k tok · $0`); else `js:true` with its price, never when a paid copy
   of the URL came back with the same text. The move never names a part already read in this session, over
   every id of the copy (a cache hit too): after `u1#5` it is `u1#5b`, and once every island part was read
   there is none.
7. **Geo copies.** The language expected comes only from the request and the URL: `country:`, the ccTLD, a
   first host label like `de.` or `en-gb.`, the first path segment like `/de/` or `/pt-br/` (`/hotel/fr/`
   names none), or the plain request's `<html lang>` when plain got the page itself. Only when the URL names
   no locale (none of these, and no query value such as `hl=fr`) **and** the call sent no country (Bright
   Data's own pick) is `en` expected; otherwise no copy is flagged. A provider copy in another language
   carries `! lang:it · served in it, sent US · the provider's geo copy → fats u2 provider:bd fresh:true
   ≤$0.0015` on every read (`sent none → fats u1 country:us fresh:true` for Bright Data's own pick). The next
   step always changes the request: the URL's own country when another was sent, else the other provider
   when its key is set, else none, only a count on the `=` line (`lang:it · sent US`). Once a newer copy asked
   another way is in the session, the row points at it for $0 (`→ fats u2`); a copy that differs from a
   `country:` you named says `served in en (asked DE)`, with no paid fix. The tier word names a non-default
   exit (`oxyjs·fr`).

Every body is recognised by its first bytes (`%PDF`, the zip signature), whoever fetched it.

- **PDF.** A PDF whose plain fetch fails (the URL ends in `.pdf` or has `/pdf/` or `/doi/pdf` in its path, or
  the failed answer says PDF) goes to Oxylabs without JavaScript (30 s, `timeout:` changes it), then Bright
  Data for the file's own bytes; never to JavaScript rendering. An answer that is not a PDF is `! not_pdf` and
  the next provider is tried; a file that ends early and lost a page is `! pdf_truncated` with the pages read.
  A PDF becomes one section per page; pages without a text layer (scans) are counted in a `! no_text` row,
  since this build has no OCR. Text extraction stops after 10 s (`! parse_cut` names the pages not read).
- **DOCX/XLSX** never go to a provider; they are saved and reported as not convertible yet.
- **Text documents** (a `text/plain`, CSV, Markdown, JSON or XML answer, or a body that parses as JSON or opens
  like XML) are served as they are: one `lN` row per line, 300 lines per section (`u1#2` = lines 301-600), JSON
  top-level keys, XML elements or a feed's item titles as `u1#0`. They never go to a provider.
- **Markdown from the server.** The plain request asks for `text/markdown` before HTML. A Markdown answer is
  cleaned like a page and marked `md` on the `=` line. When it is much thinner than the page or has no
  headings, `fats` asks again for HTML; if that fails, the Markdown copy is served. `FATS_ACCEPT_MD=0` turns
  this off.
- **The og card.** When the plain answer is a JavaScript shell (or a refusal other than 404/410) but its head
  carries the page's own card (title, description, image), the card is served at once for $0, marked `og`
  and `(card only)`, with `! js_shell → fats u1 js:true · oxyjs ≤$… ≈25s` for the full render. A site-wide card (its
  title is the site's name) is never taken for the page.
- **No learned start.** Every page walks the same ladder from the plain request; what past runs did never
  starts a page further down it, nor reorders `js:true` (step 5). `fats` keeps no per-site run log.

**Billing.** The plain request is free. Both providers bill an answer below 500. A provider's own 5xx or
error envelope, its own refusal (`provider_refused`) and a search answer without a result list print `$0 (not
charged)`; Bright Data's 5xx inside an answer is its own unlock failure and not billed. Every call `fats` cuts
or abandons (a call past its timeout, a search cut at `wait:` or `timeout:`, the loser after a serve) prints
`≤$0.0015 (unverified)`, never `$0`: Bright Data billed some of them (searches cut at 15 s) and not others
(page timeouts, searches cut at 1 s). `fats stats` keeps the two apart (`$0.0390 + ≤$0.0105 unverified`), and
`cost:` counts the verified part. A finished Oxylabs job that relays the site's own 5xx is billed, marked
`(assumed)`. On 2026-10-15 Bright Data's zone counter matched on pages (23 printed, 23 counted), and the
window's total, $0.0405, fell inside the printed $0.0390–$0.0495. Oxylabs has no usage counter for a Web API
key, so its cut calls stay unverified. Decodo has no usage read for a Web Data API token either: its `$` is the
list price ($0.0010 without JavaScript, $0.0015 rendered), never reconciled. `dry:true` prints the route and its worst-case price before
anything is spent; `fats stats` lists provider, time, cost and route of every fetch; `fats` shows each
provider's last 10 results. Prices are a table in the source (list prices per 1,000 requests) and are estimates,
not your bill; `fats account` reads what the providers report.

## How a page is cleaned

1. Decode the text by the page's charset (BOM, then the `Content-Type` header, then `<meta charset>`, else
   UTF-8) and store it as UTF-8; `fats stats` names a charset that was not UTF-8.
2. Parse HTML with html5ever (the WHATWG tree builder browsers follow); resolve links with the WHATWG URL rules.
3. Prune chrome: navigation, cookie banners, hidden elements, link lists. Text hidden only from screen readers
   (`aria-hidden`) and button text stay; two numbers in neighbouring elements get a space between them.
4. Render headings, lists, tables and links (`[l7]`) as Markdown. A table row that holds another table (a
   layout table) is read as its cells' blocks; a table of 8 or more such rows, each with a text of 12 words or
   more, becomes one section per row (a forum or comment thread).
5. **Loss guard.** Every prune is measured against the page with nothing pruned. If the clean render loses more
   than 30% of that page's numbers (prices, dates, counts), `fats` serves the render with only chrome pruned, and
   if that also loses more than 30%, the unpruned page. `fats stats` shows which one was served and the loss.
6. **Islands.** A `<template>` holding a fifth of the page's text, or a JSON data script of 20 KB or more, is
   listed after the sections as an island (`[u8#2] island … → fats u8#2`). A filter that finds nothing in the
   served text searches the islands. `! thin` marks a page whose served text is under 15% of the text in its
   bytes, unless the page's own description or title is in a served section.

`Ntok` estimates what a model counts: ASCII letters / 2.7 plus every other non-space character / 0.8, within
±10% of Claude's count on 36 of 42 sampled reads. A page read whole says what the read puts in context: its `=`
line (the timing counted at a fixed width, so every read of the copy says one size) and its rows; an outline says
the page's size. `fats stats` prints the same figure. Read and list caps use the same unit. The outline shows each
section's size and a preview: its first 80 characters, or, when the unpruned page was served (its sections open
with menus), the stretch of the section that holds the most words of the page title not already in its heading.

## Cache and ledger

The cache is short-lived: a page unread for 30 minutes, or fetched an
hour ago, is never read again and is deleted with its URL index row; its id then says
`! snapshot_missing … → fats <url> fresh:true ≤$X`, a new fetch at its price. Once its job is an hour old too,
the job's command and the id's URL (a search's result URLs too) are forgotten: `fats u3` and `fats j2` then say
`! expired`. The ids themselves stay, so an id never names another page. What was spent stays in a small ledger
next to the keys (`spend.log`: time, cost and the record's cache dir per paid record; no URL, no query): the
`fats stats` total is this session's rows written since it began (its first write, kept in its `state.json`),
plus the calls in flight of a dead run (`≤$X`; the one sweep that claims its run file stores them as the id's
failed copy, which writes their row; never for a run whose lock is held). So a dead run's id reads
as that failed copy, `! abandoned … → fats u3 fresh:true ≤$X`, for $0 before and after the sweep; its `fats stats`
row is the same `!` row with what it ran before the arrow, and `fats u3 dry:true` says `local read · $0`; only the
`fresh:true` form spends. A ledger fats cannot write is a `! cache_write ledger not
written: …` row in `fats stats`, and the total reads `spent ? (ledger not written)`, never a silent $0. A swept
page's own row reads `u3 swept 0ms $0.0000`: what it cost is in the total. A search result never fetched before
the hour is over leaves no row.

## Settings

`fats config` prints every setting and every behaviour threshold, its value and where the value came from:
`mod` (on this command), `env`, `settings.json`, `default` or `build` (the built-in table).

Every `build · limits.json` row, and `session`, `cache`, `json`, `nocache`, `offline`, `allow_private`, `chunk`,
`account_ttl`, `accept_md`, can be set three ways; the first one present wins:

1. a mod on the command: `batch_parallel:2`;
2. the env, `FATS_<NAME>` in capitals: `FATS_BATCH_PARALLEL=3`;
3. `settings.json` in the key dir (0600): `fats config set batch_parallel 8`, `fats config unset batch_parallel`.

```text
$ fats config set batch_parallel 8
= config set batch_parallel 8 · saved settings.json · from the next command
$ fats config | grep batch_parallel
batch_parallel 8 · settings.json · lanes (RAM guard, not a provider limit)
$ fats config set plain_host_pace_ms abc
! refused · plain_host_pace_ms abc · want a number ≥ 0 like 1000
$ fats config set chunk 99999
! refused · chunk 99999 · want an integer 50-10000
$ fats config set cost_cap 0.01
! refused · cost_cap is per call: the cost: mod
```

Per-call knobs (`hedge:`, `wait:`, `timeout:`, `cost:`, `country:`, `device:`, `provider:`, `max_bytes:`) stay
mods; `fats config set` refuses them by their mod (`wait` → `the wait: mod`). `fats setup <free|oxy|bd|decodo|gh>
off|on` turns a provider (or the free plain fetch) off without deleting
its key, saved in `settings.json` too; turning off the last thing that can fetch is refused
(`! refused · nothing left that can fetch: turn one on or add a key`). A switched-off provider's dry and failure
row is `! off oxy · turned off (key kept) → fats setup oxy on` and its status health `oxy off (key kept)`; with
the free fetch off, `fats setup free` reads `= setup free · free off (turned off) → fats setup free on` and the
`fats setup` summary starts `= setup free off (turned off) · …` with that row first. A
Bright Data key saved without a zone reads `! no_zone bd · key saved, no Web Unlocker zone yet → fats setup bd
zone:<name>`. `fats setup` takes only a switch name (`plain` = `free`), `on|off`, `import-env` and mods; any
other word is a repair at $0, never a search: `fats setup oxi` → `? setup oxi → fats setup oxy`.

`fats setup` with no name checks every saved key (one free read each) and writes nothing: a zone it would pick
is named (`· zone not saved → fats setup zone:<name>`), saved only when `zone:` names it. Each row names the
key by its last 4 characters and its source: `oxy ok 351 scrapers · key …6XBR auth.json`. A key exported in the
environment is never sent; `fats` and `fats setup` name it by its tail on one row: beside a different saved key
`env OXYLABS_WEB_API_KEY …SEDm ≠ saved …6XBR · not used · replace: fats setup oxy`, where none is saved `! no_key
oxy · env OXYLABS_WEB_API_KEY …SEDm not imported → fats setup import-env` (in `fats setup` it replaces the
provider's plain `no_key` row: one row a missing key). The status line names every key by its
tail, Bright Data's zone and the file: `keys oxy …6XBR · bd …k2Qf zone fatsrc · decodo none · gh none ·
auth.json` (` · network off` under `FATS_OFFLINE`). `fats setup import-env` is that same check
with each exported key (`OXYLABS_WEB_API_KEY`, `BRIGHTDATA_API_KEY`, `DECODO_AUTH_TOKEN`, `GH_TOKEN`) where no
key is saved, and saves the ones that pass (`· key …Vz5J OXYLABS_WEB_API_KEY · saved auth.json (600)`). A different
saved key is never replaced by it; replacing one (`fats setup oxy`, the form, the menu, the pipe) keeps the old file
as `auth.json.bak` (0600).

`printf %s "$KEY" | fats setup oxy` (stdin not a terminal) reads ONE line, strips its trailing CR/LF, runs the
menu's check ($0) and saves the key when it passes: `= setup oxy ok 351 scrapers · key …6XBR saved auth.json
(600) · $0`, or `= ! bad_key setup oxy · … · not saved → …`, rc 1. It never starts the key form. Nothing on
stdin within 2 s (an agent's shell) = the key form, as before. A key typed on a `fats setup` command line
(`fats setup oxy <key>`, or after a word for setup: `fats login <key>`, `keys`, `auth`, `signin`, `credentials`)
is refused, never echoed or searched: `! key_in_argv · never on the command line (shell history,
ps) · paste: fats setup oxy · pipe: printf %s "$KEY" | fats setup oxy`.

| setting | default | set with |
|---|---|---|
| session | a hash of the working directory | `FATS_SESSION`: letters, digits, `-`, `_`, ≤ 64; any other name prints a `!` row and gets a session hashed from it |
| cache | the OS cache dir + `/fatsearch`. A page or search dir is never read 30 min after its last read or 60 min after its fetch (`cache_idle_s`, `cache_ttl_s`), and is deleted then: this session's after every answer, every session's at most every 5 min (`sweep_every_s`, the root's `swept` file). Only a root fats made, or the default one, is swept (it holds `.fatsearch-cache`; `config` says `swept`, else `not swept (dir not created by fats)`), and in it only the `u<N>`/`j<N>` dirs holding a `meta.json` and `run/` of a real session dir; a symlink is never followed | `FATS_CACHE`, made absolute once at start (a relative dir is the working dir's; `config` shows the result); `~/…` (no shell expanded it), `/`, `$HOME` or a dir fats cannot make is refused with `! bad_setting` and the default is used; swept, and sized by status, only when fats created it, so a project dir is never swept or walked (status: `not swept (dir not created by fats)`) |
| key_dir | the OS config dir + `/fatsearch` (`auth.json` = keys, `settings.json` = switches, the Bright Data zone and set values; each 0600). The 0.1 files `oxylabs.key`, `brightdata.key`, `brightdata.zone` move in once | `FATS_KEY_DIR` |
| keys | none | `auth.json` only: `fats setup` saves them there (a replaced key leaves `auth.json.bak`); `fats setup import-env` copies `OXYLABS_WEB_API_KEY`, `BRIGHTDATA_API_KEY`, `DECODO_AUTH_TOKEN`, `GH_TOKEN` in where none is saved. A key in the environment is never sent; nothing else is read: no `./.env`, no other tool's files; `fats setup gh` copies the `gh` CLI's token once. The Bright Data zone: `BRIGHTDATA_UNLOCKER_ZONE`, else `settings.json` |
| json | false | `FATS_JSON=1`, `json:true` |
| nocache | false | `FATS_NOCACHE=1` (ignore cached pages) |
| network | on | `FATS_OFFLINE=1` (cached pages and local files only; telemetry off too) |
| telemetry | `full` (the daily update check + the weekly counts; `minimal` = the update check only; `off` = nothing, no update hint); the setup menu shows them as `On (anonymous)` / `Minimal` / `Off` | `fats setup telemetry full\|minimal\|off` (settings.json), `FATS_TELEMETRY=full\|minimal\|off` (`0` = off, `1` = full); off whatever is set under `DO_NOT_TRACK=1`, `CI` or `FATS_OFFLINE=1` ([Telemetry](#telemetry)) |
| telemetry_debug | false | `FATS_TELEMETRY_DEBUG=1`: print each request on stderr as `[telemetry] GET …` / `POST …`, send nothing |
| allow_private | false: loopback, private and link-local addresses are refused at every redirect hop and after DNS (behind an `HTTP_PROXY` the proxy resolves names, so only IP literals are checked) | `FATS_ALLOW_PRIVATE=1` |
| chunk | 300 tokens (text without headings is cut into blocks this size) | `FATS_CHUNK` (50–10000) |
| match | words, exact phrase first when not pinned: exact hits, then the all-words hits, else the rows with the most words (`match:k/n words`) | `match:` |
| list_cap · grep_cap · read_cap | 300 · 500 · 1000 tokens (limits.json rows) | `max:` per command |
| provider | auto: plain, then the providers whose key is set and switched on ([How a page is fetched](#how-a-page-is-fetched)) | `provider:plain\|bd\|oxy\|decodo\|auto` |
| country | `country:`, else the domain's European ccTLD (`.de` → DE, `.uk` → GB), else `FATS_COUNTRY`, else US on Oxylabs; Bright Data pages get only a named one (its own best location otherwise); searches: `country:`, else `FATS_COUNTRY`, else US | `country:de`, `FATS_COUNTRY` |
| device | desktop (sent to Oxylabs and Decodo only when mobile) | `device:mobile` |
| hedge | fetch 5 s, search 2.5 s; a paid ask (`js:true`, `provider:`) is not hedged | `hedge:0` (race) to `hedge:60` (seconds) |
| wait | 30 s, then `pending` (a search past it: in the background, `fats jN`) | `wait:0..300` |
| cost_cap | $0.05 per command | `cost:` |
| plain · bd · oxy · oxyjs · decodo · decodojs | 10 s $0 · 40 s $1.50/1k · 10 s $1.00/1k · 40 s $1.50/1k · 15 s $1.00/1k · 40 s $1.50/1k; a one-key retry after a no-JS timeout gets min(2× the timeout, 90 s − the run's time so far), none when that is less than the failed call had | `timeout:` = ONE wall for every provider call together (bd, oxyjs, oxy no-JS, decodo, decodojs), 1–300 s, one at a time (no hedge), a hard budget: no retry (list prices, estimates) |
| max_bytes | 16 MiB per page (cleaning takes ~17× the input in memory); ceiling 64 MiB | `max_bytes:32m` |
| loss_max · thin_share · … · gh_max · batch_parallel | 44 behaviour thresholds (`src/data/limits.json`), one `fats config` row each: the share of numbers a prune may lose (0.3), the `thin` share and floor, the pruned-numbers count bar, prose and thread sizes, island sizes (an island pointer needs 5 paragraphs), icon/avatar sizes, parse and PDF budgets (2 s, 10 s), max depth 512, the one-key wall (90 s), a batch's processes (`batch_parallel`, 8: a RAM guard, not a provider limit), the plain brake (`plain_host_pace_ms`, 1000), the host check's wait (`dns_wait`, 5 s), … | `name:value`, `FATS_<NAME>`, `fats config set` |
| gov_hosts | by label, no host list: `sec.gov`, `data.gov.uk`, `x.gouv.fr` (skip Bright Data) | build |
| mcp | `fats mcp`, stdio, one tool, files from the working dir + cache | build |

Booleans take `1 true yes on` or `0 false no off`. A threshold is a number of its kind, never under its floor
(1 for a count such as `batch_parallel`; 0 where 0 turns it off). A value that is refused prints a `! bad_setting`
row naming where it came from (`FATS_CHUNK=…` or `settings.json chunk=…`) and the default is used. `fats config set`
checks a value the same way and refuses it instead of saving it (`! refused`, exit 1); a value with spaces is kept
whole.

Keys are sent only to their own provider and are never printed; `fats` and `fats config` show where a key came
from (env or `auth.json`), not its value.

### Telemetry

Once a day `fats` asks `https://fatsearch.com/v1/latest` for the newest version, and once a week (level `full`)
it posts last week's counts to `https://fatsearch.com/v1/l2`, both from a background process after the answer
is out. No URL, search, page text, file, key, IP or id is sent; the fields: [docs/telemetry.md](telemetry.md).
The first run prints a one-line notice on stderr, `~ anonymous usage stats on · what's sent: fats telemetry show · off: fats setup`, and sends nothing, nor does a process already running then (an
MCP server). A failed check is tried again after an hour, 3 times a day at most; everything is kept under
`<cache>/telemetry/`. `fats telemetry show` prints the next request exactly as it would go, the last check's
failure (`last check: <day> failed <why> · next after 1 h`) and the last ones sent (kept in `<cache>/telemetry/sent.jsonl`, the last
`telemetry_sent_keep`). `fats` and `fats telemetry` show `fats X available → <upgrade command>` when a newer
version is out.

## What it does not do (yet)

- **Search without a key.** `fats <words>` needs an Oxylabs Web API key or a Bright Data key + zone
  (`! no_key → fats setup`); `since:` is a hint, not a filter.
- **Semantic matching.** `match:near` and `match:topic` need a local model; they answer `! not_in_v1`.
- **OCR.** Scanned PDF pages are counted in a `! no_text` row, not read.
- **DOCX / XLSX.** Saved and reported as `! not_in_v1 zip`.
- **OS keychain.** Keys come from `auth.json` (0600) only.
- **Remote MCP.** `fats mcp` speaks stdio only; no HTTP transport, no OAuth.
- **Filters across pages.** A filter reads one page (`j3.u1 price`); a job-wide filter answers `! not_in_v1`.
- **Logins.** A page behind a login stays behind it; providers are called only with your own keys.
- **App pages.** A page that is only a JavaScript app to every provider stays `! no_copy`; `fats` serves what
  the bytes hold (card, islands) and stops.

## Build

```sh
cargo build --release --locked
./check.sh          # quick, every commit: fmt, clippy, tests (cargo-nextest), line budget; ~15 s warm
./check.sh full     # quick + cargo deny, version cooldown, release build, binary size; ~100 s warm
```

`check.sh` needs `cargo install --locked cargo-nextest` and `python3` (the line budget); `full` also needs
`cargo install --locked cargo-deny`. The parity test compares the output with a reference
implementation on a set of cached pages that is not part of this repository; without `FATS_PARITY` it skips
and says so.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your
option. Security reports: [SECURITY.md](SECURITY.md). Contributing: [CONTRIBUTING.md](CONTRIBUTING.md).
