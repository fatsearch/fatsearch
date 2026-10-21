# Changelog

All notable changes to `fats` are in this file. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions: [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Numbers come from my own runs on one Apple
Silicon Mac, one run each unless a range is given; provider results move run to run.
Method: [docs/benchmarks.md](docs/benchmarks.md).

## 0.2.2 - 2026-10-21

### Changed

- Windows installs through npm (`npm i -g fatsearch`). The Claude Code, Codex and Gemini CLI agents ship npm
  packages, so most Windows setups already have npm. Without Node: the `fatsearch-x86_64-pc-windows-msvc.zip`
  from the releases page.
- Telemetry: on Windows a binary in `%USERPROFILE%\.cargo\bin` reports `via=cargo`.

### Fixed

- Windows: a page's cache path in a record and in `fats stats` uses one separator (`…\cache\t\u1`, was
  `…\cache\t/u1`), and the default key dir and cache follow `%APPDATA%` and `%LOCALAPPDATA%` as the install
  guide says (the system's known folder when one is unset or not an absolute path).
- Windows: a stored cache entry that names a drive (`t/C:x`) can no longer point a read outside the cache.
- Windows: a `file://host/path` URL is refused (it opened a network share); `fats` reads local files only.

## 0.2.1 - 2026-10-21

### Changed

- No musl (static Linux) binaries: the 0.2.0 release build failed on both musl targets (BoringSSL built with the
  host compiler links glibc symbols musl lacks). The five 0.1.3 targets ship; musl returns with a proper cross toolchain.
- The dependency cooldown gate reads its waivers from `scripts/cooldown-waivers.txt` as well as commit lines, so a
  release squashed to one commit keeps them. The release export runs the gate before the preview push. No change to
  the binary.
- The agent setup skill lives in this repo at a release tag and installs through the Agent Skills CLI
  (`npx skills add fatsearch/fatsearch`); the website no longer serves executable instructions. `fats guide`
  drops its install pointer, and the line that points at it now reads `agents: fats guide = the manual`.

## 0.2.0 - 2026-10-20

### In short
- New: anonymous usage stats, on by default; `fats telemetry show` = what is sent, `fats setup telemetry off` = off.
- New: Decodo Web Data API as a third paid provider (`DECODO_AUTH_TOKEN`), without JS and rendered.
- New: `fats setup` is a menu on a terminal. Piped or over MCP it opens a one-time key form on `127.0.0.1`.
- New: `fats setup <provider> on|off` turns a provider off without deleting its key.
- New: `fats guide` prints the whole usage guide; `fatsearch` works as a second name for `fats`.
- New: `fats config set|unset` changes any threshold; `fats config` lists 88 settings with their source.
- New: the free fetch starts at most one request per site per second from your IP; a page waits, it never pays.
- New: a host that does not resolve ends at $0 (`no_host`, `dns_failed`, `dns_timeout`) before any paid call.
- Changed: keys live in one `auth.json` (mode 0600); the 0.1 key files move in by themselves on the first run.
- Changed: `fats stats` is a short index of the session's searches and pages; `fats stats full` is the old view.
- Changed: a batch's `cost:` is one pool for all its pages, and a batch runs 8 pages at once (was 4).
- Changed: a failed page names the missing keys that could still serve it and points to `fats setup`.
- Removed: `./.env` and the Bright Data CLI's files are no longer read for keys; use the environment or `fats setup`.
- Removed: the `timeout:150` "one more try" after a cut render, and the `render_fix_s` setting.
- Fixed: a domain-like word inside a search (`contact at acmebuilders.com phone`) stays a word, not a `site:` search.
- Fixed: path-like words, a lone `-` and search operators (`filetype:pdf`) go to the search as typed.
- Fixed: reading a result right after a search waits for its page instead of answering `pending 0s`.
- Fixed: a failed certificate ends at $0; an https server that refuses the connection is tried over http at once.

### Added
- Anonymous usage stats, on by default, with one notice on stderr the first time,
  `~ anonymous usage stats on · what's sent: fats telemetry show · off: fats setup` (that run, and any process
  already running then, an MCP server too, sends nothing). Once a day `fats` asks `https://fatsearch.com/v1/latest`
  for the newest version, from a background process after the answer is out (a failed check: again after an hour,
  3 tries a day; `fats telemetry show` prints `last check: <day> failed <why> · next after 1 h`): the request carries the fats version, OS, CPU, the package it came from (npm, Homebrew, cargo,
  the install scripts), the month of the first run, how many of the last 7 days it ran, and the level. Once a
  week, at level `full`, it posts last week's counts to `/v1/l2`: commands by kind (`read`, `search`,
  `filter`, …), the agent host that ran them (Claude Code, Codex, pi, MCP, a terminal), calls per provider tier
  with ok / failed / billed counts, error codes from fats' own list, token and time medians, and where fats
  itself panicked (`outline.rs:412 index out of bounds`, no message values). The counts are kept in the cache
  (`telemetry/week/<year>-W<nn>.json`, this week and last; only at level `full`). Never sent: URLs, site names, searches, filter words, page text,
  files, keys, your IP address, any id. `fats telemetry show` prints the next request byte for byte and the last
  20 sent ones; `fats setup telemetry full|minimal|off` changes it (`minimal` = the version check only), or the
  menu `fats setup → Settings → Telemetry` (`On (anonymous usage stats)` = `full`, `Minimal (update check only)`, `Off`);
  `DO_NOT_TRACK`, `CI` (and `GITHUB_ACTIONS`, `BUILD_NUMBER`, `TF_BUILD`, `TEAMCITY_VERSION`) and `FATS_OFFLINE`
  set to anything but empty, `0`, `false`, `no` or `off`, or `FATS_TELEMETRY=0`, turn it off whatever is set;
  `FATS_TELEMETRY_DEBUG=1` prints each request instead of sending it. `fats` and `fats telemetry` show
  `fats X available → <the upgrade command for how you installed it>` when a newer version is out, and the run
  after an upgrade prints `~ fats updated to X` with the release's note. What is sent and why:
  [docs/telemetry.md](docs/telemetry.md).
