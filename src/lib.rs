//! `fats`: web pages and documents → small, clean, cached Markdown for AI agents.
//! Module map (one crate for now): lang (argv + the only printer) · html (html5ever → node arena) ·
//! clean (prune, loss guard, doc; `kind_of` = what a body is) · text (text documents) · provider (plain, Bright Data, Oxylabs) · fetch (router, content check) · outline (sections, reads) · search (filter ladder) · cache (ids,
//! sessions, snapshots) · stats (status, stats, config, dry) · cli (dispatch) · mcp (`fats mcp`, one tool) · settings (every knob + source) · tables (generated data).

#![forbid(unsafe_code)]

pub mod cache;
pub mod clean;
pub mod cli;
pub mod fetch;
pub mod html;
pub mod lang;
pub mod mcp;
pub mod outline;
pub mod pdf;
pub mod provider;
pub mod search;
pub mod settings;
pub mod stats;
pub mod tables;
pub mod text;
