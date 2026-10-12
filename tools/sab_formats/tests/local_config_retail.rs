//! The machine's `.sab-local.toml` names a real retail install and a real 2008 prototype.
//!
//! Game-gated: built by the `retail` feature, reads only the repo-root `.sab-local.toml`, and fails
//! naming the missing file, key or path. Every other retail test depends on these two resolving.

use std::path::Path;

use sab_formats::game_paths::{local_config_proto2008_bin360, local_config_retail_dir, retail_root};

#[test]
fn local_config_names_a_retail_install() {
    let dir = local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"));
    assert!(retail_root(&dir), "{} is not a retail install", dir.display());
}

#[test]
fn local_config_names_the_2008_prototype() {
    let bin360 =
        local_config_proto2008_bin360(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"));
    assert!(bin360.join("Saboteur.xex").is_file(), "{} has no Saboteur.xex", bin360.display());
}