- `fats guide` prints the whole usage guide (the rules, the loop, reading the `=` line, sessions, keys; about
  2.0k tokens), built into the binary, so it always matches the installed version. `fats help`, `-h` and
  `--help` stay the four-line card, now with a last line `agents: fats guide = the skill (~2.0k tok)`; `fats`
  (the status) says it on line 2, under the `=` line, and the same line ends `fats --version`, `fats setup` and the answer to a word that is no command (`whoami`, `info`,
  `install`). The MCP server keeps the card in its instructions and adds `First time? Call fats("guide")`, also
  at the end of the tool description. The installed skill (`skills/fatsearch/SKILL.md`) is now its description
  plus `Run fats guide before the first use`: update it with `npx skills update fatsearch`.
- `fatsearch` as a second name for `fats`, added by Homebrew, npm and the install scripts (`cargo install` adds
  only `fats`), so the package name works as a command too.
- Decodo Web Data API as a third paid provider (`DECODO_AUTH_TOKEN`, `fats setup` checks it for $0): `decodo`
  without JS ($0.0010) and `decodojs` rendering ($0.0015), `provider:decodo`, priced in `dry:true`, `stats` and every
  tried path. With three keys the page ladder is plain → oxy → decodo (both after plain's captcha) → bd ∥ oxyjs →
  decodojs one hedge later; `js:true` tries bd → oxyjs → decodojs one at a time. A page whose no-JS fetch was cut at
  its timeout goes straight to the renders instead of trying the next provider's no-JS fetch on the same slow page
  (a booking page spent 10 s + 15 s there); `dry:true` still quotes that rung, since a no-JS fetch that fails at a
  wall leads to it. Decodo shows up wherever providers are listed: the status `keys` line (`decodo:none`) and health
  rows (`decodo no key`, `decodojs no key`), the `config` rows for both tiers, `provider:plain|bd|oxy|decodo`,
  `device:mobile (oxy, decodo)`, and `fats setup` (also `dry:true`): `decodo none` with its `! no_key` row and
  sign-up page.
- `fats setup` on a terminal is a menu (↑↓ Enter): the free fetch and each provider with its state, add or replace
  a key (hidden paste, checked live), turn one off or on, and a Settings submenu: Agent connection (MCP) · Cache (its folder, how long pages
  live, pages and size) · Stats (this session's pages and money) · Telemetry. `fats setup oxy` (also `bd`, `decodo`,
  `gh`) jumps to one, `fats setup telemetry` to the telemetry screen. Piped or over MCP, `fats setup oxy` opens a one-time key form on `127.0.0.1` for 10 minutes and prints the
  steps for the agent to hand to the human (dashboard URL, then the local form URL); the key never passes through
  the chat, and the form refuses a POST whose `Origin` is not its own (400, no provider call). `fats setup` alone,
  piped, is the dry check of every key at $0.
- The Bright Data key form saves the configured zone or the account's only one with the key; with several and none
  picked it says `key saved · no zone yet → tell your agent: fats setup bd zone:<name>` and lists them. A saved
  Bright Data key without a zone reads `! no_zone bd · key saved, no Web Unlocker zone yet → fats setup bd
  zone:<name>` in dry runs and failure rows, never `! no_key`, and `no zone` in the status health; `fats setup bd zone:<name>` runs the setup check with that
  zone.
- `fats setup` ranks providers by a dated measurement on 19 hard pages, each figure with its sample: Oxylabs
  `#1 · 14/19 hard pages (median of 9 runs, 12–16, 2026-10-15)`, Decodo `14/19 hard pages (1 run, 2026-10-19)`, Bright
  Data `11/19 hard pages (median of 9 runs, 8–14, 2026-10-15)`.
- On/off switches without deleting a key: `fats setup <free|oxy|bd|decodo|gh> on|off` (`plain` is another name for
  `free`). A switched-off tier's dry and failure row reads `! off oxy · turned off (key kept) → fats setup oxy on`
  (`free fetch turned off` for plain), never an empty `! no_key`, and its status health reads `oxy off (key kept)`
  (`off` without a key). With the free fetch off, `fats setup free` (or `plain`) reads `= setup free · free off
  (turned off) → fats setup free on` instead of `ready`, and the `fats setup` summary starts `= setup free off
  (turned off) · …` with that row first. Turning off the last thing that can fetch is refused (`! refused · nothing left that can
  fetch`).
- `fats setup gh` copies the token of a signed-in `gh` CLI (`gh auth token`) into `auth.json`, else asks for one.
- `fats config set <name> <value>` and `fats config unset <name>`: every threshold in `config` is settable, and so are
  `session`, `cache`, `json`, `nocache`, `offline`, `allow_private`, `chunk`, `account_ttl`, `accept_md` (per-call
  knobs such as `hedge:`, `wait:`, `cost:` stay mods). The value comes from the first of: a mod on the command
  (`batch_parallel:2`), the env (`FATS_BATCH_PARALLEL=3`), `settings.json`, the built-in table; each row names its
  source. `config set` checks a value with the same rule as the reader and refuses it (rc 1) instead of saving one
  that `config` then refuses (`config set chunk 99999` → `! refused · chunk 99999 · want an integer 50-10000`); a
  count threshold has a floor of 1 (`batch_parallel 0` is refused); a value with spaces is kept whole (`config set
  cache <dir>/My Cache`). A bad env value is a `! bad_setting` row and the built-in is used; a bad saved value names
  `settings.json <name>` and `fats config unset <name>`, not an env var. A per-call knob is refused with its mod:
  `config set cost_cap 0.01` → `! refused · cost_cap is per call: the cost: mod`, `config set wait 60` → `! refused ·
  wait is per call: the wait: mod`. A name in `settings.json` or the environment (`FATS_<NAME>`) that no setting reads
  (`render_fix_s`, removed in this release) is a `! unknown_setting <name> · read by nothing` row in `fats config`,
  with its fix (`unset FATS_<NAME>`, `fats config unset <name>`); other answers stay quiet.
- Human screens on a terminal: `fats`, `fats help`, `fats help setup|mcp|search` and a failed page read as short
  sectioned screens (READY? / TRY / MORE). `FATS_HUMAN=1` forces them, `FATS_HUMAN=0` turns them off. Piped
  output and MCP stay the agent lines.
- Home-IP brake on the free fetch: at most one plain request start per host per second across every `fats`
  process (setting `plain_host_pace_ms`, 1000; 0 turns it off). A plain fetch held by it waits for its turn, keys
  or not; the brake never buys a paid call for a free page and is no failure code. The wait is shown apart from the
  fetch time (`plain 3ms $0.0000 · waited 1.4s`). Providers get no pace. A host's first start is paced too: the
  first-seen check runs under the same lock, so 12 processes started together on one new host keep ≥ 1 s apart
  (min gap 0.998 s; before, bursts of 2–7 starts went out ~0.1 s apart). The host's pace file holds the last start
  time, read and written under its lock (never the file's modified time, which a copy or a sync moves); a pace file
  that will not open or lock means no wait.
- No paid call before a verdict on the host's address: the system resolver gets `dns_wait` seconds (a new threshold
  row, 5; it retries by itself, so one lookup; a shorter `timeout:` caps it), and none ends the fetch `! dns_timeout →
  fats u1 fresh:true dns_wait:15` at $0 (before, the providers ran at 1.5 s, and a name that resolves to 127.0.0.1
  billed a render of the provider's own loopback). An error back from the resolver in under a second (`dns_fast_ms`,
  1000) is `! dns_failed` with the resolver's own words (`nodename nor servname provided, or not known`) and `→ check
  the network, then fats u1 fresh:true`, $0. A host whose every address is private is `private_address` before any
  tier. `dry:true` looks nothing up (before, it asked the resolver, so with no network a dry run of a good URL said
  `dns_timeout`): a host without a real TLD, or with a label no resolver can ask for (over 63 characters, empty as in
  `a..com`, a hyphen at either end; RFC 1035), is `no_host` (the real run answers that shape the same at once, $0,
  without asking any resolver), a private IP or `localhost` `private_address`, each with its move instead of a quote;
  `dns_timeout` and a name's private address are the real run's. The `instant` row of `fats config` says `dns
  dns_wait:5s, else dns_timeout $0`.

### Changed
- Keys live in one `auth.json` and switches and set values in one `settings.json`, both in the key dir, mode 0600.
  Keys come from the env first, then `auth.json`. The 0.1 files (`oxylabs.key`, `brightdata.key`,
  `brightdata.zone`) move in once, by themselves, on the first run, and are deleted; nothing to do. Status,
  `config` and `setup` name that file as a key's source (`keys oxy:auth.json bd:auth.json`, `key auth.json
  (OXYLABS_WEB_API_KEY or auth.json)`, `oxy key from auth.json`) instead of `file`.
- A `FATS_CACHE` inside the key dir is refused, as one above it already was.
- `fats setup <word>` takes only a switch (`free`, `plain`, `oxy`, `bd`, `decodo`, `gh`), `on|off` and mods; any
  other word is a `?` repair at $0 (`fats setup oxi` → `fats setup oxy`), never a web search.
- A provider never set up no longer adds a `! no_key` row to every dry run of a user with another paid key (only
  `provider:<it>` names it). A failed fetch names every missing key whose tier could still serve the page, the
  cheapest first (bd, oxy, decodo), and its move is `→ fats setup`, never `nothing left to try`. A dry run's
  `! no_key` row ends `→ fats setup <provider>` (was `→ fats setup · <signup URL>`); the `! no_key` rows of a
  search, of `fats account` and of a failed page lose the sign-up URL too, and a failed page's row ends in a bare
  `→ fats setup`. A dry run lists its `! no_key` rows in the same order, bd → oxy → decodo.
- `fats stats` is a session index: one short row per search and fetch (`u2 rust-lang.org · Rust Programming
  Language · plain 229ms $0.0000 · 445tok`), the last 20 with `… N more · fats stats all`, `limit:N`, a word filter
  over host and title (`fats stats grill`, no longer a web search), `full`/`json` = every row whole as before. A
  failed page's row keeps its `!` code. `fats j<N>` lists its pages in the same row format.
- `fats config`: 73 → 88 rows (`= config · 88 settings`). New rows `plain_host_pace_ms` (1000), `dns_wait` (5) and
  `dns_fast_ms` (1000) for the host check, `reason_chars` (50, the longest provider reason a failed row quotes),
  `thin_chars` (300), `blob_share` (0.9), `row_chars` (90), `decodo`, `decodojs`, and the telemetry rows (`telemetry`,
  `telemetry_debug`, `telemetry_wait_s`, `telemetry_l2_bytes`, `telemetry_sent_keep`, `week_samples`, `crash_chars`);
  `render_fix_s` is gone. `list_cap`, `grep_cap` and `read_cap` are threshold rows now (`read_cap 1000 · build ·
  limits.json`, was `read_cap 1000tok · default`); the `provider` and `device` rows name decodo; the `instant` row no
  longer lists `gh auth token`. The `tables` row reads `tables 8 files · 2092 rows · built-in · providers 49, errors
  53, check 86, sites 69, clean 167, words 1605, limits 52, telemetry 11` (was `7 files · 2012 rows`, providers 43,
  errors 49, sites 42, limits 38); `sites` holds, under `fence`, the special-purpose address blocks the fence refuses
  (IANA registry data), the same list as before.
- `batch_parallel` defaults to 8 pages at once (was 4); its row reads `lanes (RAM guard, not a provider limit)`:
  a 50-page local batch peaked at ~225 MB RSS with 8 lanes (150 MB with 4, 260–310 MB with 16).
- A batch's `cost:` is one pool for all its pages: each paid call takes its price from the pool before it starts
  and gives back what it did not spend (a call cut before its bill counts as spent), so a batch never bills past
  the cap (2 walled pages at `cost:0.002` billed $0.0030 before, $0.0015 now). At launch each page holds one paid
  try (its dearest rung) from the pool, an even share each when the tries sum over the cap, so one page's hedge
  never buys a later page's first try (8 pages that each need a $0.0015 render at `cost:0.012`, the first two slow, default hedge: 8
  ok; before, the hedges on the slow two left 2 pages `budget_hit`). A page gives its share back when it stops (plain served it, or its share buys no rung); of two
  pages on even shares, which one runs on is a race, never input order. A page short only of shares that pages
  already running will give back waits for them (every 250 ms, within its `timeout:`) instead of failing: at
  `cost:0.002`, a page that needs a $0.0015 render gets it once the other page's plain request serves (before:
  `budget_hit`). A share held by a page still queued is never waited for. A run killed before it gave its share back
  holds none. A page the pool left
  without any paid try fails as `budget_hit` (the batch line counts those only, `1 ! budget_hit`; `stats` names
  it); its row, with or without a stored copy, names the cap and every page's part of it, its own too, summing to
  the total (`! budget_hit bd oxyjs not tried · $0.0030 > cost:0.002 (spent $0.0020: u1 $0.0010, u3 $0.0010 held)
  → fats u2 fresh:true cost:0.003 ≤$0.0025`; `held` = reserved, not billed yet, and read at each print, so it
  becomes the billed figure once known) and is the page's only move; once a newer copy of the URL served, the row
  and its `stats` row point there (`→ fats u4`, $0), and with no paid key the move is `→ fats setup`. A
  page that bought a rung keeps the code of what the rung met. `dry:true` on a batch adds `batch up to $0.0020 ·
  cap $0.0020` (the total never above the cap). A price sum rounds up to the next $0.0001 past float noise (3 ×
  $0.0015 prints $0.0045, was $0.0046).

### Removed
- The longer render after a cut: a render timeout with no other keyed provider left offered `· one more try →
  fats uN fresh:true timeout:150`. In 712 saved runs, 0 of 131 answered renders took between 60 and 150 s
  (oxyjs p90 41 s, max 53 s; bd p90 29 s, max 59 s), and the `timeout:150` retries that served did so in under
  60 s. The row now ends there, `failed 1/1 here`, no arrow; the `render_fix_s` setting is gone. Your own
  `timeout:` still sets a render's time.
- `./.env` in the current directory and the Bright Data CLI's credential files are no longer read for keys. Put
  the key in the environment or run `fats setup <provider>`.
- Fetches no longer run `gh auth token` themselves; `fats setup gh` copies the token once.

### Fixed
- Windows: printed commands no longer say `fats.exe` (`fats.exe setup` → `fats setup`); a crash is counted (its
  `src\x.rs` path never matched); a local page's path drops the `\\?\` prefix; a read page keeps its 30 min again;
  a detached fetch keeps its lock (a Windows file lock is never a child process's) instead of reading as
  `! abandoned` at once.
- A search word shaped like a path read a local file instead: `NORTHROP GRUMMAN CORP /DE/ company business` answered
  `/DE/ 0B local`, and a lone `-` among words (`irs liaison committee - tei svc company`) read stdin. Beside other
  words a bare shape of one segment and no extension (`/DE/`) is now a search word unless it exists, and `-` reads
  stdin only as the whole command (`fats -`). A local path never goes to a search provider: a token starting `./`,
  `../`, `~/`, `~name/`, `$VAR/` or `file:`, a dot-led name (`.env`, `.ssh/config`), any token holding a `\`
  (`.\secret\plan`, `\\server\share`), a `/` path of two or more segments (`/etc/hosts`), a bare path of three or more
  (`internal/hr/salaries`; `TCP/IP` and `and/or` stay words), a drive path (`C:\x`, `C:/x`), or a name with a document
  or key extension (`.pdf .docx .html .txt .md .csv .json .xml .zip .xlsx .pptx .key .pem .crt .p12 .pfx`: `acme
  secret.txt`) stays a file wherever it stands, missing or not (`! no_file`). A bare name that is a file in the
  working directory (`README`) is that file; a key or credential file (`.env`, `.env.local`, `id_rsa`,
  `authorized_keys`, `*.pem`, `*.key`, …) is refused by name, never read, stored or searched: `! secret_file id_rsa ·
  a key or credential file is not read · $0` (exit 1; before, `fats .env` copied the file into the cache and a missing
  `id_rsa` went to a paid web search), a missing one alone `! no_file`; a directory (`/etc`) asks: `? /etc is a
  directory` (exit 2). A missing name whose extension is also a real domain ending (`notes.md`, and a lone `main.rs`)
  asks instead of guessing: `? notes.md: a missing file or a site · type ./notes.md or https://notes.md` (exit 2, $0).
  Under `fats mcp` that check never looks outside the working directory and the cache: a path out there is refused
  alone and beside words alike, whether it exists or not (before, the answer told a model whether `/x/y` existed on
  the host). `dry:true` gives the run's own answer instead of planning a read (it printed `./notes.md 0B local → … →
  new u1` for a missing file): `! no_file`, `! not_allowed`, `! secret_file`, and `? u99 unknown` (exit 2) for an id
  the session does not hold.
- A page asked by a short numeric id (`/eli/reg/2021/821`) that the site redirected to a front page
  (`/TodayOJ/index.html`) was served with no `! redirected` flag. A path whose numbers are all gone from the landing
  now says so; a short alias such as `/releases/latest/` landing on its post stays unflagged. Before, a served page
  whose short last path part was gone from where it landed (`/a`, `/food/`, `@Brian`, a bare `?q=` search) was flagged
  `! redirected`, while the fetch itself judged a landing only by an asked id of 8 or more letters or digits. One rule
  now judges both: a shorter id names no page and carries no flag.
- An arrow pasted whole ran its decorations as words: `fats u1 js:true ≤$0.0030 ~25s` made `≤$0.0030 ~25s` filter
  words, and `fats u1#3 · 1.5k tok · $0` filtered for `· 1.5k tok · $0`. A command that starts with a page id, a job
  id, a URL or `setup`/`stats`/`config` now drops what fats prints after an arrow's command: a `(…)` group, `≤$X`,
  `~25s`, and a `·` with the words after it; mods typed after them still count. A search keeps its parentheses (`fats
  rust (programming language)`). Pasted unquoted into bash or zsh, an arrow also broke the shell itself (zsh expands
  `~25s`): a printed arrow now has no `(…)` group and puts its notes after a `·` (`→ fats u1 js:true · oxyjs ≤$0.0015
  ≈25s`, `→ fats u3 wait:30 · free, check again in 30 s`, `→ fats u2 · rendered`), times read `≈25s`, and a price a
  shell expanded (`≤zsh.0015`) is dropped too.
- An island that is one encoded blob (a data script holding a 157 KB base64 string) was listed with an arrow,
  `→ fats u1#1`, and reading it printed 1,000 tokens of base64 per part with 80k more to go. When an island's
  first part is at least 90 % base64 characters with no space, its row now prints `(encoded)` and its size, and
  no arrow. The 90 % is a new `fats config` row, `blob_share`.
- Under `fats mcp`, a local file was checked against the working-directory fence with its symlinks resolved, then
  opened by its name again, so a link switched between the two could still read a file outside. The read now
  opens the resolved path the check judged.
- Reading a result id right after a search answered `pending 0s` while its page landed a fraction of a second
  later, so every search-then-read cost one extra call. A read of an id still fetching now waits like a fetch
  does (the default `wait:`, 30 s) and answers the page the moment it lands; `wait:0` keeps the instant
  `pending`. A search's job id (`fats j3`) waits the same way. `fresh:true` on an id still fetching answers that
  fetch's page, never a second call.
- A page whose only move is a meta refresh to its own site with no delay (a "latest release" link that
  forwards to the release's page) bought a paid render, $0.0015 and 10 s, and was then flagged `redirected`.
  The free fetch now follows that refresh once, as a browser does, and serves the page it lands on at $0. That
  page is judged as any landing: `! redirected` says where it went only when an asked id of 8 or more letters
  or digits is gone (`/releases/latest/` names no page: no flag); the refreshing page is kept in the saved record as `refreshed_from` and shown in the outline as
  `uN#0 info | refreshed from: <url>`. A refresh with a delay
  (digits only, `0`), an empty `url=`, to another origin (scheme, host and port; http to https is the one scheme
  move), beside a script, or on a page with text of its own is not followed. A target that fails or answers no
  2xx leaves the refreshing page as the page.
- A page's `js:true` arrow gave a price but not the provider, so a served card offered `→ fats u1 js:true
  ≤$0.0015` to the very provider that had just refused the URL (`400 url: value not supported`, 158 ms). The arrow
  now names the providers its `dry:true` would call, `→ fats u1 js:true · oxyjs ≤$0.0015 ≈25s` (`bd, then oxyjs`
  with both keys), and is dropped when one of them refused the URL in this session.
- A provider's error said only `provider_error`: the reason sat in the saved error file. The `!` row and
  `fats stats full` now carry the provider's own short reason, e.g. `oxyjs 400 url: value not supported` (status
  plus the field it named, at most 50 characters, from the body's fields only, each masked once decoded so a key
  spelled in JSON escapes is masked too; the provider's answer kept for `fats stats` is masked the same way, and
  a key echoed in upper case is masked too). A render the provider
  refused (a 4xx) or gave up on after its own retries (`faulted`) is never asked again for that URL in the
  session, by name or by an arrow that would route to it (`fresh:true`): the arrow names another keyed provider,
  else the row ends `failed 1/1 here`. Before, it offered `provider:oxy js:true` again, which failed the same way.
  A 4xx on one field (`400 geo_location`) holds only for an arrow whose request sends that provider the same
  country; one on the URL, a 404 or a give-up holds for every copy. The same check guards every printed paid
  arrow: the `js:true` hint, a language fix and a spending-cap row lose an arrow that would ask a refused provider.
  A second run that ends on the same provider with the same code counts (`failed 2/2 here`) whatever ran before
  it; the count drops the arrow only when that arrow is the same call again, so a provider never asked stays the
  move beside it (`failed 2/2 here → fats u2 fresh:true provider:bd`). Bright Data's rate limit is a wait, not its answer: the arrow asks Bright
  Data again (`fresh:true provider:bd`), never the provider that just gave up.
- A dead subdomain whose DNS zone answers "no such record" rather than "no such name" (NODATA, common in signed zones)
  read as a host that exists: the paid ladder ran (up to 48 s and ≤$0.0010 per try) and its arrow ran it again. A name
  with no address from the system resolver and none from the second resolver (no A and no AAAA record, or no such
  name) is now `! no_host`, $0, at once, with no provider call.
- A whole page's `Ntok` figure counted its own `=` line with that read's timing (`local 1ms`, `cache 2h`): one
  page printed 716 or 717 tok by the ms digits, and its cache read another size. It still counts the `=` line (it
  is in the agent's context), with the timing at a fixed width, so every read of a copy prints one size, and
  `fats stats` prints that same figure (it printed the page's size: 52 where the read said 93).
- A word with a real domain ending in the middle of a search (`Purchasing.NYC Lakewood …`, `contact at
  acmebuilders.com phone`, `rust x.ai`), or capitalised at its start (`Booking.com hotels Lisbon`), turned the
  whole search into a one-site `site:` search, often 0 results at full price, and the `run it:` line rewrote the
  input as a URL. It is now a search word: a bare name is a host only as the whole command or leading it in
  lower case. A bare host still names a site where an address is typed: alone (`fats example.com`,
  `fats Booking.com`), beside other hosts only (`fats example.com example.org` reads both pages), or leading the
  words in lower case (`fats docs.rs tokio spawn` = `site:docs.rs`; alone, a name ending `.rs .py .sh .md .zip .mov
  .java .tf .pl .ml` is a file when it exists and asks when not, `fats https://docs.rs` reads the site); a scheme, a `/` path or `site:` makes a host
  anywhere. A name with a document extension is a file first (`notes.md`: read when it exists, asked about when not).
- A search engine's operator (`filetype:pdf`, `before:2026-01`, `intitle:fats`; also `inurl intext after
  allintitle allinurl`) answered `? filetype: unknown` with rc 2. It now goes to the search as typed. Another
  unknown key still asks (`countr:lt` → `? countr: unknown`).
- A short page with one script and a real paragraph is the page; before, it bought renders. When the page's
  title words are not on screen (a one-word title, a forum's "View topic" label), a copy showing one text of 12+
  words outside nav, header, footer, aside and noscript that asks nothing of JavaScript, in 300+ visible chars,
  is served instead of judged `js_shell` (a short forum thread, a 12.7k-char forum topic). A link-only page with
  no such text still is. The 300 is a new `fats config` row, `thin_chars` (it was a fixed number).
- `fats uN comments` on a page whose few comments fall under the thread bar (three on a short forum item) said
  `no comment thread served` while the comments were on screen. It now says `no thread found · N sections: fats uN`
  (the comments are in the page's text), and a comment with no replies says `no replies · list: fats uN comments`.
- An https page whose server refused the connection (or failed the TLS handshake) went on through every paid provider,
  all failing at the same server (92 s on one forum), and then offered `fresh:true` to run them again. The free fetch
  now asks the same URL over http at once, in the same call: a page served there is the page, $0, its `=` line saying
  `landed http://<host>`; when http fails too, the run goes on as before. The saved record keeps the https cause
  first, then what http answered (`tls:wrong-version-number+http:403`).
- An https page whose certificate failed the check (expired, made out to another host, or from an untrusted issuer)
  went on to the paid providers (an expired certificate cost $0.0025, for a page no provider can make valid). When
  http only sends it back to https (a 301) or fails too, the run now ends at the free fetch, $0: `tried
  plain:tls_failed✗`, and the row `u1 ! tls_failed certificate-verify-failed · $0 → check the certificate · fats u1
  fresh:true provider:oxy ≤$0.0025` (the arrow names a keyed provider that fetches without a browser; none keyed, no
  arrow); `fats stats full` shows the same. A page asked with `provider:` or `js:true` still goes to that provider. A
  handshake that failed for another reason (a wrong protocol version) still walks the providers.
- `fats u1 js:true` on a page whose render was already served in this session (asked earlier, or reached through an
  arrow) paid for a second render with the same text. It now answers the stored render, `cache`, $0, from any
  provider; `dry:true` says `= u2 (rendered, cache) · $0`; `fresh:true` asks for a new one.
- README names the MCP Registry entry again (`mcp-name: io.github.fatsearch/fats`); the registry proves crate
  ownership by that line in the crates.io README, and the 0.1.3 README rewrite had dropped it, so the registry
  entry stayed at 0.1.2. `check.sh` now fails without it.

## 0.1.3 - 2026-10-19

### Added
- `skills/fatsearch/SKILL.md`: the usage skill for agents (Agent Skills standard; `npx skills add fatsearch/fatsearch`,
  `gh skill install fatsearch/fatsearch`): the rules learned from agents misreading pages, the loop, the `=` line.
- `AGENTS.md` for coding agents changing this repository: build, gates, the rules that reject a change, the file map.

### Fixed
- A file name among the search words (`opencode.json`, `next.config.mjs`, `cargo build main.rs`) stays a search
  word; it no longer turns the search into a paid `site:` search on a host that does not exist. A bare name is a
  site only when it ends in a real top-level domain; `fats docs.rs tokio spawn` still searches docs.rs.

### Changed
- A search ending in the word `json` (`fats vite config json`, `fats example.com rust json`) now searches for it
  instead of printing JSON; beside a page id or file (`fats u1 price json`) it is still the JSON switch. Scripts
  that wrote `fats <words> json` write `fats <words> json:true`.
- README rewritten: the banner, the race picture (one page, six readers, real token counts), the cost table, the
  three-step start, "For agents". The measurement is recorded in `docs/benchmarks.md` ("One page, six readers").
- README and installation: every channel is live (Homebrew, npm/npx, crates.io, cargo-binstall, MCP Registry);
  the "not published yet" marks are gone; version badges read from the registries.

## 0.1.2 - 2026-10-17

### Added
- npm package `fatsearch` (`npm i -g fatsearch`, `npx fatsearch`), generated by cargo-dist: it installs `fats`
  by downloading the release archive for the machine from GitHub at install (or at first run when install
  scripts are off). Every tagged release publishes it.
- MCP Registry entry `io.github.fatsearch/fats` (`server.json`: the crates.io and npm packages, run as
  `fats mcp`), listed after every release by a GitHub OIDC workflow. `check.sh` fails when its version is not
  the crate's.

### Changed
- The installers require glibc 2.35+ on Linux, the version the Linux builds link against (was 2.31, which
  let them install a binary that cannot start on 2.31–2.34).

### Fixed
- The key dir is made owner-only (0700) whenever `fats` saves a key into it, also when it existed already: on
  Linux the installer's receipt can create `~/.config/fatsearch` at 0755 before `fats` first runs.

## 0.1.1 - 2026-10-17

### Added
- Prebuilt binaries and installers via cargo-dist: every tagged release builds `fats` for macOS (Apple silicon,
  Intel), Linux (x86_64, ARM64) and Windows (x86_64) and attaches the archives, `.sha256` checksums, GitHub build
  attestations and shell / PowerShell installers. A Homebrew formula (`fatsearch/tap/fatsearch`, installs `fats`)
  is generated too.

### Changed
- The crate's `homepage` is the GitHub repository (was fatsearch.com, which has no site yet).

## 0.1.0 - 2026-10-17

First public release.

### Added
- `fats <url>`: fetch, clean, outline. One `=` line (title, place, counts, tokens, how it was fetched, what it
  cost), then numbered sections (`u1#3`) the agent reads on demand. Every page is cached; later reads are free.
- Fetch ladder: plain request first, then Oxylabs (no JS, then JS) and Bright Data. Any one provider key works;
  with both, the cheaper route goes first. `wait:N`, `timeout:N`, `fresh:true`, `country:xx`, `device:mobile`, `json`.
- Readers: HTML and PDF (one section per page). Pictures and videos are counted on the `=` line; pixels on request.
  Office files are stored, not read yet.
- Honesty: a `!` line names what is missing and the next free move; a render that failed is not offered again;
  the price glyph `≤$X` is the worst case; nothing is dropped silently.
- `fats mcp`: stdio MCP server, one tool `fats(cmd)`, ~60-token card. `fats setup` finds, checks and saves keys
  (mode 600). `fats stats`, `fats config`, `fats guide`, `--dry`.
- One static binary, ~17 MB, ~4 ms start. No daemon, no browser, no telemetry.
- Command aliases, all free: `login`, `keys`, `auth` run `fats setup`; `?`, `commands` print the guide;
  `balance`, `usage`, `spend`, `credits` run `fats account`. Words of other tools (`whoami`, `logout`, `install`,
  `upgrade`, …) get a `!` line naming the right command, rc 2.
- Discussion threads: `fats uN comments` lists the top-level comments, most replies first, one row each
  (`uN#k · replies N · <text>`). `fats uN#k` opens one comment with its direct replies; `uN#k full` reads its
  whole tree. The outline footer says how many comments this copy served; `N of M comments served` appears only
  when the page itself states its total, never as a guess. `--help` stays the same card, under 500 bytes.
- `fats gh <login>` lists that account's repositories by stars, not only repos that mention the name.
- `fats stats` adds one spend line over all sessions (last 24 h and all time, unverified cuts apart). The ledger
  sits beside the keys, holds a time and a price per paid record, never a URL or a query, and is never swept.

### Changed
- Default `wait:` is 30 s (was 5 s): most walled renders finish inside it. A run still going reads `pending`
  with its free next move (`→ fats uN wait:30`).
- `timeout:N` is one wall for every call of a command, the plain request included.
- A batch of URLs runs at most 4 at once; ids are given in input order, `dry:true` prints the same ids, and a URL
  given twice is fetched once (its second row says `same as above`).
- The cache lives in the OS cache dir (`~/Library/Caches/fatsearch` on macOS, `~/.cache/fatsearch` on Linux,
  the local app-data dir on Windows; `FATS_CACHE` overrides it, but not with an empty, relative, `/` or home path).
  A page is kept 30 min after its last read and at most 60 min after its fetch; a swept id says so and names the
  refetch. Only a dir fats made itself is ever swept.
- `limit:N` sets both the results a search fetches and the rows shown, at most 20 rows within the read cap; a
  larger N says it was capped.

### Fixed
- An unknown flag (`--login`, `-x`, `rust --fast`) is refused with a `!` line and never becomes a paid web search.
  Beside an id or a file a dash word is a free filter (`fats u1 -n`). A dash inside a phrase is kept as typed.
- A run whose process died reads `! abandoned` in `fats jN`, in its page read and in `stats`, with what it spent
  and a priced next move.
- A page that moves to a sign-in page is judged by whole words of its new address (`login`, `sign_in`, `oauth`,
  `sso`, …), so `/authors` or `/docs/authentication` is not a wall; a real sign-in wall is rendered once, not twice.
