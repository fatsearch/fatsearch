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

/// `keys` (provider name → key) as `<dir>/auth.json`, the file `fats setup` writes: 0600 in a 0700 dir. The one
/// way a test gives fats a key: it reads none from the environment.
pub(crate) fn auth(dir: &Path, keys: &[(&str, &str)]) {
    let keys: serde_json::Map<String, serde_json::Value> =
        keys.iter().map(|(n, k)| (n.to_string(), (*k).into())).collect();
    std::fs::create_dir_all(dir).expect("key dir");
    let file = dir.join("auth.json");
    std::fs::write(&file, serde_json::json!({"version": 1, "keys": keys}).to_string()).expect("auth.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).expect("chmod key dir");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).expect("chmod auth.json");
    }
}
