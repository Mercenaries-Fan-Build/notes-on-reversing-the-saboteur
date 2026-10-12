//! Where the tests find game data — one place, read from the environment, never a hardcoded path.
//!
//! Retail tests already use `SAB_GAME_DIR` (see `settings::detected`). The 2008 prototype tests used
//! to carry a Windows path to one developer's desktop in eight files, so on any other machine or OS
//! they could never run and reported a pass while doing nothing. `SAB_2008_DIR` is the prototype's
//! counterpart: point it at any folder the app itself accepts as the 2008 build
//! (`source::prototype2008_bin360` — the same rule detection and the loader use).

use std::path::PathBuf;

fn sab_2008_dir() -> Option<PathBuf> {
    std::env::var("SAB_2008_DIR").ok().filter(|d| !d.trim().is_empty()).map(PathBuf::from)
}

/// `SAB_2008_DIR` as given, when it is a folder `Prototype2008Source::open` will accept.
pub fn prototype2008_root() -> Option<PathBuf> {
    let dir = sab_2008_dir()?;
    crate::source::prototype2008_bin360(&dir).map(|_| dir)
}

/// The 2008 build's `Bin360` content directory, for tests that read files inside it directly.
pub fn prototype2008_bin360() -> Option<PathBuf> {
    crate::source::prototype2008_bin360(&sab_2008_dir()?)
}
