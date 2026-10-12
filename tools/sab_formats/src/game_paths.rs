//! Locating the game's files.
//!
//! * [`retail_root`] — whether a folder is a retail PC install (`Global/Dynamic0.megapack` +
//!   `Saboteur.exe`).
//! * [`proto2008_bin360`] — the June 2008 Xbox 360 prototype's `Bin360` content directory, recognised by
//!   its contents, not its folder name.
//! * [`LOCAL_CONFIG`] — the machine-local `.sab-local.toml` naming where the installs are.
//!
//! Game-gated tests (the `retail` feature) read [`LOCAL_CONFIG`] through the `local_config_*` functions
//! and panic with the error they return, which names the missing file, key or path. Tools read the
//! environment first ([`RETAIL_DIR_VAR`], [`PROTO2008_DIR_VAR`]) and fall back to the config
//! ([`retail_dir`], [`proto2008_bin360_dir`]).

use std::path::{Path, PathBuf};

/// Machine-local, git-ignored config naming where this machine keeps the game and its tools.
///
/// Searched for from a start folder upward; the nearest file wins. One `key = "…"` per line:
///
/// * `retail_dir` — a retail PC install root (GOG or Steam), see [`retail_root`].
/// * `proto2008_dir` — the June 2008 Xbox 360 prototype: its extract folder or its `Bin360` contents
///   under any name, see [`proto2008_bin360`].
pub const LOCAL_CONFIG: &str = ".sab-local.toml";

/// The `retail_dir` key of [`LOCAL_CONFIG`].
pub const RETAIL_DIR_KEY: &str = "retail_dir";
/// The `proto2008_dir` key of [`LOCAL_CONFIG`].
pub const PROTO2008_DIR_KEY: &str = "proto2008_dir";

/// The environment variable a tool or CI run uses to name the retail install.
pub const RETAIL_DIR_VAR: &str = "SAB_GAME_DIR";
/// The environment variable a tool or CI run uses to name the 2008 prototype.
pub const PROTO2008_DIR_VAR: &str = "SAB_2008_DIR";

/// True when `root` is a retail PC install: `Global/Dynamic0.megapack` and `Saboteur.exe` at the root.
///
/// GOG and the Steam build (which adds `steam_api.dll` and SecuROM) share this layout and their asset
/// formats.
pub fn retail_root(root: &Path) -> bool {
    root.join("Global").join("Dynamic0.megapack").is_file() && root.join("Saboteur.exe").is_file()
}

/// The June 2008 prototype's `Bin360` content directory for `root`, by contents — not by folder name.
///
/// Either `root` contains a `Bin360/` subdir (the extract's outer folder), or `root` itself holds
/// `Saboteur.xex` (the `Bin360` contents, under whatever name the folder was given).
pub fn proto2008_bin360(root: &Path) -> Option<PathBuf> {
    if root.join("Bin360").is_dir() {
        Some(root.join("Bin360"))
    } else if root.join("Saboteur.xex").is_file() {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// Every [`LOCAL_CONFIG`] from `start` upward, nearest first, each with its text.
fn local_configs(start: &Path) -> impl Iterator<Item = (PathBuf, String)> + '_ {
    start.ancestors().filter_map(|d| {
        let file = d.join(LOCAL_CONFIG);
        std::fs::read_to_string(&file).ok().map(|text| (file, text))
    })
}

/// The `<key> = "…"` value in one [`LOCAL_CONFIG`]'s text, or `None` when the key is absent. The key
/// must match exactly.
fn config_value(text: &str, key: &str) -> Option<PathBuf> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| PathBuf::from(v.trim().trim_matches('"')))
    })
}

/// The path `key` names in the nearest [`LOCAL_CONFIG`] above `start`, checked by `accept` (which turns
/// the configured path into the one callers want, or explains why it is not acceptable). The error says
/// which of the three things is wrong — no config, no key, or a path that does not qualify.
fn local_config_path(
    start: &Path,
    key: &str,
    accept: impl Fn(&Path) -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    let fix = format!("write `{key} = \"/path/to/it\"` into {LOCAL_CONFIG} at the repository root");
    let Some((file, text)) = local_configs(start).next() else {
        return Err(format!(
            "game-gated test: no {LOCAL_CONFIG} found in {} or any folder above it; {fix}",
            start.display()
        ));
    };
    let Some(path) = config_value(&text, key) else {
        return Err(format!("game-gated test: {} has no `{key}` key; {fix}", file.display()));
    };
    accept(&path).map_err(|why| {
        format!("game-gated test: {} names {key} = {}, which {why}", file.display(), path.display())
    })
}

fn accept_retail(p: &Path) -> Result<PathBuf, String> {
    if retail_root(p) {
        Ok(p.to_path_buf())
    } else {
        Err("is not a retail install (needs Global/Dynamic0.megapack and Saboteur.exe)".into())
    }
}

fn accept_proto2008(p: &Path) -> Result<PathBuf, String> {
    proto2008_bin360(p)
        .ok_or_else(|| "is not the 2008 prototype (needs a Bin360/ folder or Saboteur.xex)".into())
}

/// The retail install for a **game-gated test**: `retail_dir` in the nearest [`LOCAL_CONFIG`] above
/// `start`, and nothing else. Tests pass `env!("CARGO_MANIFEST_DIR")` and panic with the error.
pub fn local_config_retail_dir(start: &Path) -> Result<PathBuf, String> {
    local_config_path(start, RETAIL_DIR_KEY, accept_retail)
}

