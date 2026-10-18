//! Model-browser groupings: user overrides persisted next to the game. The *authoritative* grouping
//! is not a name heuristic — it comes from the game's own `GameTemplates` (see `assets.rs`), where an
//! `FxHumanBodySetup` / `Weapon` / `CAR` / `Prop` / … template names an assembled asset and references
//! its part meshes. This module only holds the display categories and the user's manual re-grouping.

use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level browser categories, in display order. Assets are placed by their template kind; a world
/// block is `World`; anything not referenced by a template is `Ungrouped` (never hidden — the user can
/// group it by hand).
pub const CATEGORIES: &[&str] = &["Characters", "Vehicles", "Weapons", "Props", crate::assets::WORLD, "Ungrouped"];

/// User overrides: asset name → category, saved as JSON next to the game so custom groupings persist.
pub struct Groups {
    overrides: HashMap<String, String>,
    path: PathBuf,
}

impl Groups {
    pub fn load(game_dir: &str) -> Groups {
        let path = PathBuf::from(game_dir).join("sab_workshop_model_groups.json");
        let overrides = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();
        Groups { overrides, path }
    }

    /// The effective category for an asset: the user override if present, else the template-derived
    /// `default`.
    pub fn category<'a>(&'a self, name: &str, default: &'a str) -> &'a str {
        match self.overrides.get(name) {
            Some(c) => c.as_str(),
            None => default,
        }
    }

    /// Assign an asset to a category. If it equals the template default, the override is dropped, so
    /// the file only holds genuine user changes. Persists immediately.
    pub fn set(&mut self, name: &str, category: &str, default: &str) {
        if category == default {
            self.overrides.remove(name);
        } else {
            self.overrides.insert(name.to_string(), category.to_string());
        }
        self.save();
    }

    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }

    fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.overrides) {
            let _ = std::fs::write(&self.path, s);
        }
    }
}
