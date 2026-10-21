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

## Not installed?

1. `fats --version` prints `= fats <version> · …`? Go to 4.
2. Install one way: `brew install fatsearch/tap/fatsearch` · `npm i -g fatsearch` · `cargo install --locked fatsearch` ·
   `curl -fsSL https://github.com/fatsearch/fatsearch/releases/latest/download/fatsearch-installer.sh | sh`
   (Windows: `npm i -g fatsearch`).
3. Keys are optional (plain pages cost $0). Keys already in the environment work as they are. Never ask for a key
   in chat: `fats setup oxy` (or `bd`, `decodo`) prints a local form; pass its steps to the user.
4. Connect: `claude mcp add --scope user fats -- fats mcp` · `codex mcp add fats -- fats mcp` · other hosts: a
   stdio server, command `fats` (absolute path for GUI apps), args `["mcp"]`.
5. Verify: `fats https://example.com` prints a first line with `Example Domain`.
6. Ask the user one question: which page or search to start with.
