//! MCP surface: `fats mcp` = a stdio server with ONE tool, `fats(cmd)`. Its text is exactly what the CLI
//! prints for the same command line (same `cli::run`, same cache, same session), so there is one language
//! on every surface.
//!
//! What differs from the shell: local files are read only from the working directory and the cache (the
//! model, not the user, picks the path), and `-` (stdin) is refused because stdin carries the protocol.
//! Failures are `!` rows in the text; protocol errors only for requests that are not a `fats` call.

use crate::lang::{CARD, Out, WORDS, bin, local_path};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, ProgressToken, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use std::path::PathBuf;

/// The tool description: one line + two examples (~60 tokens) for clients that drop `instructions`.
pub const DESCRIPTION: &str = "Web pages, PDFs and local documents as small cached Markdown with ids. \
cmd = one fats command line, e.g. `https://example.com/menu.pdf` (fetch → outline with ids) or \
`u1#3` / `u1 tuna price` (read section 3 / the lines with those words). First time? Call fats(\"guide\").";

/// Sent once at `initialize`: the card plus how to read the replies.
pub fn instructions() -> String {
    format!(
        "fats turns web pages, PDFs and documents into small, clean, cached Markdown. The one tool `fats` takes \
         the same command line as the `fats` CLI and returns the same text.\n\n{CARD}\n\n\
         Every reply starts with a `=` line (what happened, which id); ids it prints (u1, u1#3, l4) are what the \
         next cmd takes. A `!` row names what failed and, after →, the command that fixes it. Pages stay cached \
         in this session: re-reading u1#3 fetches nothing. Local files: the working directory and the cache only. \
         An empty cmd prints the status. Anonymous usage stats (no URL, search or key): \
         `fats telemetry show` = what is sent · off: `fats setup telemetry off`.\n\nFirst time? Call fats(\"guide\")"
    )
}

fn tool() -> Tool {
    let cmd = serde_json::json!({"type": "string", "description": "e.g. u1#3"});
    let schema = serde_json::json!({"type": "object", "properties": {"cmd": cmd}, "required": ["cmd"]});
    let schema = schema.as_object().cloned().unwrap_or_default();
    // not read-only (MCP ToolAnnotations): a call may spend provider credit and adds to the cache; additive only
    let hints = ToolAnnotations::new().read_only(false).destructive(false).open_world(true);
    Tool::new("fats", DESCRIPTION, schema).with_annotations(hints)
}

struct Fats;

impl ServerHandler for Fats {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fats", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions())
    }

    async fn list_tools(
        &self,
        _req: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![tool()]))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if req.name != "fats" {
            return Err(ErrorData::invalid_params(format!("unknown tool {} · the one tool is fats", req.name), None));
        }
        let Some(cmd) = req.arguments.as_ref().and_then(|a| a.get("cmd")).and_then(|v| v.as_str()) else {
            return Err(ErrorData::invalid_params("arguments.cmd: want a string, e.g. u1#3", None));
        };
        let (cmd, asked) = (vec![cmd.to_string()], vec![cmd.to_string()]);
        // `cli::run` blocks (and runs its own fetch runtime), so it gets a blocking thread
        let work = tokio::task::spawn_blocking(move || crate::cli::run(&cmd));
        let (text, rc, image) = ticking(work, ctx.meta.get_progress_token(), &ctx.peer)
            .await
            .map_err(|e| ErrorData::internal_error(format!("fats crashed: {e}"), None))?;
        // the cache sweep, the notice (stderr, never in the answer), the update check: off the answer's path
        tokio::task::spawn_blocking(move || crate::telemetry::tick(&asked));
        // pixels only when the command asked (`u1 m1`), after the line that says their cost; never
        // `structuredContent` beside them (Claude Code drops image blocks when it is set, issue 54737); a saved
        // file Claude cannot read as pixels (avif, heic, …, or one that does not decode) is said, never dropped
        let mut blocks = vec![ContentBlock::text(text)];
        blocks.extend(image.map(|p| match pixels(std::path::Path::new(&p)) {
            Some((b64, mime)) => ContentBlock::image(b64, mime),
            None => ContentBlock::text(format!("! not_viewable · Claude reads png, jpeg, gif, webp → open {p}")),
        }));
        // rc 1 = the command failed, 2 = usage: the text says why either way
        Ok(if rc == 0 { CallToolResult::success(blocks) } else { CallToolResult::error(blocks) }.into())
    }
}

