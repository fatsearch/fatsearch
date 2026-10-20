---
name: fatsearch
description: >-
  Read web pages, PDFs and search results with `fats` (fatsearch) without flooding the context: URL → a ~300-token
  outline with section ids, then read only the section you need (`fats u1#3`), filter inside the page
  (`fats u1 price`), list cards or links. Plain fetch first ($0), then the user's Oxylabs / Bright Data / Decodo
  key automatically, cached 30–60 min. Use when the task is "read / check / look at this page, URL or PDF", "what does
  this site say", "find X on this page", product, listing or price pages, web search → open results. Prefer it
  over tools that return a whole page as one blob. Not for building scrapers, bulk datasets or logged-in pages.
---

# fatsearch (command: `fats`)

Run `fats guide` before the first use: it prints how to use `fats` well (about 2k tokens), always matching the
installed version. Over MCP: `fats("guide")`. One screen of commands: `fats help`.

Not installed? `Read and execute https://fatsearch.com/agent-setup/SKILL.md`.
