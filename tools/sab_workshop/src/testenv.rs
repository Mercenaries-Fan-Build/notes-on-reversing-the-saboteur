//! The builds the game-gated tests read, each opened once, from the paths in the repo-root
//! `.sab-local.toml` (`retail_dir`, `proto2008_dir`).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::source::{GameSource, Overrides};
use crate::sources::prototype2008::Prototype2008Source;
use crate::sources::retail_pc::RetailPcSource;

fn here() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn retail_arc() -> &'static Arc<RetailPcSource> {
    static SRC: OnceLock<Arc<RetailPcSource>> = OnceLock::new();
    SRC.get_or_init(|| {
        let dir = sab_formats::game_paths::local_config_retail_dir(here()).unwrap_or_else(|e| panic!("{e}"));
        Arc::new(RetailPcSource::open(&dir, &Overrides::default()).unwrap_or_else(|e| panic!("{}: {e}", dir.display())))
    })
}

fn proto2008_arc() -> &'static Arc<Prototype2008Source> {
    static SRC: OnceLock<Arc<Prototype2008Source>> = OnceLock::new();
    SRC.get_or_init(|| {
        let dir = sab_formats::game_paths::local_config_proto2008_bin360(here()).unwrap_or_else(|e| panic!("{e}"));
        Arc::new(Prototype2008Source::open(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())))
    })
}

pub fn retail() -> &'static RetailPcSource {
    retail_arc()
}

pub fn proto2008() -> &'static Prototype2008Source {
    proto2008_arc()
}

pub fn both_builds() -> [&'static dyn GameSource; 2] {
    [retail(), proto2008()]
}

/// Both builds, as the app holds a source.
pub fn both_shared() -> [Arc<dyn GameSource>; 2] {
    [retail_arc().clone(), proto2008_arc().clone()]
}
