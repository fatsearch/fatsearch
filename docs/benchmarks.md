# Benchmarks

What I measure, how, and what came out. One Apple Silicon Mac (macOS, arm64), release builds, real providers, real
pages. Most numbers are one run each; where I have several runs I give the range, because the providers' own speed
and luck move results more than most code changes do.

## Method

- **Fresh shell per key mode.** Every command runs in an `env -i` shell with a scratch `HOME`, cache and key
  directory per mode, keys passed through the environment only. Modes: Oxylabs Web API key only, Bright Data key
  (+ Unlocker zone) only, both keys.
- **One run per page per mode**, `fats <url> fresh:true wait:90 json`: no cache, the background run given up to
  90 s before the answer is read. One host in flight at a time, three pages in parallel.
- **Judge = does the first answer hold the page's content.** A small model (Claude Haiku, 3 samples) reads the
  answer `fats` printed, not the raw page, and says whether it holds the page's real content (the post, the thread,
  the listing, the article) rather than a wall, a shell or an error page. Served = p ≥ 0.5; "unsure" = 0.3 ≤ p <
  0.7. I spot-read the unsure ones.
- **Provider weather.** The same build, run a minute apart, has moved by up to 3 pages. A single run is a sample,
  not a score; compare bands, not rows.
- **Money** is the provider's list price as `fats stats` prints it. Bright Data was checked against its own zone
  counter (exact on runs where nothing else used the zone). Oxylabs has no usage counter for the Web API key, so
  calls `fats` cut are printed as unverified upper bounds.

## The hard pages

Pages a plain request does not get: JavaScript apps, captcha and Cloudflare walls, sites that block datacenter
traffic. 20 pages; one is left out of every count (a product URL that now redirects to a category page, so every mode
"served" it), 19 counted. Of the 19:

| host | what it is |
|---|---|
| tiktok.com | two video posts (caption, author) |
| instagram.com | a post |
| reddit.com | two threads (one English, one in a non-English subreddit) |
| stackoverflow.com | two questions with answers |
| forum.joomla.org | a forum topic behind Cloudflare |
| youtube.com | a search results page |
| nytimes.com | a news article |
| ebay.com | an item page |
| zara.com | a product page in a non-English locale |
| trivago.com | a hotel page |
| booking.com | a hotel page |
| airbnb.com | a listing |
| imdb.com | a title page |

The other 3 counted pages are two real-estate listings and a collectibles shop product page on regional European
sites. I don't list them here.

## Results: hard pages, time to an answer

38 hard pages: social posts, news, shops, travel, classifieds and search pages that an agent's built-in web fetch
failed on. Each page has one question with a known answer; a reader counts on a page when the answer is in the
text it returned **and** the call ended within the time. A refusal, a wall or an empty page never counts, however
fast it came back. The run had 40 pages; the 2 pages of one government site are dropped for every reader (on
2026-10-20 it blocked every reader we ran, and no hosted reader ever read it).

FATSEARCH: both keys (Oxylabs + Bright Data), `fats <url> fresh:true wait:90 json`, a fresh `env -i` shell with a
scratch cache, three pages in parallel, one per host. One run per reader and date.

| reader (run date) | answered /38 | within 1 s | 10 s | 20 s | 60 s |
|---|--:|--:|--:|--:|--:|
| FATSEARCH 0.2.0 (2026-10-21) | **35** | 12 | 19 | 19 | **35** |
| FATSEARCH 0.1.3 (2026-10-20) | 36 | 14 | 20 | 28 | 36 |
| FATSEARCH 0.1.0 (2026-10-16) | 35 | 12 | 17 | 27 | 35 |
| Firecrawl (2026-10-16) | 24 | 0 | 22 | 24 | 24 |
| Tavily (2026-10-16) | 19 | 9 | 17 | 18 | 19 |

**0.2.0 is slower than 0.1.3 on this run** (19 vs 28 pages within 20 s; 13 pages slower, 1 faster, one new miss on
TikTok). On most of the slower pages the paid call that won came back later. An A/B run of the 0.1.3 binary ten minutes
after 0.2.0 (same script, keys and pages) was as slow: 33/38 answered, 12/20/21/33 within 1/10/20/60 s. Bright
Data's winning call took a median 23.8 s then, against 13.9 s the day before. So this is the providers' hour, not
a 0.2.0 change (`measure/2026-10-20-hard-pages-0.2.0`, sub-folder `ab-0.1.3`).

