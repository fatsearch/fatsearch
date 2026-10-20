//! Binary `fats`: argv → lang → core → stdout. All logic lives in the library.

#![forbid(unsafe_code)]

use std::io::Write;

fn main() {
    fats::stats::hook(); // the start time; a panic in our code is counted (`stats::crash`), then the default hook prints it
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match argv.as_slice() {
        [m] if m == "mcp" => std::process::exit(fats::mcp::serve()),
        [c, id] if c == "--continue" => std::process::exit(fats::cli::resume(id)),
        _ if let Some(rc) = fats::setup::tty(&argv) => std::process::exit(rc),
        _ => {}
    }
    let (text, rc, _) = fats::cli::run(&argv);
    // a closed pipe (`fats u1 | head -1`) is not an error worth a panic
    std::io::stdout().lock().write_all(text.as_bytes()).ok();
    fats::telemetry::tick(&argv); // the cache sweep, the notice, the update check: after the answer's bytes, stderr only
    std::process::exit(rc);
}
