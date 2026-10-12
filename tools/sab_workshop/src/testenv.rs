//! Where the tests find game data — one place, read from the environment, never a hardcoded path.
//!
//! Retail tests already use `SAB_GAME_DIR` (see `settings::detected`). The 2008 prototype tests used
//! to carry a Windows path to one developer's desktop in eight files, so on any other machine or OS
//! they could never run and reported a pass while doing nothing. `SAB_2008_DIR` is the prototype's
//! counterpart: point it at a `Bin360/` folder, or at a folder that contains one — the same two
//! shapes `Prototype2008Source::open` accepts.

use std::path::PathBuf;

/// `SAB_2008_DIR` as given, when it is a folder `Prototype2008Source::open` will accept.
pub fn prototype2008_root() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("SAB_2008_DIR").ok().filter(|d| !d.trim().is_empty())?);
    let is_bin360 = dir.file_name().map(|n| n == "Bin360").unwrap_or(false) && dir.is_dir();
    (is_bin360 || dir.join("Bin360").is_dir()).then_some(dir)
}

/// The `Bin360/` folder itself, for tests that read files inside it directly.
pub fn prototype2008_bin360() -> Option<PathBuf> {
    let root = prototype2008_root()?;
    if root.file_name().map(|n| n == "Bin360").unwrap_or(false) {
        Some(root)
    } else {
        Some(root.join("Bin360"))
    }
}