/// `work`, the host's spinner fed while it runs: every 5 s (`cli::TICK`) a `notifications/progress` with the
/// request's `_meta.progressToken`, `still working · 10 s`; none without a token (MCP spec, basic/utilities/progress).
/// A host may show it to the human; none shows it to the model before the answer, so the answer never waits on it.
async fn ticking<T>(
    mut work: tokio::task::JoinHandle<T>,
    token: Option<ProgressToken>,
    peer: &Peer<RoleServer>,
) -> Result<T, tokio::task::JoinError> {
    let Some(token) = token else { return work.await };
    let mut s = 0;
    loop {
        if let Ok(done) = tokio::time::timeout(crate::cli::TICK, &mut work).await {
            return done;
        }
        s += crate::cli::TICK.as_secs();
        let msg = format!("still working · {s} s");
        peer.notify_progress(ProgressNotificationParam::new(token.clone(), s as f64).with_message(msg)).await.ok();
    }
}

/// A stored picture as Claude views it (`outline::viewed`, standard tier), base64 + MIME type: the file as it is
/// when it fits and is at most 3.7 MB (Claude takes 5 MB of base64 a picture on Bedrock and Vertex, 10 MB on its
/// own API: platform.claude.com vision docs, "Request limits"), else downscaled (area average: no full-size float
/// copy) and sent as JPEG. Decoding is capped for old PCs (12k px a side, 256 MiB). None = not a picture this
/// build decodes (png, jpeg, gif, webp: the formats Claude reads), or past the cap.
fn pixels(path: &std::path::Path) -> Option<(String, &'static str)> {
    let b64 = |b: Vec<u8>| base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b);
    let (mut reader, mut limits) =
        (image::ImageReader::open(path).ok()?.with_guessed_format().ok()?, image::Limits::default());
    (limits.max_image_width, limits.max_image_height, limits.max_alloc) = (Some(12_000), Some(12_000), Some(256 << 20));
    reader.limits(limits);
    let (format, raw) = (reader.format()?, std::fs::read(path).ok().filter(|b| b.len() <= 3_700_000));
    let img = reader.decode().ok()?;
    let (size, _) = crate::outline::viewed(img.width(), img.height(), crate::outline::STD);
    if let Some(b) = raw.filter(|_| size == (img.width(), img.height())) {
        return Some((b64(b), format.to_mime_type()));
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.thumbnail_exact(size.0, size.1).to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
    Some((b64(out.into_inner()), "image/jpeg"))
}

/// A words.json `mcp_hosts` row: (name, the config files its `mcp add` writes, that command by hand with `{b}` =
/// the server's command, or none: fats writes the first file itself; the name a person knows).
pub type Host = (&'static str, Vec<&'static str>, &'static str, &'static str);

/// THE added rule: the first of `h`'s files (`read`) holding a server named `fats`: a TOML `[mcp_servers.fats]`
/// table (Codex) or a JSON `mcpServers.fats` at the top level (Claude Code's user scope and project `.mcp.json`,
/// Cursor) or under `projects.<this working dir>` (Claude Code's local scope: another project's server is not this
/// one's; its key matched as `cli::dir_key`). As words.json names it.
pub fn added_at(h: &Host) -> Option<&'static str> {
    let here = crate::cli::dir_key(&std::env::current_dir().unwrap_or_default().to_string_lossy());
    let json = |v: serde_json::Value| {
        let local = v["projects"].as_object().into_iter().flatten().filter(|(k, _)| crate::cli::dir_key(k) == here);
        [&v].into_iter().chain(local.map(|p| p.1)).any(|v| !v["mcpServers"]["fats"].is_null())
    };
    let toml = |t: &str| t.lines().any(|l| l.trim() == "[mcp_servers.fats]");
    // a file that never names `fats` is never parsed (`~/.claude.json` keeps every project's state)
    let on = |t: String| toml(&t) || t.contains("fats") && serde_json::from_str(&t).is_ok_and(json);
    h.1.iter().copied().find(|p| std::fs::read_to_string(path_of(p)).is_ok_and(on))
}

