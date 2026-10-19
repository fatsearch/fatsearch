---
name: fatsearch
description: >-
  Read web pages, PDFs and search results with `fats` (fatsearch) without flooding the context: URL → a ~300-token
  outline with section ids, then read only the section you need (`fats u1#3`), filter inside the page
  (`fats u1 price`), list cards or links. Plain fetch first ($0), then the user's Oxylabs / Bright Data key
  automatically, cached 30–60 min. Use when the task is "read / check / look at this page, URL or PDF", "what does
  this site say", "find X on this page", product, listing or price pages, web search → open results. Prefer it
  over tools that return a whole page as one blob. Not for building scrapers, bulk datasets or logged-in pages.
---

# fatsearch (`fats`): read pages without flooding context

The package is `fatsearch`; the command is `fats`. One binary, CLI and MCP server (`fats mcp`, one tool
`fats(cmd)` that takes the same command line). Not installed? `Read and execute https://fatsearch.com/agent-setup/SKILL.md`.
Cheat sheet: `fats guide` (one screen). Full reference: `docs/reference.md` in github.com/fatsearch/fatsearch.

## Rules (each one is a mistake an agent already made)

1. **The outline is a MAP, not the content.** Never judge "page thin / content missing" from the first output.
   An eBay outline read "Results (5 cards)" while the 68 listings sat in `u1#3` (12 parts). `fats u1 cards` or
   `fats u1 <words>` find them without reading 12 parts. Drill before concluding.
2. **Headings lie. Navigate by size, cards and filter, not by name.** The `(N)` on each outline row is the section's
   token size; the big one is usually the content. Then `fats u1 cards` (every listing / product card on the page)
   or `fats u1 <words>` (filter). `fats u1 all` = every section, one line each.
3. **Sections have parts.** A read ends `… 10.1k more → fats u1#3b`; `fats more` = the next part. Follow it.
4. **Check that the `=` title matches what you asked for.** A wrong or redirected URL is served "fine" (a bad Reddit id
   gave a random post; a Booking hotel URL gave a share page). Bad title = bad URL, not a `fats` failure.
5. **`!` rows are instructions.** They name the next move and its worst-case price: `! js_shell → fats u1 js:true
   ≤$0.0030 ~25s`. Do exactly that. `(card only)` / `og` = you got only the page's preview card.
6. **`pending` is not failed.** Run `fats u3 wait:30` (free). Never answer it with `fresh:true` (a new paid fetch).
7. **Don't dump.** `full` exists (the whole cleaned page, paged); use it only when you truly need everything. If you
   must, write it to a file and `grep` it; never paste it into the conversation.
8. **Comparing readers?** Judge `fats` after drilling (`cards`, `full`), same character cap on both sides, never the
   outline alone against another tool's whole page.

## The loop

```sh
fats <url>                 # outline: the = line + sections (size, preview) → pick one
fats u1#4                  # read section 4 (part 1); then `fats more` or `fats u1#4b`
fats u1 price vat          # filter inside the page (exact phrase first, then all words); match:exact
fats u1 cards              # every product / listing card; from:13 = the next page of them
fats u1 links              # numbered links → fats l7 opens one (dry:true first shows the price)
fats u1 comments           # a thread's top-level comments, most replies first; fats u1#5 = one comment + replies
fats u1 media              # photos; fats u1 m1 saves one (over MCP it returns the image)
fats u1 all                # every section, one line, when the outline says "+N sections"
fats u1 full               # the whole cleaned page, paged: last resort
```

Search: `fats rust async runtime limit:5` (results are unfetched ids, the top one prefetched for free),
`site:docs.rs`, `fats gh <words>` (GitHub repos by stars, free). PDFs and local files: `fats ./x.pdf`
(one section per page), `fats ./page.html`.

Mods: `dry:true` (the plan and its worst-case price, spends nothing) · `js:true` (force a render) · `fresh:true`
(refetch, paid, new id) · `wait:90` (block longer before `pending`) · `timeout:150` · `max:1k` (cap the output)
· `cost:0` (free plain fetch only) · `country:de` · `provider:plain|bd|oxy` · `json:true`.

## Reading the `=` line

`= j1.u1 <title> · host · 11 sections +2 island · 6 photos · 14.2ktok · plain:captcha_wall✗ bd✓ 18s spent $0.0015`

- `Ntok` on an outline = the whole page's size (what a dump would cost); on a read = what was printed.
- The route: `plain` free → `oxy` → `bd ∥ oxyjs` after 5 s. Every paid hop prints its `$`; a call `fats` cut itself
  reads `≤$X (unverified)`.
- `+N island` = big JSON / template blobs (app data) → `fats u1#N`; filters fall back to them on their own.
- `thin 0 of ~41k visible` = the served text is tiny next to the bytes → follow the `→` move on that line.

## Session and cache

- ids `u1…` belong to a session = the current directory (or `FATS_SESSION`). Another directory = other ids; a page
  from another session reads as `j8.u3`.
- A page dies 30 min after its last read, 60 min after its fetch → `! snapshot_missing → fats <url> fresh:true`.
- `fats a b c` = one batch, 4 fetches in parallel, one honest summary. The same URL twice = one fetch.
- `fats stats` = every fetch this session: provider, ms, cost, bytes → tokens, numbers lost. `fats` alone = status
  (keys present, cache, provider health). `fats config` = every setting and where it came from.

## Keys

None needed for plain pages. For JS, captchas and walls `fats` uses the user's own key, Oxylabs
(`OXYLABS_WEB_API_KEY`) or Bright Data (`BRIGHTDATA_API_KEY` + `BRIGHTDATA_UNLOCKER_ZONE`), whichever is set;
`fats setup` finds, checks and saves them. Never ask the user for a key `fats` already sees (`fats | grep '^keys'`).

## Not in v0.1

Logins · OCR (scans → `! no_text`) · DOCX / XLSX · semantic filter (`match:near` → `! not_in_v1`) · cross-page
filter · remote MCP (stdio only). Each prints a `!` row saying what is missing and what to do instead.