/// The 2008 prototype's `Bin360` content directory for a **game-gated test**: `proto2008_dir` in the
/// nearest [`LOCAL_CONFIG`] above `start`, resolved through [`proto2008_bin360`].
pub fn local_config_proto2008_bin360(start: &Path) -> Result<PathBuf, String> {
    local_config_path(start, PROTO2008_DIR_KEY, accept_proto2008)
}

/// A non-empty environment variable as a path.
fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// The retail install for a **tool**: [`RETAIL_DIR_VAR`] first, then `retail_dir` in the nearest
/// [`LOCAL_CONFIG`] above `start`. The error names what was tried and why each was rejected.
pub fn retail_dir(start: &Path) -> Result<PathBuf, String> {
    match env_path(RETAIL_DIR_VAR) {
        Some(p) => accept_retail(&p).map_err(|why| format!("{RETAIL_DIR_VAR} = {}, which {why}", p.display())),
        None => local_config_path(start, RETAIL_DIR_KEY, accept_retail)
            .map_err(|e| format!("{RETAIL_DIR_VAR} is not set, and {e}")),
    }
}

/// The 2008 prototype's `Bin360` directory for a **tool**: [`PROTO2008_DIR_VAR`] first, then
/// `proto2008_dir` in the nearest [`LOCAL_CONFIG`] above `start`.
pub fn proto2008_bin360_dir(start: &Path) -> Result<PathBuf, String> {
    match env_path(PROTO2008_DIR_VAR) {
        Some(p) => {
            accept_proto2008(&p).map_err(|why| format!("{PROTO2008_DIR_VAR} = {}, which {why}", p.display()))
        }
        None => local_config_path(start, PROTO2008_DIR_KEY, accept_proto2008)
            .map_err(|e| format!("{PROTO2008_DIR_VAR} is not set, and {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sab_formats_game_paths_{name}"));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn retail_root_needs_both_files() {
        let root = scratch("retail_root");
        assert!(!retail_root(&root));
        std::fs::create_dir_all(root.join("Global")).unwrap();
        std::fs::write(root.join("Global").join("Dynamic0.megapack"), b"x").unwrap();
        assert!(!retail_root(&root), "the megapack alone is not an install");
        std::fs::write(root.join("Saboteur.exe"), b"MZ").unwrap();
        assert!(retail_root(&root));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A renamed `Bin360` (the contents, under any folder name) is recognised by the same rule as the
    /// extract's outer folder.
    #[test]
    fn proto2008_bin360_is_found_by_contents_not_folder_name() {
        let root = scratch("proto2008");
        assert_eq!(proto2008_bin360(&root), None, "a random folder is not the build");
        let renamed = root.join("Some Other Name");
        std::fs::create_dir_all(&renamed).unwrap();
        std::fs::write(renamed.join("Saboteur.xex"), b"XEX2").unwrap();
        assert_eq!(proto2008_bin360(&renamed), Some(renamed.clone()));
        std::fs::create_dir_all(root.join("Bin360")).unwrap();
        assert_eq!(proto2008_bin360(&root), Some(root.join("Bin360")));
        std::fs::remove_dir_all(&root).ok();
    }

    /// The strict test resolver distinguishes its three failures and names the file and the key, and
    /// each key matches exactly.
    #[test]
    fn local_config_reports_each_failure() {
        let root = scratch("local_config");
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();

        let e = local_config_retail_dir(&deep).unwrap_err();
        assert!(e.contains("no .sab-local.toml found"), "{e}");
        assert!(e.contains("retail_dir = "), "{e}");

        let config = root.join(LOCAL_CONFIG);
        std::fs::write(&config, "# nothing here\n").unwrap();
        let e = local_config_retail_dir(&deep).unwrap_err();
        assert!(e.contains("has no `retail_dir` key"), "{e}");
        assert!(e.contains(&config.display().to_string()), "{e}");

        let install = root.join("install");
        std::fs::create_dir_all(install.join("Global")).unwrap();
        std::fs::write(&config, format!("retail_dir = \"{}\"\n", install.display())).unwrap();
        let e = local_config_retail_dir(&deep).unwrap_err();
        assert!(e.contains("is not a retail install"), "{e}");

        std::fs::write(install.join("Global").join("Dynamic0.megapack"), b"x").unwrap();
        std::fs::write(install.join("Saboteur.exe"), b"MZ").unwrap();
        assert_eq!(local_config_retail_dir(&deep), Ok(install.clone()));
        let e = local_config_proto2008_bin360(&deep).unwrap_err();
        assert!(e.contains("has no `proto2008_dir` key"), "{e}");

        let proto = root.join("proto");
        std::fs::create_dir_all(proto.join("Bin360")).unwrap();
        std::fs::write(
            &config,
            format!("retail_dir = \"{}\"\nproto2008_dir = \"{}\"\n", install.display(), proto.display()),
        )
        .unwrap();
        assert_eq!(local_config_proto2008_bin360(&deep), Ok(proto.join("Bin360")));
        assert_eq!(local_config_retail_dir(&deep), Ok(install));

        std::fs::remove_dir_all(&root).ok();
    }
}