Same pages, both answered, medians (FATSEARCH first). FATSEARCH loses on speed here, because it starts with the
free request and climbs to a paid render only when that fails:
- 0.2.0 vs Firecrawl: n=24, 23.7 s vs 4.9 s (0.2.0 faster on 10). 0.1.3 vs Firecrawl: n=24, 9.3 s vs 4.9 s.
- 0.2.0 vs Tavily: n=18, 23.7 s vs 1.0 s (0.2.0 faster on 4). 0.1.3 vs Tavily: n=19, 15.4 s vs 1.4 s.

Firecrawl and Tavily ran on 2026-10-16, FATSEARCH on the dates shown. Pages and providers change between days, so
a cross-day row is a sample, not a ranking. Data: `measure/2026-10-20-hard-pages-0.2.0`,
`measure/2026-10-20-hard-pages-0.1.3`, `measure/2026-10-16-hard-pages-speed` (`results.json` in each, in the
website's source).

### Earlier: 19 pages, one key (2026-10-15)

Nine runs on 2026-10-15, each on the build of that moment (the pre-release builds that became 0.1.0).
p50 = median time to the first answer over all 19 pages, served or not.

| time (EEST) | Oxylabs key only | p50 | Bright Data key only | p50 |
|---|---|---|---|---|
| 03:33–03:53 | 13/19 | 46 s | 14/19 | 21 s |
| 04:37–04:53 | 12/19 | 36.3 s | 8/19 | 17.1 s |
| 06:50–07:08 | 14/19 | 22.2 s | 13/19 | 17.5 s |
| 07:47–08:05 | 13/19 | 30.6 s | 11/19 | 12.9 s |
| 09:15–09:40 | 16/19 | 30.5 s | 8/19 | 23.6 s |
| 11:10–11:26 | 15/19 | 30.3 s | 12/19 | 28.4 s |
| 12:13–12:40 | 14/19 | 30.7 s | 12/19 | 20.9 s |
| 14:05–14:35 | 14/19 | 42.7 s | 10/19 | 25.5 s |
| 17:26–17:45 | 16/19 | 27.2 s | 9/19 | 37.7 s |

**Oxylabs key 12–16 of 19, Bright Data key 8–14 of 19 over nine runs.** Three earlier runs on the same 19 pages
fell in the same bands: 2026-10-14 18:15–18:37 (Oxylabs 16 and 15 in two passes, Bright Data 14), 2026-10-14
21:19–21:40 (Oxylabs only, 16) and 2026-10-15 01:59–02:40 (Oxylabs 14, Bright Data 10). Both keys together served 14 of the 20 in
the one run made with both (2026-10-14). Before the any-one-key ladder, one key got 10 and 12 of 20.

What loses, in words: Instagram posts (shells or "Post isn't available" pages), the YouTube results page on Bright
Data (the results sit in a script block, see the outline's islands), renders cut at 60 s (TikTok, the Joomla forum),
Bright Data relaying a site's 502, and Reddit's captcha when it lands on both the plain request and the provider.

Spend for one 19-page pass: $0.017–0.028 billed per key mode at list prices, plus up to $0.013 in calls `fats` cut,
printed as unverified.

## Results: easy pages

Seven pages fetched in parallel in one session, both keys set, 2026-10-15 17:26–17:45: example.com, a Wikipedia
article, the Hacker News front page, a docs.rs crate page, an arXiv PDF, gov.uk, and a host that does not exist.
All seven answered from the free plain request, $0.

| page | time |
|---|---|
| host that does not exist (`! no_host`) | 101 ms |
| example.com | 120 ms |
| gov.uk | 144 ms |
| docs.rs crate page | 210 ms |
| arXiv PDF | 442 ms |
| Wikipedia article | 483 ms |
| Hacker News front page | 732 ms |

## Other readers

The same 38 pages through three hosted readers and Claude's built-in tools, one run each.

- **FATSEARCH:** as above.
- **Firecrawl:** v2 scrape with default settings, plus one enhanced-proxy pass on its failures; the better of the
  two counts.
- **Tavily:** Extract, advanced depth.
- **Jina Reader:** no key.

For the agent rows, Claude Haiku got one tool and the question. With FATSEARCH the tool runs the command line;
with Firecrawl it returns Firecrawl's saved copy, cut to 1,000 tokens. "Built-in" is Claude's own web search and
web fetch tools.

| of 38 pages | FATSEARCH | Firecrawl | Tavily | Jina Reader (no key) | Built-in fetch |
|---|---|---|---|---|---|
| Answer in the returned text | **35** (0.2.0, 10-21) · 36 (0.1.3, 10-20) · 35 (0.1.0, 10-16) | 24 (10-16) | 19 (10-16) | 8 (10-16) · 17 (10-20) | 17, page read (10-16) |
| Agent answered correctly (10-16) | **26** | 24 | — | — | 17 |
| Median input tokens per question (10-16) | **2.6k** | 2.8k | — | — | 8.3k |

Where readers miss is as telling as how often (2026-10-16):
- Firecrawl declined 9 pages outright as sites it does not support (social networks, a news site, a recipe site,
  an airline).
- Tavily tried and came back empty on 16.
- Jina's keyless tier refused 21 domains for anonymous use on 2026-10-16 and only 3 on 2026-10-20, so its number
  is about access, not extraction.
- Four answers have gone stale since the questions were written; FATSEARCH 0.1.0 read all 34 that have not.

In the agent runs, FATSEARCH answers "pending" after 5 seconds while a slow page is still loading. The model often
gave up there, which cost it more points than any page it failed to read. Agent rows: the 2026-10-16 run, FATSEARCH
0.1.0 (`measure/2026-10-16-hard-pages-speed`).

## Tested in agents: Claude Code, 51 tasks, FATSEARCH MCP vs Firecrawl MCP (2026-10-19)

The same 51 web tasks (docs, news, shops, forums, social, PDFs, travel, bot walls, searches) in Claude Code
2.1.280 with `claude-sonnet-5`, headless, built-in web tools off, one run per side. FATSEARCH MCP 0.1.3 with an
Oxylabs key against Firecrawl's hosted MCP with an API key. Right = the agent's final answer matched the known one.

| | FATSEARCH | Firecrawl MCP |
|---|--:|--:|
| Right answers | 50/51 | **51/51** |
| Cost per 1,000 tasks (Claude + the tool) | **$15.06** | $53.48 |
| Tokens per task, median | **5,026** | 38,129 |
| Time per task, median | **5.3 s** | 6.7 s |
| Time per task, slowest tenth (p90) | 43.8 s | **21.5 s** |

Firecrawl wins on right answers (FATSEARCH missed one Spotify page) and on the slow tail: FATSEARCH waits for
the paid render on blocked pages. Cost = Claude's bill plus the tool: Firecrawl credits at its Hobby plan,
FATSEARCH the provider cost `fats` prints. Data: `measure/2026-10-20-claude-code-firecrawl-vs-fats` (`results.json`,
`paired_repeat1`).

## Tested in agents: Claude Code WebFetch gets 403, FATSEARCH reads the page (2026-10-19)

Claude Code 2.1.280's WebFetch returned `HTTP 403 Forbidden` on an npm package page, a Medium post and a G2 reviews
page. FATSEARCH 0.1.3 read all three: npm and Medium through the free request (345 ms and 1,479 ms, $0), G2 through
Bright Data after the plain request hit a captcha (48.1 s, $0.0015). One run each. Data:
`measure/2026-10-19-web-fetch-403` (`meta.json`, the raw outputs).

## Tested in agents: open-source readers, the same Wikipedia page (2026-10-19)

`https://en.wikipedia.org/wiki/Rust_(programming_language)`, default settings, tokens counted with Anthropic's
token counter. "Answer" = the three answer strings are in the text.

| reader, call | tokens | answer |
|---|--:|---|
| FATSEARCH 0.1.3: outline, one filter, one section | **1,384** | yes |
| mcp-server-fetch, default (5,000 characters) | 2,373 | no |
| mcp-server-fetch, whole page | 76,147 | yes |
| Crawl4AI MCP `md` | 74,896 | yes |
| mcp-searxng `web_url_read` | 117,088 | yes |
| open-webSearch `fetchWebContent`, default (30,000 characters) | 11,609 | yes |
| Firecrawl self-hosted `/v2/scrape` | 107,332 | yes |

The cut-short defaults are smaller than FATSEARCH's whole read only when they cut the answer out (mcp-server-fetch
default). Data: `measure/2026-10-19-wikipedia-rust-oss` (`counts.json`).

## Binary and startup

- Release binary, symbols stripped, macOS arm64, 0.2.0: **17,405,936 bytes** (17.4 MB; 0.1.0: 16,703,872, +4.2%
  for the setup menu and key forms, the third provider, the batch pool and the update check). Before stripping (0.1.5): 19.6 MB. The
  size gate in `check.sh full` fails a build more than 5% over its record, re-recorded at 0.2.0.
- Release binary, Linux x86_64 (GitHub's `ubuntu-latest` runner, `check.sh full`), 0.2.0: **20,664,112 bytes**
  (20.7 MB, +20% over macOS arm64); the size gate holds it to its own record.
- Startup, 0.1.0 release build, `python3 scripts/startup.py --runs 100`, three passes, 2026-10-16 00:08 EEST
  (load average 8–9): `fats guide` p50 **4.07–4.13 ms** (p90 4.30–4.57 ms), `fats config` p50 4.32–4.41 ms,
  against 1.02–1.08 ms for `/usr/bin/true` on the same machine. Startup moves with machine load; compare only
  runs made side by side.
- These are the native binary's numbers. Installed from npm, `fats` starts through a Node shim that adds Node's
  own start, ~40 ms.
- Token estimate: 42 default reads (pre-release build, 2026-10-14) counted with Anthropic's token counter
  (claude-opus-5-5). Recomputed offline on those stored bytes and counts with today's rule (no new count): the
  estimate of the printed bytes is within ±10% on 36 of the 42 (median 1.01 real per estimated token). A whole
  read's `Ntok` counts its `=` line (timing at a fixed width) and rows: 1.01 real tokens per `Ntok` on the 5
  whole reads (0.95–1.12, 4 of 5 within ±10%; counting the rows alone, 1.16 and 2 of 5; 3.45 with the old
  chars / 4). example.com, counted with claude-haiku-4-5 (2026-10-20): its rows are 66 real tokens, rows + `=` line
  106; the old figure printed `93tok` (rows only) and `stats` 52. The read now prints `143tok` on both; haiku-4-5
  counts 1.13–1.41× fewer than opus-5-5 on these bytes, so 106 haiku ≈ 120–150 opus.
- A cached read: 5.1 ms (0.1.0). A saved TikTok shell re-read offline with its next step: 22–39 ms.
- `timeout:8` with both keys ends at 8.09 s (0.1.0): one wall for every render in the run.
- A 53 MB HTML page is refused at the default 16 MiB cap in 10 ms (27 MB RSS); with `max_bytes:64m` it is cleaned
  in 2.95 s at 743 MB RSS. A 16 MB page: 0.88 s, 239 MB RSS.

## One page, six readers

2026-10-17 17:43 EEST, one run each, default settings. Page: `https://en.wikipedia.org/wiki/Rust_(programming_language)`.
Tokens = Anthropic `POST /v1/messages/count_tokens` (a real tokenizer, not an estimate) over the kept captures;
bytes = the capture file's size. The README's race picture and its cost table come from this run: the picture's
1,384 is outline + filter + section, the table's 918 is outline + section.

| reader (default settings) | bytes returned | tokens |
|---|--:|--:|
| Firecrawl v2 `/scrape`, markdown | 239,357 | 112,517 |
| Bright Data Web Unlocker, markdown | 218,850 | 103,625 |
| Oxylabs Web API `/v1/scrape`, markdown, no JS | 216,346 | 102,609 |
| Tavily `/extract`, basic | 206,278 | 97,504 |
| Jina Reader `r.jina.ai` (no key) | 201,602 | 94,415 |
| `fats <url>` (outline) | 1,263 | 618 |
| `fats u1 "Graydon Hoare"` (one filter) | 1,104 | 466 |
| `fats u1#37` (one section) | 768 | 300 |

The raw HTML (`curl`, ~1 MB, ~530k tokens) was not kept, so it is not in the table.

The fetch was plain HTTP for `fats` ($0). The "model bill" column in the README assumes $2 per million input
tokens (Claude Sonnet 5.5 input, checked 2026-10-19); put your model's price in. The raw captures are kept with the website's source
(`measure/2026-10-17-wikipedia-rust`); the website keeps its runs and counts at [fatsearch.com/benchmarks](https://fatsearch.com/benchmarks/).
