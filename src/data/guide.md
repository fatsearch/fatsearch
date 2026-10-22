# fatsearch (`fats`): read pages without flooding context

`fats` (package fatsearch): CLI + MCP server (`fats mcp`, one tool `fats(cmd)`, same command line).
One screen: `fats help`.
Reference: github.com/fatsearch/fatsearch docs/reference.md.

## Rules (each one an agent mistake)

1. Outline = MAP, not content. Never judge a page thin from it: eBay's said "Results (5 cards)", 68 listings sat
   in u1#3 (12 parts). Drill: `fats u1 cards`, `fats u1 <words>`.
2. Headings lie. Go by size: row's (N) = its tokens; biggest = content.
3. Sections have parts: `… 10.1k more → fats u1#3b`. Follow it.
4. Check the = title. Wrong URL serves "fine" (bad Reddit id: random post; Booking hotel URL: share page).
   Bad title = bad URL, not a fats failure.
5. ! row = next move + worst price. Paste whole, exactly: `! js_shell → fats u1 js:true · bd, then oxyjs ≤$0.0030 ≈25s`
   (card only) / og = preview card only.
6. pending ≠ failed: `fats u3 wait:30`, free.
7. Don't dump. full = last resort: to a file, then grep. Never into chat.
8. Comparing readers? Drill first (cards, full), same char cap both sides. Never outline vs whole page.
9. No URL? Still the web: a price, "X vs Y", cheapest, news, "what X said", "that page" = `fats <words>`
   first; never from memory, never ask for a URL. A price or fact: the vendor's / source's own page, never a blog.

## Commands

```sh
fats <url>          # outline: = line + sections (size, preview)
fats u1#4           # section 4 part 1; next part: fats more or fats u1#4b
fats u1 price vat   # filter: exact phrase, then all words; match:exact
fats u1 cards       # product / listing cards; from:13 = next page
fats u1 links       # numbered; fats l7 opens one, fats l7 dry:true = price first
fats u1 comments    # top level, most replies first; fats u1#5 = one + replies
fats u1 media       # photos; fats u1 m1 saves one (MCP: returns it)
fats u1 all         # every section, one line (outline says +N sections)
fats u1 full        # whole cleaned page, paged
fats rust async runtime limit:5  # web search: unfetched ids, top one prefetched free; site:docs.rs
fats gh <words>     # GitHub repos by stars, free
fats ./x.pdf        # section per page; fats ./page.html too
fats a b c          # batch: 8 in parallel, one summary
fats stats          # session index: a row per fetch, provider ms cost tok
fats stats full     # + bytes → tokens, numbers lost
fats                # status: keys, cache, provider health
fats config         # every setting + its source
fats telemetry show # anonymous usage stats (no URL, search, key), as sent
fats setup telemetry off  # stop them
```

Mods: dry:true plan + worst price, $0 · js:true force render · fresh:true refetch, paid, new id · wait:30 s
before pending · timeout:150 · max:1k cap output · country:de · provider:plain|bd|oxy|decodo ·
json:true.

## The = line

`= j1.u1 <title> · host · 11 sections +2 island · 6 photos · 14.2ktok · plain:captcha_wall✗ bd✓ 18s spent $0.0015`

- Ntok: outline = whole page (dump cost); read = what it printed.
- Route: plain free → oxy, decodo (after captcha) → bd ∥ oxyjs after 5 s → decodojs. Keyed tiers only;
  dry:true shows the real one. Paid hop prints its $; call fats cut = ≤$X (unverified).
- +N island = JSON / template blobs (app data): read like a section; filters fall back to them.
- thin 0 of ~41k visible = served text tiny vs bytes: follow the → (that line's or its ! row's).

## Session, cache

- ids u1… belong to a session = current dir (or FATS_SESSION): other dir, other ids; other session's page
  reads j8.u3.
- Page dies 30 min after last read, 60 after fetch: `! snapshot_missing → fats https://ex.com/ fresh:true`.

## Keys

Plain pages: none. Web search, JS, captchas, walls: ONE key: Oxylabs (default) · Bright Data · Decodo (pages
only, no search). Keys live in auth.json only; `fats | grep '^keys'` shows each one's last 4 chars.
**Never ask for a key in the chat.**

- `fats setup` = check every key, $0, writes nothing.
- `fats setup oxy` (or bd, decodo, gh): dashboard link + local form URL. **Give the human its 3 lines only**
  (one provider, no MCP), end your turn, `fats setup` when they say done. SSH? They pipe:
  `printf %s "$KEY" | fats setup oxy`, never as an argument.
- `fats setup oxy off` / on: provider off / on; key kept.
- `fats setup free off` = no free plain fetch (user's IP, paced 1 start/site/s). Say the cost first: every page
  paid, $0.0010–0.0015 a call (dry:true = page's worst).

## Faster or cheaper (only when the user asks)

`fats config` rows build · limits.json = knobs. Try as mod (batch_parallel:8), then env FATS_BATCH_PARALLEL=8,
or keep: `fats config set batch_parallel 8` (`fats config unset batch_parallel` = default); `fats config` shows
the source. Useful: batch_parallel (8 lanes), cache_idle_s / cache_ttl_s, read_cap, plain_host_pace_ms
(home-IP brake: lower only on user's say-so). Per call: hedge: wait: timeout: cost:

## Not yet

Logins · OCR (scans → ! no_text) · DOCX / XLSX · semantic filter (match:near → ! not_in_v1) · cross-page filter ·
remote MCP (stdio only). Each prints a ! row: what's missing, what instead.
