//! Persisted user settings — the workshop's own configuration, stored per-user.
//!
//! Everything the app reads from the game is derived from ONE setting, `game_dir`. Before this the
//! install location was an emergent property of `Config::megapack`: `game_dir` was recovered by
//! walking two parents up from a hardcoded `C:/GOG Games/The Saboteur/Global/Dynamic0.megapack`, in
//! four separate places. Anyone whose copy lives elsewhere got a dead app and no way to fix it from
//! the UI, so this is the missing entry point as much as it is a convenience.
//!
//! **Why not next to the game?** `models::Groups` persists its overrides into `game_dir`, which
//! cannot work here: the game directory is the very thing being stored (you would have to know it to
//! find out where it is), and settings are the user's, not the install's. They go in the platform
//! config dir instead — `%APPDATA%/sab_workshop/settings.json` on Windows.

use std::path::PathBuf;

/// The languages that can be the Strings page default — mirrors `editor::LANGS`.
pub const LANG_NAMES: [(&str, &str); 7] = [
    ("EN", "English"),
    ("FR", "French"),
    ("DE", "German"),
    ("IT", "Italian"),
    ("PL", "Polish"),
    ("RU", "Russian"),
    ("RND", "Random"),
];

fn default_scale() -> f32 {
    1.2
}
fn default_slot() -> String {
    "02".into()
}
fn default_mod_name() -> String {
    "Untitled mod".into()
}

/// User settings. Every field carries a `serde(default)` so a settings file written by an older
/// build still loads — a missing key takes the default rather than failing the whole read and
/// silently resetting someone's install path.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// The game install root — the directory holding `Global/`, `Animations.pack`,
    /// `France.materials`. Everything else is derived from it.
    #[serde(default)]
    pub game_dir: String,
    /// Index into [`LANG_NAMES`]: which language the Strings page opens on.
    #[serde(default)]
    pub lang: usize,
    /// DLC overlay slot to publish into. `01` is the game's own, so a mod takes the next free one.
    #[serde(default = "default_slot")]
    pub dlc_slot: String,
    #[serde(default = "default_mod_name")]
    pub mod_name: String,
    /// Global type-size multiplier (see `gui::theme::type_scale`).
    #[serde(default = "default_scale")]
    pub type_scale: f32,
    /// Path to a `vgmstream-cli` executable, for decoding Wwise `.wem` voice lines so the Strings
    /// page can play them. Empty by default (the app then tries `vgmstream-cli` on PATH); it is a
    /// separate third-party tool, not bundled.
    #[serde(default)]
    pub vgmstream_path: String,
    /// Where `Export bundle` writes. Empty means `workshop_export/` beside the working directory —
    /// set from the verb bar's destination link, not typed.
    #[serde(default)]
    pub export_dir: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            game_dir: String::new(),
            lang: 0,
            dlc_slot: default_slot(),
            mod_name: default_mod_name(),
            type_scale: default_scale(),
            vgmstream_path: String::new(),
            export_dir: String::new(),
        }
    }
}

/// `%APPDATA%/sab_workshop/settings.json`, or `~/.config/sab_workshop/settings.json` elsewhere.
pub fn settings_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("sab_workshop").join("settings.json")
}

/// One thing the app needs from the install, and whether it is actually there. Shown as a checklist
/// on the Settings page so a wrong folder says WHICH file is missing rather than just "invalid".
pub struct Check {
    pub label: &'static str,
    pub rel: String,
    pub ok: bool,
    /// A missing optional file degrades one feature; a missing required one means nothing works.
    pub required: bool,
}

impl Settings {
    pub fn load() -> Settings {
        Settings::load_from(&settings_path())
    }

    /// Read settings from an explicit path. A missing or unreadable file is not an error — it is
    /// first run, and we fall back to auto-detection so the app is usable before anyone opens
    /// Settings at all.
    pub fn load_from(path: &std::path::Path) -> Settings {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if s.game_dir.is_empty() {
            s.game_dir = detect_install().unwrap_or_default();
        }
        s.clamp();
        s
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&settings_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Keep out-of-range values from a hand-edited file out of the UI.
    pub fn clamp(&mut self) {
        if self.lang >= LANG_NAMES.len() {
            self.lang = 0;
        }
        self.type_scale = self.type_scale.clamp(0.8, 2.0);
        self.game_dir = self.game_dir.replace('\\', "/");
        while self.game_dir.ends_with('/') {
            self.game_dir.pop();
        }
        if self.dlc_slot.trim().is_empty() {
            self.dlc_slot = default_slot();
        }
        self.vgmstream_path = self.vgmstream_path.trim().replace('\\', "/");
    }

    /// The configured `vgmstream-cli` path if it is set and present on disk; otherwise `None` (the
    /// caller may still fall back to a `vgmstream-cli` on PATH).
    pub fn vgmstream(&self) -> Option<String> {
        let p = self.vgmstream_path.trim();
        (!p.is_empty() && std::path::Path::new(p).exists()).then(|| p.to_string())
    }

    // ---- derived game paths: the whole point of storing one root ----
    pub fn megapack(&self) -> String {
        format!("{}/Global/Dynamic0.megapack", self.game_dir)
    }
    pub fn palettes(&self) -> String {
        format!("{}/Global/Palettes0.megapack", self.game_dir)
    }
    pub fn anim_pack(&self) -> String {
        format!("{}/Animations.pack", self.game_dir)
    }
    /// The WSAO material library — a loose file at the game root (see `wsao.rs`).
    pub fn wsao(&self) -> String {
        format!("{}/France.materials", self.game_dir)
    }

    /// Does this look like a Saboteur install, and if not, what is missing?
    pub fn check(&self) -> Vec<Check> {
        let one = |label: &'static str, rel: &str, required: bool| Check {
            label,
            rel: rel.to_string(),
            ok: !self.game_dir.is_empty()
                && std::path::Path::new(&format!("{}/{rel}", self.game_dir)).exists(),
            required,
        };
        vec![
            one("Model + texture pack", "Global/Dynamic0.megapack", true),
            one("Shared palette pack", "Global/Palettes0.megapack", true),
            one("Material library", "France.materials", true),
            one("Animations", "Animations.pack", false),
            one("UI text", "Cinematics/Dialog/English/GameText.dlg", false),
            one("Game templates", "DLC/01/GameTemplates.wsd", false),
        ]
    }

