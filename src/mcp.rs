//! MCP surface: `fats mcp` = a stdio server with ONE tool, `fats(cmd)`. Its text is exactly what the CLI
//! prints for the same command line (same `cli::run`, same cache, same session), so there is one language
//! on every surface.
//!
//! What differs from the shell: local files are read only from the working directory and the cache (the
//! model, not the user, picks the path), and `-` (stdin) is refused because stdin carries the protocol.
//! Failures are `!` rows in the text; protocol errors only for requests that are not a `fats` call.

use crate::lang::CARD;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, ProgressToken, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};

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
    let schema = serde_json::json!({
        "type": "object",
        "properties": {"cmd": {"type": "string", "description": "e.g. u1#3"}},
        "required": ["cmd"],
    });
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
        let mut r = CallToolResult::success(blocks);
        r.is_error = Some(rc != 0); // rc 1 = the command failed, 2 = usage: the text says why either way
        Ok(r.into())
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
        let tick =
            ProgressNotificationParam::new(token.clone(), s as f64).with_message(format!("still working · {s} s"));
        peer.notify_progress(tick).await.ok();
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

/// Serve on stdin/stdout until the client closes stdin (the parent exited). Exit code for `main`.
pub fn serve() -> i32 {
    let mut fence = vec![crate::settings::get().cache.value.clone()];
    fence.extend(std::env::current_dir());
    crate::cli::fence(fence);
    crate::provider::block(async {
        match Fats.serve(rmcp::transport::stdio()).await {
            Ok(s) => s.waiting().await.map_or(1, |_| 0),
            Err(e) => {
                eprintln!("fats mcp: {e}");
                1
            }
        }
    })
    .unwrap_or_else(|| {
        eprintln!("fats mcp: no async runtime");
        1
    })
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

    /// PNG chunk CRC (ISO 3309, as the PNG spec gives it).
    fn crc32(bytes: &[u8]) -> u32 {
        !bytes.iter().fold(!0u32, |c, &b| {
            (0..8).fold(c ^ u32::from(b), |c, _| if c & 1 == 1 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 })
        })
    }
}
