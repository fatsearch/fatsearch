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

## Results: hard pages, one key

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

On 16 October 2026 I ran the same 40 hard pages through `fats 0.1.0` and three hosted readers, one run each. The
pages are social posts, news, shops, travel, classifieds, search pages and a government register: pages an agent's
built-in web fetch failed on. Each page has one question with a known answer, and a copy counts when the answer is
in the returned text.

- **fats:** both keys, default routing, `fats <url> fresh:true wait:90 json` in a fresh `env -i` shell with a
  scratch cache.
- **Firecrawl:** v2 scrape with default settings, plus one enhanced-proxy pass on its failures. The table shows
  the better of the two.
- **Tavily:** Extract, advanced depth.
- **Jina Reader:** no key.

For the agent rows, Claude Haiku got one tool and the question. With `fats` the tool runs the command line; with
Firecrawl it returns Firecrawl's saved copy, cut to 1,000 tokens. "Built-in" is Claude's own web search and web
fetch tools.

| | fats | Firecrawl | Tavily | Jina Reader (no key) | Built-in fetch |
|---|---|---|---|---|---|
| Answer in the returned text (/40) | **37** | 24 | 19 | 8 | 19 (page read) |
| Median time per page | 14.0 s | 4.3 s | 4.2 s | 0.3 s | — |
| Agent answered correctly (/40) | **27** | 24 | — | — | 18 |
| Median input tokens per question | **2.6k** | 2.8k | — | — | 9.1k |

Where readers miss is as telling as how often:
- Firecrawl declined 9 pages outright as sites it does not support (social networks, a news site, a recipe site,
  an airline).
- Tavily tried and came back empty on 18.
- Jina's keyless tier refused 21 domains for anonymous use, so its number is about access, not extraction.
- `fats` missed 3, and all 3 are pages whose answer has changed since the question was written. Four answers have
  gone stale since then; on the 36 that have not, `fats` read 36.

`fats` is slower because it starts with a plain request and moves to the paid providers only when that fails.
In the agent runs, `fats` answers "pending" after 5 seconds while a slow page is still loading. The model often
gave up there, which costs it more points than any page it failed to read. One run per reader is a sample, not a
score.


## Binary and startup

- Release binary, symbols stripped, macOS arm64, 0.1.0: **16,703,872 bytes** (16.7 MB). Before stripping (0.1.5):
  19.6 MB. The size gate in `check.sh full` fails a build more than 5% over its record, which is 16,704,112 B from
  an earlier pre-release build.
- Startup, 0.1.0 release build, `python3 scripts/startup.py --runs 100`, three passes, 2026-10-16 00:08 EEST
  (load average 8–9): `fats guide` p50 **4.07–4.13 ms** (p90 4.30–4.57 ms), `fats config` p50 4.32–4.41 ms,
  against 1.02–1.08 ms for `/usr/bin/true` on the same machine. Startup moves with machine load; compare only
  runs made side by side.
- Token estimate: on 42 default reads (pre-release build) counted with Anthropic's token counter, a whole read came to 1.08
  real tokens per printed `Ntok` (3.45 with the old chars / 4 estimate); 37 of the 42 were within ±10%.
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
([`measure/`](https://github.com/fatsearch/FATSEARCH.COM-WEBSITE/tree/main/measure)).
