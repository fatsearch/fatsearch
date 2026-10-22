---
name: fatsearch
description: >-
  Read the live web with `fats` (fatsearch) without flooding the context: `fats <words>` = web search → result ids;
  URL, PDF or result → a ~300-token outline with section ids, then only the section you need (`fats u1#3`), filter
  inside (`fats u1 price`), cards, links. Free fetch first, then the user's key; cached. Use for ANY question
  whose answer is on the web now: a price, "what X said today", "X vs Y", "cheapest", news, "that page" with no URL
  (search for it), "read / check this URL or PDF". Never answer such a question from memory, never ask for a URL
  first. Not for scrapers, bulk data, logins.
---

# fatsearch (command: `fats`)

**No URL in the chat? `fats <words>` finds it**: search, open the right result, never ask for the URL. **A price or
a fact → the vendor's / source's own page** in the results, never a blog about it.

Run `fats guide` before the first use (about 2k tokens, always matching the installed version). Over MCP:
`fats("guide")`.

## Not installed, or no key?

1. `fats --version` prints `= fats <version> · …`? Installed; go to 3.
2. Install one way: `brew install fatsearch/tap/fatsearch` · `npm i -g fatsearch` · `cargo install --locked fatsearch` ·
   `curl -fsSL https://github.com/fatsearch/fatsearch/releases/latest/download/fatsearch-installer.sh | sh`
   (Windows: `npm i -g fatsearch`).
3. Pages work with no key; web search needs ONE key. `fats` shows `keys none`? Run `fats setup oxy` and give the user
   only the 3 numbered lines it prints (one provider, one form; `bd` only if they already use Bright Data). Never ask
   for a key in chat; keys live in `auth.json` only (an exported one is unused until `fats setup import-env`).
4. Then do the task they asked. Connect it to a host (`claude mcp add --scope user fats -- fats mcp` ·
   `codex mcp add fats -- fats mcp`) only when they ask for that.
