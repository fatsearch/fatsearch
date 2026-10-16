# Installing fats

`fats` is one binary: a CLI and, as `fats mcp`, a local stdio MCP server with one tool, `fats(cmd)`.
This guide builds it, adds a provider key, connects it to an agent host, and checks every step.

**Agents:** follow the [checklist](#agent-checklist) at the end; each step has a check command.
Never print, echo or paste a key value; key names and `fats` status lines are enough.
Lines marked ❓ are not verified yet.

| Thing | Where |
|---|---|
| Binary | `~/.cargo/bin/fats` (from `cargo install`) |
| Keys | macOS `~/Library/Application Support/fatsearch/` · Linux `${XDG_CONFIG_HOME:-~/.config}/fatsearch/` · Windows `%APPDATA%\fatsearch\` · or `FATS_KEY_DIR` |
| Cache (pages live ≤ 1 h) | macOS `~/Library/Caches/fatsearch/` · Linux `${XDG_CACHE_HOME:-~/.cache}/fatsearch/` · Windows `%LOCALAPPDATA%\fatsearch\` · or `FATS_CACHE` (a relative dir is resolved once against the working dir; `~/…`, `/` and `$HOME` are refused; swept only if `fats` created it) |
| Spend ledger | `spend.log` in the key dir (time, cost and cache dir per paid record; `fats stats` sums it) |
| Exact paths on this machine | `fats config` (rows `cache` and `key_dir`, each with its source) |

## 1. Prerequisites

There are no prebuilt binaries and no crates.io package yet (both are coming), so `fats` builds from source.
The build needs:

- **Rust 1.99** (pinned in `rust-toolchain.toml`), via [rustup](https://rustup.rs).
- **CMake 3.22+**, a **C and C++ compiler**, and **libclang**: the TLS stack compiles BoringSSL
  from source and generates its bindings with bindgen, which loads libclang.

macOS (tested, Apple silicon): the Command Line Tools bring the compiler and libclang.

```sh
xcode-select --install
brew install cmake
```

Ubuntu / Debian (CI builds on Ubuntu; this minimal package set is ❓ untested on a clean machine):

```sh
sudo apt-get install -y build-essential cmake libclang-dev git
```

Windows ❓ untested. What the build scripts ask for: Visual Studio Build Tools with the C++ workload,
[CMake](https://cmake.org/download/) 3.22+, [NASM](https://www.nasm.us/) (BoringSSL's x86_64 assembly on Windows),
and LLVM for libclang (`winget install LLVM.LLVM`, then set `LIBCLANG_PATH` to LLVM's `bin` folder).
<!-- source: https://rust-lang.github.io/rust-bindgen/requirements.html (2026-10-16); btls-sys 0.5.6 build/main.rs + vendored boringssl CMakeLists.txt (cmake_minimum_required 3.22, ASM_NASM on WIN32 x86/x86_64) -->

## 2. Build and install

```sh
git clone https://github.com/fatsearch/fatsearch
cd fatsearch
rustup toolchain install 1.99.0
cargo install --locked --path .
```

This compiles BoringSSL once (a few minutes) and installs `fats` (the package is named `fatsearch`).
If `fats` is not found afterwards, add `~/.cargo/bin` to `PATH` (`. "$HOME/.cargo/env"`).
Check: `fats --version` prints `= fats 0.1.0 · status: fats · guide: fats guide`; `fats guide` = the cheat sheet.

## 3. Keys (optional)

Without a key, `fats` reads what a plain HTTP request gets, for $0.
With **any one** provider key it also renders JS pages and gets past walls through that provider;
with both, it routes between them. Each key goes only to its own provider; there is no telemetry.

| Provider | Sign up | Key | Variables |
|---|---|---|---|
| Oxylabs | https://dashboard.oxylabs.io/en/registration | a Web API instance's key in the dashboard | `OXYLABS_WEB_API_KEY` |
| Bright Data | https://brightdata.com/cp/start | https://brightdata.com/cp/setting/users | `BRIGHTDATA_API_KEY` + `BRIGHTDATA_UNLOCKER_ZONE` (a Web Unlocker zone) |

Put the key in the environment (or in `./.env` as `NAME=value`), then run `fats setup`
(`fats setup dry:true` first shows where each key was found, with no call and no write). It:

- Looks for each key in this order: environment, `./.env` in the current directory, the Bright Data CLI's
  own login files (read only), the key dir.
- Checks it for $0: Oxylabs lists its scrapers; Bright Data lists zone names and types.
- Saves it to the key dir as `oxylabs.key` / `brightdata.key` (mode 600, dir 700), and the Bright Data zone
  as `brightdata.zone`: `zone:<name>` if given, else the configured one, else the account's only Web Unlocker zone.
- Never prints a key.

Fetches read a key from the environment first, then the key dir. A key only in `./.env` works after
`fats setup` has saved it. Saved keys are the simplest way to give keys to every MCP host:
hosts differ in which environment variables they pass to a server.

## 4. Verify

```console
$ fats https://example.com
= j1.u1 Example Domain · example.com · 1 sections · 0 photos · 142tok · plain 53ms spent $0.0000
[u1#1] # part 1
This domain is for use in documentation examples without needing permission. …
(whole page shown) · filter: fats u1 <words> · links: fats u1 links
```

The first line starts with `=`; ids, timing and token counts vary. `fats` alone prints the status:
session, cache path, keys, provider health, and `mcp: ready · fats mcp = stdio server, one tool fats(cmd)`.
Its keys row reads `keys none → fats setup · network on` before setup and like
`keys oxy:file bd:file · bd zone <zone> · network on` after (`env`, `file` = key dir, `none`).

Check the MCP server without any host (prints two JSON lines: `initialize` and `tools/list`):

```sh
printf '%s\n' \
 '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"check","version":"0"}}}' \
 '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
 '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | fats mcp
```

## 5. Connect an agent host

All hosts below start `fats mcp` as a local process. Notes that apply to every one:

- **GUI apps do not read the shell's `PATH`.** If a host reports it cannot start `fats`, use the absolute path
  from `command -v fats` (Windows: `where fats`) as `command`. The examples use `fats` for brevity.
- **Keys:** run `fats setup` once (section 3) and leave keys out of host configs. To pass them through a host
  instead, use its `env` field (shown below where the host has one); prefer a `${VAR}` reference to a value.
- **Local files:** over MCP, `fats` reads files only under the server's working directory and its cache.
- A slow page answers `pending` after 5 s (`wait:`), so tool calls stay short; the reply names the next command.

### Claude Code

```sh
claude mcp add --scope user fats -- fats mcp      # all your projects
claude mcp add fats -- fats mcp                   # default scope local: this project, only you
claude mcp add --scope project fats -- fats mcp   # writes .mcp.json in the project root, for the team
```

Scopes: `local` and `user` live in `~/.claude.json`; `project` lives in `.mcp.json`. Everything after `--`
is the server command. To pass a key, put `--env` before the name with another option between them:
`claude mcp add --transport stdio --env OXYLABS_WEB_API_KEY="$OXYLABS_WEB_API_KEY" fats -- fats mcp`.

`.mcp.json` (an entry without `type` is stdio; `${VAR}` expands from the environment):

```json
{ "mcpServers": { "fats": { "command": "fats", "args": ["mcp"],
  "env": { "OXYLABS_WEB_API_KEY": "${OXYLABS_WEB_API_KEY}" } } } }
```

Check: `claude mcp get fats` or `claude mcp list` (`✔ Connected`); inside a session, `/mcp`.
A project `.mcp.json` server waits for approval the first time `claude` runs in that folder.
<!-- source: https://code.claude.com/docs/en/mcp (2026-10-16) -->

### Claude Desktop

Settings → Developer → Edit Config opens `claude_desktop_config.json`:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`
- Linux ❓: not in the official docs; community builds use `~/.config/Claude/claude_desktop_config.json`.
  Use Edit Config to find the real file.

```json
{ "mcpServers": { "fats": { "command": "/home/you/.cargo/bin/fats", "args": ["mcp"] } } }
```

Use the absolute path (`command -v fats`), then quit Claude Desktop completely and start it again.
Server logs: `~/Library/Logs/Claude/mcp-server-fats.log` (macOS), `%APPDATA%\Claude\logs` (Windows).
One-click `.mcpb` bundles exist as a format; `fats` does not ship one yet.
<!-- source: https://modelcontextprotocol.io/docs/develop/connect-local-servers (2026-10-16); https://github.com/modelcontextprotocol/mcpb (2026-10-16) -->

### Codex CLI

`codex mcp add fats -- fats mcp`, or edit `~/.codex/config.toml` (or `.codex/config.toml` in a trusted project):

```toml
[mcp_servers.fats]
command = "fats"
args = ["mcp"]
# optional: forward keys from Codex's environment
env_vars = ["OXYLABS_WEB_API_KEY", "BRIGHTDATA_API_KEY", "BRIGHTDATA_UNLOCKER_ZONE"]
```

Check: `codex mcp list`, or `/mcp` in the `codex` TUI. Default `tool_timeout_sec` is 60, well above `fats`' 5 s `wait:`.
<!-- source: https://developers.openai.com/codex/mcp (2026-10-16) -->

### Cursor

`~/.cursor/mcp.json` (every project) or `.cursor/mcp.json` (this project):

```json
{ "mcpServers": { "fats": { "command": "fats", "args": ["mcp"],
  "env": { "OXYLABS_WEB_API_KEY": "${env:OXYLABS_WEB_API_KEY}" } } } }
```

Stdio servers also take `"envFile": "<path>"`. Check the server in Cursor's MCP settings list.
<!-- source: https://cursor.com/docs/context/mcp (2026-10-16) -->

### VS Code (Copilot)

`.vscode/mcp.json` in the workspace (note the top-level key `servers`), or run **MCP: Open User Configuration**:

```json
{ "servers": { "fats": { "type": "stdio", "command": "fats", "args": ["mcp"] } } }
```

From a shell: `code --add-mcp '{"name":"fats","command":"fats","args":["mcp"]}'`.
Check: **MCP: List Servers** → fats → Show Output.
<!-- source: https://code.visualstudio.com/docs/copilot/customization/mcp-servers (2026-10-16) -->

### Windsurf

Cascade panel → `...` → Open MCP config file. The docs place it at `~/.config/devin/mcp_config.json`
(macOS, Linux; `$XDG_CONFIG_HOME/devin/` if set) and `%APPDATA%\devin\mcp_config.json` (Windows).

```json
{ "mcpServers": { "fats": { "command": "fats", "args": ["mcp"] } } }
```
<!-- source: https://docs.windsurf.com/windsurf/cascade/mcp (2026-10-16) -->

### Zed

Settings → AI → MCP Servers → Add Server → Add Local Server, or edit the settings file (`zed: open settings file`):

```json
{ "context_servers": { "fats": { "command": "fats", "args": ["mcp"], "env": {} } } }
```
<!-- source: https://zed.dev/docs/ai/mcp (2026-10-16) -->

### Gemini CLI

`gemini mcp add --scope user fats fats mcp` writes `~/.gemini/settings.json` (default scope `project`:
`.gemini/settings.json`) with `{ "mcpServers": { "fats": { "command": "fats", "args": ["mcp"] } } }`.
Gemini CLI strips inherited variables whose names contain `KEY`, `TOKEN` and similar, so
`OXYLABS_WEB_API_KEY` does not reach the server unless set in `env` (`"OXYLABS_WEB_API_KEY": "$OXYLABS_WEB_API_KEY"`).
Saved keys (`fats setup`) avoid this. Check: `/mcp`.
<!-- source: https://geminicli.com/docs/tools/mcp-server/ (2026-10-16) -->

### ChatGPT and other OpenAI apps: not supported yet

ChatGPT connects to remote MCP servers over a public HTTPS (streamable HTTP) endpoint. `fats mcp` is
stdio only: no HTTP, no OAuth, no remote server. OpenAI's Secure MCP Tunnel can forward to a local stdio
server for workspaces that have custom MCP access; ❓ untested with `fats`. Codex CLI (above) works today.
<!-- source: https://help.openai.com/en/articles/12584461-developer-mode-apps-and-full-mcp-connectors-in-chatgpt-beta (2026-10-16); https://developers.openai.com/plugins/deploy/connect-chatgpt (2026-10-16); https://developers.openai.com/api/docs/guides/secure-mcp-tunnels (2026-10-16) -->

## 6. Plain shell use (any agent)

An agent that can run shell commands needs no MCP at all:

```sh
fats https://example.com     # outline with ids, cached as u1
fats u1#3                    # read section 3
fats u1 price                # lines that match, inside the page
fats u1 json                 # the same answer as one JSON record (or FATS_JSON=1)
```

Every command: [README, "All commands"](../README.md#all-commands). Full reference: [reference.md](reference.md).

## 7. Uninstall

```sh
fats config | grep -E '^(cache|key_dir) '   # note both paths first
cargo uninstall fatsearch                   # removes ~/.cargo/bin/fats
```

Then delete the two folders it printed (defaults in the table at the top). The key dir holds `oxylabs.key`,
`brightdata.key`, `brightdata.zone` and the spend ledger `spend.log`. In the cache dir, pages and searches go
within the hour; the session id lists (an id's URL cleared once its page is gone and its job an hour old),
`index.json` (rows of live pages only), `jobs.json` (commands cleared after an hour) and `account.json` stay
until you delete the dir.
Remove the host entry too: `claude mcp remove fats` (add `--scope user|project` to match), or delete the
`fats` entry from the host's config file. `fats` never writes to the Bright Data CLI's files.

## 8. Troubleshooting

| You see | Cause | Fix |
|---|---|---|
| ``is `cmake` not installed?`` during the build | CMake missing | install CMake 3.22+ (section 1) |
| `couldn't find any valid shared libraries matching: [...libclang...]` | libclang missing | install it (section 1) or set `LIBCLANG_PATH` to its folder |
| a CMake error about `ASM_NASM` (Windows) ❓ | NASM missing | install NASM, put it on `PATH` |
| `fats: command not found` | `~/.cargo/bin` not on `PATH` | `. "$HOME/.cargo/env"`, or use the absolute path |
| host: server failed to start / `ENOENT` | GUI app without your shell `PATH` | absolute path from `command -v fats` |
| `keys none → fats setup` (status), or `! no_key oxy · would try … · set OXYLABS_WEB_API_KEY → fats setup · oxy <sign-up page>` | no key; the page needs a provider | either provider's key (section 3); plain pages still work |
| `! bad_key replace OXYLABS_WEB_API_KEY → fats setup` | key rejected | new key from the dashboard, `fats setup` again |
| `! no_zone … unblocker zones: a, b → fats setup zone:a` or `! bad_zone bd <zone> → fats setup` | no zone picked, or an unknown one | `fats setup zone:<name>`; no zone yet: create a Web Unlocker zone in the Bright Data panel |
| `! key_mode oxylabs.key is 644, readable by others → chmod 600 <path>` | key file too open | run the printed `chmod` |
| `! offline` | `FATS_OFFLINE` is set | unset it |
| `! not_allowed <file>` (over MCP) | file outside the server's working directory | save it under that directory |
| `! bad_setting FATS_… · want … · using …` | an env value `fats` refused | fix or unset that variable |

Every `!` row ends with `→` and the command that fixes it; `fats config` shows each setting and its source.

## Agent checklist

Run in order. Stop at the first failed check and report its output. Never print key values.

1. Tools present. Check: `cmake --version` (3.22+), `cc --version`, `rustup --version`, `git --version`.
2. Clone: `git clone https://github.com/fatsearch/fatsearch && cd fatsearch`. Check: `test -f rust-toolchain.toml`.
3. Toolchain: `rustup toolchain install 1.99.0`. Check: `rustc --version` in the clone prints `rustc 1.99.0`.
4. Install: `cargo install --locked --path .`
   Check: `command -v fats` prints a path; `fats --version` starts with `= fats `.
5. Plain fetch: `fats https://example.com`
   Check: the first line starts with `= ` and contains `Example Domain`.
6. Keys, only if a key is already in the environment or `./.env`: `fats setup`
   Check: `fats | grep '^keys'` shows `oxy:file` or `bd:file`. No key: skip; report that plain pages work.
7. MCP server: run the `printf … | fats mcp` block from section 4.
   Check: the output contains `"name":"fats"` in the `tools/list` reply.
8. Register with the host in use (section 5), using the absolute path.
   Claude Code: `claude mcp add --scope user fats -- "$(command -v fats)" mcp`; check `claude mcp get fats`.
   Codex: `codex mcp add fats -- "$(command -v fats)" mcp`; check `codex mcp list`.
   Gemini CLI: `gemini mcp add --scope user fats "$(command -v fats)" mcp`; check `/mcp`.
   File-based hosts: write the JSON block with `command` = the output of `command -v fats`, then ask the
   user to restart the app.
9. Final status: `fats`
   Check: a `keys` row and `mcp: ready · fats mcp = stdio server, one tool fats(cmd)`.
