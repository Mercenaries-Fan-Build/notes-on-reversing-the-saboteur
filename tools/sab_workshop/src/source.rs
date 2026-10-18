//! The **skeleton key**: one interface across every version of The Saboteur / WildStar we care
//! about. The workshop's viewer, exporter, browsers and editor pages talk to a `dyn GameSource` and
//! never touch version-specific types (megapacks, SBL packs, DTEX vs Xenos textures, PC vs Xbox
//! byte order, …); each version has its own impl under `sources/` and translates its files into the
//! shared vocabulary below: template-assembled assets, decoded parts, engine material bindings, clips
//! bound by bone-name hash, game text, and the texture pool.
//!
//! Adding a new version = one new file under `sources/`, one new fingerprint in `detect_version`.

use std::path::{Path, PathBuf};

use sab_formats::gametemplates::GameTemplates;

use crate::clip::AnimClip;
use crate::dtex::CpuTexture;
use crate::formats::SubMesh;
use crate::meshload::LoadedMesh;
use crate::resolve::ModelTex;

/// A game build the workshop knows how to read. The `label()` is what the UI shows the user; the
/// enum discriminant is what code branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameVersion {
    /// The clean GOG PC release. Also matches the Steam SecuROM twin (same content).
    RetailPc,
    /// The 2008-05-20 Xbox 360 prototype — 17,148 loose files under `Bin360/`, `SBL*` packs,
    /// `BLU2` templates, `ANM0` clips, `.xex` binaries.
    Prototype2008Xbox360,
}

impl GameVersion {
    pub fn label(self) -> &'static str {
        match self {
            GameVersion::RetailPc => "Retail PC (GOG / Steam)",
            GameVersion::Prototype2008Xbox360 => "Prototype (2008-05 Xbox 360)",
        }
    }

    /// What a model's saved material bindings are filed under, before its name: the builds share
    /// asset names (both have `FBS_RS_Sean`) but not texture names.
    pub fn profile_prefix(self) -> &'static str {
        match self {
            GameVersion::RetailPc => "",
            GameVersion::Prototype2008Xbox360 => "proto2008_",
        }
    }
}

/// One mesh record a build offers to the asset list and the assembler.
#[derive(Clone, Debug)]
pub struct MeshItem {
    pub name: String,
    /// Where the record is stored: a megapack and offset, or a pack path.
    pub location: String,
    /// A world-block mesh (the 2008 build's world packs): browsable on its own, never one of a
    /// template's parts — the engine streams it with the world, not with the template.
    pub world: bool,
}

/// What a template's reference to a mesh name resolves to.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshFind {
    Found(usize),
    /// No record of that name among the build's assembled meshes.
    Absent,
    /// Records of that name with different contents, none of them the template's own: their pack
    /// paths.
    Ambiguous(Vec<String>),
}

/// A part's record exactly as stored, for a bundle's `raw/`.
pub struct RawRecord {
    /// The file name it is written under.
    pub file: String,
    pub bytes: Vec<u8>,
    /// Where it came from.
    pub location: String,
}

/// One part of an assembled model at one level of detail: its mesh item, and the index range its
/// geometry of that level occupies.
#[derive(Clone, Debug)]
pub struct PartRange {
    pub mesh: usize,
    pub name: String,
    pub index_start: u32,
    pub index_count: u32,
    pub lod: u8,
}

/// One clip of the build's catalog.
#[derive(Clone, Debug)]
pub struct ClipRow {
    pub name: String,
    pub duration: f32,
    /// The bone name hash each track drives, in track order (`0` for an unbound track).
    pub bones: Vec<u32>,
}

/// One texture the Icons page lists.
#[derive(Clone, Debug)]
pub struct IconRow {
    pub name: String,
    pub hash: u32,
    /// Opaque to everything but the source that made it.
    pub key: (usize, usize, usize),
}

/// What a source has been able to figure out about the selected folder. `detect_version` returns
/// this alongside the version so the UI can show "here's what I found" even when opening fails.
#[derive(Debug, Default)]
pub struct SourceFingerprint {
    /// Which files/paths matched — for the picker UI to show WHY we chose this version.
    pub matched: Vec<String>,
    /// Things that looked wrong but did not stop the open.
    pub warnings: Vec<String>,
}

impl SourceFingerprint {
    pub fn matched(&mut self, s: impl Into<String>) {
        self.matched.push(s.into());
    }
    pub fn warn(&mut self, s: impl Into<String>) {
        self.warnings.push(s.into());
    }
}

