//! A test's scratch dir that removes itself. One file for both sides: the unit tests (`#[cfg(test)] mod scratch`
//! in lib.rs) and tests/*.rs (`#[path = "../src/scratch.rs"] mod scratch;`): a bare `temp_dir().join(…)` dir
//! outlives a failing test.

use std::path::{Path, PathBuf};

/// `$TMPDIR/fats-<name>-<pid>`, made empty, removed on drop: a failing test too (drop runs on unwind).
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new(name: &str) -> Scratch {
        let dir = Scratch::path(name);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    /// Where `new(name)` makes it: a test that sets the path in the environment once reads it here.
    pub(crate) fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fats-{name}-{}", std::process::id()))
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}
