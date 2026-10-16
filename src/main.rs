//! Binary `fats`: argv → lang → core → stdout. All logic lives in the library.

#![forbid(unsafe_code)]

use std::io::Write;

fn main() {
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match argv.as_slice() {
        [m] if m == "mcp" => std::process::exit(fats::mcp::serve()),
        [c, id] if c == "--continue" => std::process::exit(fats::cli::resume(id)),
        _ => {}
    }
    let (text, rc, _) = fats::cli::run(&argv);
    // a closed pipe (`fats u1 | head -1`) is not an error worth a panic
    std::io::stdout().lock().write_all(text.as_bytes()).ok();
    fats::cache::sweep(); // the cache lifetime, after the answer's bytes (a harness still waits for the exit)
    std::process::exit(rc);
}