/// Paths that replace what the retail build derives from its root (command-line flags).
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    pub megapack: Option<String>,
    pub palettes: Option<String>,
    pub wsao: Option<String>,
    pub anim_pack: Option<String>,
    /// A generated `anim_bone_map.json` catalog in place of the pack's `ANIM` block.
    pub anim_index: Option<String>,
    /// A folder of loose `<hash>.dtex` texture records consulted before the packs.
    pub dtex_dir: Option<String>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.megapack.is_none()
            && self.palettes.is_none()
            && self.wsao.is_none()
            && self.anim_pack.is_none()
            && self.anim_index.is_none()
            && self.dtex_dir.is_none()
    }
}

/// The skeleton-key interface. Every version implements this; every consumer talks to `dyn` this —
/// no `if version == Retail` branches in app code.
pub trait GameSource: Send + Sync {
    fn version(&self) -> GameVersion;
    /// A UI-friendly name including the root path.
    fn label(&self) -> String;
    /// What the fingerprint told us at open time.
    fn fingerprint(&self) -> &SourceFingerprint;

    // ── models ───────────────────────────────────────────────────────────────────────────────

    /// The build's object definitions, which the asset list is assembled from.
    fn templates(&self) -> &GameTemplates;
    /// Every mesh record the asset list and the assembler index.
    fn meshes(&self) -> &[MeshItem];
    /// The record a reference from template `template` to the mesh named by `hash` loads.
    fn find_mesh(&self, hash: u32, template: &str) -> MeshFind;
    /// Decode one mesh record as a part for [`crate::meshload::merge`].
    fn load_part(&self, mesh: usize) -> Result<LoadedMesh, String>;
    /// The record as stored, byte-exact.
    fn raw_part(&self, mesh: usize) -> Result<RawRecord, String>;
    /// Bind textures to an assembled model's submeshes from the engine's own material data.
    /// `uvless` lists index ranges with no texture coordinates.
    fn resolve_textures(&self, parts: &[PartRange], submeshes: &[SubMesh], uvless: &[(u32, u32)]) -> Result<ModelTex, String>;
    /// What the source knows about one mesh record, as label/value rows.
    fn describe_mesh(&self, mesh: usize) -> Vec<(String, String)>;
    /// World blocks the build's world meshes are streamed in, each with its [`MeshItem::world`] items.
    fn world_blocks(&self) -> &[(String, Vec<usize>)];

    // ── animation ────────────────────────────────────────────────────────────────────────────

    /// The animation clips, in catalog order.
    fn clips(&self) -> &[ClipRow];
    /// Decode one clip of [`GameSource::clips`].
    fn load_clip(&self, index: usize) -> Result<Box<dyn AnimClip>, String>;

    // ── editor pages ─────────────────────────────────────────────────────────────────────────

    /// The game-text file of each language the build ships: `(label, path)`.
    fn game_text_files(&self) -> Vec<(String, PathBuf)>;
    /// The textures the Icons page lists; `progress(done, total)` reports a long sweep.
    fn icon_textures(&self, progress: &dyn Fn(usize, usize)) -> Result<Vec<IconRow>, String>;
    /// Decode an Icons-page texture: its finest level, or for `preview` the smallest level at least
    /// that size.
    fn decode_icon(&self, row: &IconRow, preview: Option<u32>) -> Result<CpuTexture, String>;
    /// Whether the editor may publish edits into this build.
    fn publishable(&self) -> bool;
}

// ── version detection ────────────────────────────────────────────────────────────────────────

/// Sniff the folder and identify which build it is. Returns `None` if nothing recognized.
///
/// Detection is by **fingerprint**, not by user assertion — a mis-typed path can't crash a loader
/// meant for a different version. Which folders qualify is decided by `sab_formats::game_paths`
/// (`proto2008_bin360`, `retail_root`).
pub fn detect_version(root: &Path) -> Option<(GameVersion, SourceFingerprint)> {
    if let Some(dir) = sab_formats::game_paths::proto2008_bin360(root) {
        let mut fp = SourceFingerprint::default();
        fp.matched(format!("Bin360 root: {}", dir.display()));
        if dir.join("Saboteur.xex").is_file() {
            fp.matched("Saboteur.xex (XEX2)");
        }
        if dir.join("Animations.pack").is_file() {
            fp.matched("Animations.pack");
        }
        if dir.join("GameTemplates.wsd").is_file() {
            fp.matched("GameTemplates.wsd (BLU2)");
        }
        return Some((GameVersion::Prototype2008Xbox360, fp));
    }

    // Retail PC (GOG or Steam SecuROM twin): `Global/Dynamic0.megapack` + `Saboteur.exe` at root.
    if sab_formats::game_paths::retail_root(root) {
        let mut fp = SourceFingerprint::default();
        fp.matched("Global/Dynamic0.megapack");
        fp.matched("Saboteur.exe");
        if root.join("France.materials").is_file() {
            fp.matched("France.materials (WSAO)");
        } else {
            fp.warn("France.materials missing — opening this build needs its material table");
        }
        if root.join("steam_api.dll").is_file() {
            fp.matched("steam_api.dll (Steam SecuROM twin)");
        }
        return Some((GameVersion::RetailPc, fp));
    }

    None
}