/// The hosts' names joined by `sep`: those holding fats (`added_at`) when `added` is true, those that do not when
/// false, else all of them.
fn names(added: Option<bool>, sep: &str) -> String {
    let on = |h: &&Host| added.is_none_or(|a| a == added_at(h).is_some());
    WORDS.mcp_hosts.iter().filter(on).map(|h| h.0).collect::<Vec<_>>().join(sep)
}

/// The status row of the hosts (`added_at`) and the command that adds fats to one; under `fats mcp` the server's
/// own. Never `ready` while none holds it: a model relays that word as "connected". The add is `· add:`, never an
/// arrow: a status with no key has one next move (`→ fats setup oxy`), and a second arrow gets relayed as a step.
pub fn hosts_row(b: &str) -> String {
    let (added, left) = (names(Some(true), ", "), names(Some(false), "|"));
    let add = if left.is_empty() { String::new() } else { format!(" · add: {b} setup mcp {left}") };
    match (crate::cli::FENCE.get(), added.is_empty()) {
        (Some(_), _) => "mcp: serving (this process) · local files from the working directory + the cache".into(),
        (None, true) => format!("mcp: not added to {}{add}", names(None, ", ")),
        (None, false) => format!("mcp: added to {added} · stdio, one tool fats(cmd){add}"),
    }
}

/// The menu's agent connection buttons (`p` = `mcp`, else none): `Add to <host>` each, `· added ✓` once it holds
/// fats (`added_at`).
pub fn buttons(p: &str) -> impl Iterator<Item = String> {
    let row = |h: &Host| format!("Add to {}{}", h.3, ["", " · added ✓"][usize::from(added_at(h).is_some())]);
    WORDS.mcp_hosts.iter().filter(move |_| p == "mcp").map(row)
}

/// Button `at` of screen `p` pressed: fats added to that host (`add`), the row it said. None = no host's button.
pub fn pressed(p: &str, at: usize, dry: bool) -> Option<String> {
    add(&[WORDS.mcp_hosts.get(at).filter(|_| p == "mcp")?.0], dry).lines.pop()
}

/// A host's config file as words.json `mcp_hosts` names it (`lang::local_path`: `~/` = the home dir):
/// `VAR=dir/file` = `$VAR/file` while `$VAR` is set (`settings::env`), else `dir/file` (Codex's `CODEX_HOME`).
fn path_of(p: &str) -> std::path::PathBuf {
    let set = p.split_once('=').and_then(|(v, p)| Some(crate::settings::env(v)? + &p[p.rfind('/')?..]));
    local_path(set.as_deref().unwrap_or(shown(p)))
}

/// `fats setup mcp [<host>]`: no host = which hosts hold fats and the command per host; a host = fats added to it
/// for the user: its own `mcp add` run when the host has one (`run`), else its file written (`write`); `dry` names
/// the command or the file and touches nothing. A tool call never gets here (`settings::guard`: `not_under_mcp`).
pub fn add(host: &[&str], dry: bool) -> Out {
    let Some(name) = host.first() else { return Out::new(vec![format!("= setup {}", hosts_row(bin()))]) };
    let Some(h) = WORDS.mcp_hosts.iter().find(|h| h.0 == *name) else {
        return Out::ask(&format!("? setup mcp {name} · hosts: {} setup mcp {}", bin(), names(None, "|")));
    };
    match (added_at(h), h.2.contains("{b}")) {
        (Some(at), _) => Out::new(vec![format!("= setup mcp {name} already added · {} · $0", shown(at))]),
        (None, true) => run(h, dry),
        (None, false) if dry => Out::new(vec![format!("= dry setup mcp {name} · would write {} · $0", h.1[0])]),
        (None, false) => write(h),
    }
}

/// A config file as a person reads it: `~/.codex/config.toml`, never `CODEX_HOME=`.
fn shown(p: &str) -> &str {
    p.rsplit('=').next().unwrap_or(p)
}