    /// True when every REQUIRED file is present — the bar for the app being able to do anything.
    pub fn is_valid(&self) -> bool {
        self.check().iter().all(|c| !c.required || c.ok)
    }
}

/// Settings for whatever install is on this machine, or `None` when there is none.
///
/// The entry point for anything that must not name an install location literally — tests and
/// headless runs included. A hardcoded `C:/GOG Games/...` passes on the machine it was written on
/// and is a lie everywhere else; this asks the same detector the app uses.
pub fn detected() -> Option<Settings> {
    detect_install().map(|game_dir| Settings { game_dir, ..Default::default() })
}

/// Look for an install in the usual places, so first run is usually zero-configuration. Ordered
/// most- to least-likely; the first hit that passes the required-file check wins.
pub fn detect_install() -> Option<String> {
    let mut roots: Vec<String> = Vec::new();
    for drive in ["C:", "D:", "E:"] {
        roots.push(format!("{drive}/GOG Games/The Saboteur"));
        roots.push(format!("{drive}/Program Files (x86)/GOG Galaxy/Games/The Saboteur"));
        roots.push(format!("{drive}/Program Files (x86)/Steam/steamapps/common/The Saboteur"));
        roots.push(format!("{drive}/SteamLibrary/steamapps/common/The Saboteur"));
        roots.push(format!("{drive}/Program Files (x86)/Pandemic Studios/The Saboteur"));
    }
    roots.into_iter().find(|r| {
        let probe = Settings { game_dir: r.clone(), ..Default::default() };
        probe.is_valid()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file from an older build (missing keys) must load with defaults rather than
    /// resetting the user's install path to nothing.
    #[test]
    fn partial_settings_file_keeps_known_keys() {
        let s: Settings = serde_json::from_str(r#"{"game_dir":"C:/Games/Sab"}"#).expect("parse");
        assert_eq!(s.game_dir, "C:/Games/Sab");
        assert_eq!(s.dlc_slot, "02");
        assert_eq!(s.type_scale, 1.2);
        assert_eq!(s.lang, 0);
    }

    #[test]
    fn clamp_normalises_separators_and_range() {
        let mut s = Settings {
            game_dir: "C:\\GOG Games\\The Saboteur\\".into(),
            lang: 99,
            type_scale: 9.0,
            ..Default::default()
        };
        s.clamp();
        assert_eq!(s.game_dir, "C:/GOG Games/The Saboteur");
        assert_eq!(s.lang, 0);
        assert_eq!(s.type_scale, 2.0);
    }

    /// Save then load must return exactly what went in, including a path that needed normalising —
    /// this is the whole contract of the Settings page's Save button.
    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join("sab_workshop_settings_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("settings.json");

        let mut written = Settings {
            game_dir: "D:/SteamLibrary/steamapps/common/The Saboteur".into(),
            lang: 3,
            dlc_slot: "07".into(),
            mod_name: "Resistance Pack".into(),
            type_scale: 1.45,
            vgmstream_path: "C:/tools/vgmstream/vgmstream-cli.exe".into(),
            export_dir: "D:/mods/bundles".into(),
        };
        written.clamp();
        // save_to must create the intermediate directories, not fail on them.
        written.save_to(&path).expect("save");

        let read = Settings::load_from(&path);
        assert_eq!(read.game_dir, written.game_dir);
        assert_eq!(read.lang, 3);
        assert_eq!(read.dlc_slot, "07");
        assert_eq!(read.mod_name, "Resistance Pack");
        assert_eq!(read.vgmstream_path, "C:/tools/vgmstream/vgmstream-cli.exe");
        assert_eq!(read.export_dir, "D:/mods/bundles");
        assert!((read.type_scale - 1.45).abs() < 1e-6);
        // A stored install path must NOT be replaced by auto-detection.
        assert_ne!(read.game_dir, detect_install().unwrap_or_default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Derived paths must match what the app previously hardcoded.
    #[test]
    fn derived_paths() {
        let s = Settings { game_dir: "C:/GOG Games/The Saboteur".into(), ..Default::default() };
        assert_eq!(s.megapack(), "C:/GOG Games/The Saboteur/Global/Dynamic0.megapack");
        assert_eq!(s.palettes(), "C:/GOG Games/The Saboteur/Global/Palettes0.megapack");
        assert_eq!(s.anim_pack(), "C:/GOG Games/The Saboteur/Animations.pack");
        assert_eq!(s.wsao(), "C:/GOG Games/The Saboteur/France.materials");
    }
}