/// Open the folder as a live `GameSource`: `detect_version` first, then the version's impl.
pub fn open_source(root: &Path, overrides: &Overrides) -> Result<Box<dyn GameSource>, String> {
    let (v, _fp) = detect_version(root).ok_or_else(|| format!("no known Saboteur build detected in {}", root.display()))?;
    match v {
        GameVersion::RetailPc => Ok(Box::new(crate::sources::retail_pc::RetailPcSource::open(root, overrides)?)),
        GameVersion::Prototype2008Xbox360 => {
            if !overrides.is_empty() {
                return Err(format!(
                    "{} is the 2008 prototype; --megapack, --palettes, --wsao, --pack, --index and --dtexdir name retail files",
                    root.display()
                ));
            }
            Ok(Box::new(crate::sources::prototype2008::Prototype2008Source::open(root)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_random_folder() {
        let root = std::env::temp_dir();
        assert!(detect_version(&root).is_none());
    }

    /// Both builds named in the repo-root `.sab-local.toml` fingerprint as their version.
    #[cfg(feature = "retail")]
    #[test]
    fn detects_both_builds() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let retail = sab_formats::game_paths::local_config_retail_dir(here).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(detect_version(&retail).map(|v| v.0), Some(GameVersion::RetailPc));
        let proto = sab_formats::game_paths::local_config_proto2008_bin360(here).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(detect_version(&proto).map(|v| v.0), Some(GameVersion::Prototype2008Xbox360));
    }

    /// Every clip of both builds loads and samples to its track count, except the 30 loose 2008 clips
    /// `LoadAnimFile` rejects (`docs/2008_animations_format.md`).
    #[cfg(feature = "retail")]
    #[test]
    fn every_clip_loads() {
        for src in crate::testenv::both_builds() {
            let mut rejected = Vec::new();
            for (i, row) in src.clips().iter().enumerate() {
                match src.load_clip(i) {
                    Ok(c) => {
                        assert_eq!(c.bind_hashes().len(), c.num_transform_tracks(), "{}: {}", src.label(), row.name);
                        assert_eq!(c.sample_all(c.duration() * 0.5).len(), c.num_transform_tracks(), "{}: {}", src.label(), row.name);
                    }
                    Err(e) => rejected.push(e),
                }
            }
            let want = match src.version() {
                GameVersion::RetailPc => 0,
                GameVersion::Prototype2008Xbox360 => 30,
            };
            assert_eq!(rejected.len(), want, "{}: {}", src.label(), rejected.join("\n"));
        }
    }

    /// Clips bind to the player's rig by bone hash: every bound track of the 2008 `Sean_wave`, and on
    /// retail every track of every biped clip.
    #[cfg(feature = "retail")]
    #[test]
    fn clips_bind_to_the_player_by_hash() {
        for src in crate::testenv::both_builds() {
            let all = crate::assets::build(src);
            let p = crate::assets::player(src.templates(), &all).unwrap();
            let rig = crate::boot::assemble(src, &all[p].name, &all[p].assembly()).unwrap().lm.bone_hashes;
            let binds = |i: usize| {
                let c = src.load_clip(i).unwrap();
                let tracks = crate::clip::bind_to_rig(c.as_ref(), &rig);
                c.bind_hashes().iter().zip(&tracks).all(|(h, t)| *h == 0 || *t >= 0)
            };
            match src.version() {
                GameVersion::Prototype2008Xbox360 => {
                    let i = src.clips().iter().position(|c| c.name == "animations/Sean_wave.hkx").expect("Sean_wave");
                    assert!(binds(i), "Sean_wave does not bind to {}", all[p].name);
                }
                GameVersion::RetailPc => {
                    let on_rig: Vec<usize> = (0..src.clips().len()).filter(|&i| src.clips()[i].bones.iter().all(|h| *h == 0 || rig.contains(h))).collect();
                    assert!(on_rig.len() > 1000, "only {} retail clips are on the player's rig", on_rig.len());
                    assert!(on_rig.iter().take(50).all(|&i| binds(i)));
                }
            }
        }
    }
}