/// The server's command: this binary by the name a shell finds it when that is this same file (`/opt/homebrew/bin/
/// fats` outlives an upgrade; the versioned path behind it does not), else its own path; on Windows behind `cmd /c`.
fn server() -> Vec<String> {
    let exe = std::env::current_exe().unwrap_or_else(|_| bin().into());
    let same = |p: &PathBuf| std::fs::canonicalize(p).ok() == std::fs::canonicalize(&exe).ok();
    let path = on_path(bin()).filter(same).unwrap_or(exe).display().to_string();
    if cfg!(windows) { vec!["cmd".into(), "/c".into(), path] } else { vec![path] }
}

/// The first `name` in `PATH`'s dirs (Windows: `.exe`, `.cmd`, `.bat`), as a shell finds it.
fn on_path(name: &str) -> Option<PathBuf> {
    let ext: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    let dirs = std::env::var_os("PATH")?;
    std::env::split_paths(&dirs)
        .flat_map(|d| ext.iter().map(move |e| d.join(format!("{name}{e}"))))
        .find(|p| p.is_file())
}

/// The host's own `mcp add` (`h.2`, `{b}` = `server`), run when its binary is on `PATH`; its rc and first line of
/// output said when it fails. Without the binary: `no_host` and the command to type, never a success.
fn run(h: &Host, dry: bool) -> Out {
    let server = server();
    let words: Vec<String> =
        h.2.split(' ').flat_map(|w| if w == "{b}" { server.clone() } else { vec![w.into()] }).collect();
    let quote = |w: &String| if w.contains(' ') { format!("'{w}'") } else { w.clone() };
    let line = words.iter().map(quote).collect::<Vec<_>>().join(" ");
    let (name, tool) = (h.0, words[0].as_str());
    let fail = |code: &str, why: String| {
        Out::fail(code, format!("= ! {code} setup mcp {name} · nothing added · $0"), format!("! {code} {name} · {why}"))
    };
    let Some(exe) = on_path(tool) else {
        return fail("no_host", format!("{tool} not on PATH · install {} first, or add by hand: {line}", h.3));
    };
    if dry {
        return Out::new(vec![format!("= dry setup mcp {name} · would run {line} · $0")]);
    }
    let ran = std::process::Command::new(exe).args(&words[1..]).stdin(std::process::Stdio::null()).output();
    // `added` only once the host's own file holds fats (an alias or a wrapper can exit 0 and write nothing)
    match ran {
        Ok(_) if added_at(h).is_some() => Out::new(vec![format!("= setup mcp {name} added · {line} · $0")]),
        Ok(o) if o.status.success() => fail("host_failed", format!("ran {line}, fats not in {}", shown(h.1[0]))),
        Ok(o) => {
            let said = [o.stderr, o.stdout].map(|b| String::from_utf8_lossy(&b).trim().to_string());
            let first = said.iter().find_map(|s| s.lines().next()).unwrap_or("no output");
            fail("host_failed", format!("rc {} · {first}", o.status.code().unwrap_or(-1)))
        }
        Err(e) => fail("host_failed", format!("{tool} did not start: {e}")),
    }
}

/// A host with no `mcp add` (Cursor): `mcpServers.fats` = `{command, args}` written into its first file, every
/// other server and key kept; a file that is not a JSON object is left as it is (`host_failed`).
fn write(h: &Host) -> Out {
    let (name, at) = (h.0, shown(h.1[0]));
    let server = [server(), vec!["mcp".into()]].concat();
    let entry = serde_json::json!({"command": server[0], "args": server[1..]});
    let fail = |why: String| {
        let row = format!("! host_failed {name} · {at} {why} · add by hand: mcpServers.fats = {entry}");
        Out::fail("host_failed", format!("= ! host_failed setup mcp {name} · nothing written · $0"), row)
    };
    let file = path_of(h.1[0]);
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut v: serde_json::Value = match serde_json::from_str(if text.trim().is_empty() { "{}" } else { &text }) {
        Ok(v) => v,
        Err(e) => return fail(format!("is not JSON ({e})")),
    };
    let servers = v.as_object_mut().map(|o| o.entry("mcpServers").or_insert_with(|| serde_json::json!({})));
    let Some(servers) = servers.and_then(serde_json::Value::as_object_mut) else {
        return fail("holds no mcpServers object".into());
    };
    servers.insert("fats".into(), entry.clone());
    match crate::cache::jsave(&file, &v) {
        Ok(()) => Out::new(vec![format!("= setup mcp {name} added · {at} · $0")]),
        Err(e) => fail(format!("not written: {e}")),
    }
}

/// Serve on stdin/stdout until the client closes stdin (the parent exited). Exit code for `main`.
pub fn serve() -> i32 {
    let mut fence = vec![crate::settings::get().cache.value.clone()];
    fence.extend(std::env::current_dir());
    crate::cli::fence(fence);
    let served = crate::provider::block(async {
        let running = Fats.serve(rmcp::transport::stdio()).await.map_err(|e| e.to_string())?;
        running.waiting().await.map(|_| 0).map_err(|e| e.to_string())
    });
    served.unwrap_or_else(|| Err("no async runtime".into())).inspect_err(|e| eprintln!("fats mcp: {e}")).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytes` saved as `name` in a scratch dir of its own: (the dir, removed on drop; the file).
    fn saved(name: &str, bytes: &[u8]) -> (crate::scratch::Scratch, std::path::PathBuf) {
        let dir = crate::scratch::Scratch::new(&format!("mcp-{name}"));
        std::fs::write(dir.join(name), bytes).expect("write");
        let file = dir.join(name);
        (dir, file)
    }

    /// A noisy PNG that fits the view (1000x1000) but not Bedrock's 5 MB of base64 goes as a JPEG of the same size.
    #[test]
    fn big_file_goes_as_jpeg() {
        let mut seed = 7u32;
        let noisy = image::RgbaImage::from_fn(1000, 1000, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            image::Rgba(seed.to_be_bytes())
        });
        let mut png = std::io::Cursor::new(Vec::new());
        noisy.write_to(&mut png, image::ImageFormat::Png).expect("png");
        assert!(png.get_ref().len() > 3_700_000, "the raw file is over the cap");
        let (b64, mime) = pixels(&saved("noisy.png", png.get_ref()).1).expect("pixels");
        assert_eq!(mime, "image/jpeg");
        assert!(b64.len() < 5_000_000, "{} B of base64", b64.len());
    }

    /// A PNG whose header claims 30000x30000 is refused before any pixel buffer is made.
    #[test]
    fn huge_header_is_refused_fast() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(1, 1).write_to(&mut png, image::ImageFormat::Png).expect("png");
        let mut png = png.into_inner();
        png[16..24].copy_from_slice(&[0, 0, 0x75, 0x30, 0, 0, 0x75, 0x30]); // IHDR width, height = 30000
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let t0 = std::time::Instant::now();
        assert!(pixels(&saved("huge.png", &png).1).is_none());
        assert!(t0.elapsed().as_millis() < 500, "{:?}", t0.elapsed());
        // a real file past the side cap is not decoded either
        let mut wide = std::io::Cursor::new(Vec::new());
        image::GrayImage::new(12_001, 1).write_to(&mut wide, image::ImageFormat::Png).expect("png");
        assert!(pixels(&saved("wide.png", wide.get_ref()).1).is_none());
    }

    /// One directory under every spelling a Windows host or `canonicalize` writes is one key; elsewhere a key is
    /// the path as written, case and separators kept.
    #[test]
    fn a_dir_key_is_one_spelling() {
        let key = crate::cli::dir_key;
        let keys = [r"\\?\C:\Work\fats", r"C:\Work\fats", "C:/Work/fats", "c:/work/fats"].map(key);
        assert_eq!(keys.iter().all(|k| *k == keys[0]), cfg!(windows), "{keys:?}");
        assert_eq!(key(r"\\?\UNC\srv\share"), key(r"\\srv\share"), "the verbatim prefix is no part of a key");
        assert_eq!(key("/home/me/Work"), if cfg!(windows) { r"\home\me\work" } else { "/home/me/Work" });
    }

    /// PNG chunk CRC (ISO 3309, as the PNG spec gives it).
    fn crc32(bytes: &[u8]) -> u32 {
        !bytes.iter().fold(!0u32, |c, &b| {
            (0..8).fold(c ^ u32::from(b), |c, _| if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 })
        })
    }
}
