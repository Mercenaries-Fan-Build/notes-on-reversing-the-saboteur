//! The **skeleton key**: one interface across every version of The Saboteur / WildStar we care
//! about. The workshop's viewer, exporter, and browsers talk to a `dyn GameSource` and never touch
//! version-specific types (megapacks, packlogs, DTEX vs DDS, PC vs Xbox tiling, …); each version
//! has its own impl under `sources/` and can lay out its files any way it likes.
//!
//! The community wants secret-hunting parity across the retail GOG PC build, the 2008 Xbox 360
//! prototype, and whatever else turns up. Rather than hard-fork the workshop per version, we
//! **fingerprint the selected folder**, pick the right impl, and let each impl translate its files
//! into the shared vocabulary below.
//!
//! Adding a new version = one new file under `sources/`, one new fingerprint in `detect_version`.

use std::path::Path;

use crate::dtex::CpuTexture;

/// A game build the workshop knows how to read. The `label()` is what the UI shows the user; the
/// enum discriminant is what code branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GameVersion {
    /// The clean GOG PC release. Also matches the Steam SecuROM twin (same content).
    RetailPc,
    /// The pre-release Xbox 360 build (May 22 2008 content freeze + Jun 26 rebuild) — 17,148
    /// loose files under `Bin360/`, per-block `.Pack` containers with `.packlog` manifests,
    /// `SBL4` sub-pack magic, `.dds` textures, `.xex` binaries.
    Prototype2008Xbox360,
}

impl GameVersion {
    pub fn label(self) -> &'static str {
        match self {
            GameVersion::RetailPc => "Retail PC (GOG / Steam)",
            GameVersion::Prototype2008Xbox360 => "Prototype (2008-05 Xbox 360)",
        }
    }
}

/// One asset the source can hand out — a mesh, a texture, a material, etc. Kept opaque so each
/// impl can key its lookups however it wants (offset into a megapack, filename hash on disk, …).
#[derive(Clone, Debug)]
pub struct AssetHandle {
    /// A version-specific opaque key. The source is the only thing that interprets it.
    pub key: u64,
    /// Human-readable name. Empty when the source only has a hash (which the retail impl
    /// resolves via GameTemplates; the 2008 impl resolves via the `.packlog` inventory).
    pub name: String,
    pub kind: AssetKind,
    /// The plaintext content hash (typically `pandemic_hash(name)`) when the source knows it.
    /// Lets consumers join across kinds — e.g. "find every texture / material handle that
    /// belongs to the same block as this mesh". `None` when the source has only opaque keys.
    pub hash: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Mesh,
    Texture,
    Material,
    Animation,
    UiString,
    Other,
}

/// What a source has been able to figure out about the selected folder. `detect_version` returns
/// this alongside the version so the UI can show "here's what I found" even when opening fails
/// (a bad copy of the prototype, a partial GOG install, etc.).
#[derive(Debug, Default)]
pub struct SourceFingerprint {
    /// Which files/paths matched — for the picker UI to show WHY we chose this version.
    pub matched: Vec<String>,
    /// Things that looked wrong but weren't fatal — missing DLCs, absent `France.materials`,
    /// stale symbol PDBs, etc. Displayed as warnings.
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

/// The skeleton-key interface. Every version implements this; every consumer talks to `dyn`
/// this — no `if version == Retail` branches in app code, ever.
///
/// Methods return `Result<_, String>` because a source can be partially usable — a prototype
/// with corrupted assets should still open and list what it can.
pub trait GameSource: Send + Sync {
    fn version(&self) -> GameVersion;
    /// A UI-friendly name including the root path or a version tag.
    fn label(&self) -> String;
    /// What the fingerprint told us at open time.
    fn fingerprint(&self) -> &SourceFingerprint;

    // ── enumeration ──────────────────────────────────────────────────────────────────────────

    /// Every mesh this source can hand out. Cheap: header windows only, no decode.
    fn list_meshes(&self) -> Vec<AssetHandle>;
    /// Every texture the source can hand out by hash + friendly name (when known).
    fn list_textures(&self) -> Vec<AssetHandle> { Vec::new() }
    /// Every material the source can enumerate. Retail's WSAO records are keyed by drawcall
    /// hash and consumed on demand via `textures_for_material`; the default returns empty so
    /// the trait stays additive. The 2008 impl has a real answer because its `.materials`
    /// slots are pre-parsed at open time and every UID is known before any drawcall fires.
    fn list_materials(&self) -> Vec<AssetHandle> { Vec::new() }
    /// Every animation clip the source can enumerate. Retail's clips live in `Animations.pack`
    /// (`AP0L`/`ANIM` -> concatenated Havok `hkaSplineCompressedAnimation` blob) and are
    /// consumed via `crate::anim_index` + `crate::havok` — those already have first-class
    /// entry points, so this trait method is only implemented by the 2008 impl for now, whose
    /// clips are Pandemic-custom `ANM0(WAV|DEL|NOC)0` records with no Havok blob at all.
    /// Default returns empty so the trait stays additive.
    fn list_animations(&self) -> Vec<AssetHandle> { Vec::new() }

    // ── read / resolve ───────────────────────────────────────────────────────────────────────

    /// The raw MSHA bytes for a mesh handle, ready for `meshload::assemble`. Byte-exact —
    /// bundle export needs it that way.
    fn read_mesh_bytes(&self, handle: &AssetHandle) -> Result<Vec<u8>, String>;

    /// Given a material hash, return the per-slot texture hashes: `[diffuse, spec, normal, mask]`.
    /// A slot the material does not declare comes back `None`. The material-hash namespace is
    /// version-specific — retail uses pandemic_hash of a WSAO material name.
    fn textures_for_material(&self, mat: u32) -> Option<[Option<u32>; 4]>;

    /// Fetch a texture by hash and decode to RGBA8. `None` if the hash isn't known here.
    fn load_texture(&self, hash: u32) -> Result<CpuTexture, String>;

    /// Best-effort: what friendly name does this source have for a hash? Used to fill in the
    /// picker rows / manifest when the retail path only has a hash but the 2008 packlogs know
    /// the plaintext name.
    fn name_of_hash(&self, _hash: u32) -> Option<String> { None }
}

// ── version detection ────────────────────────────────────────────────────────────────────────

/// Sniff the folder and identify which build it is. Returns `None` if nothing recognized.
///
/// Detection is by **fingerprint**, not by user assertion — a mis-typed path can't crash a loader
/// meant for a different version. Each version's fingerprint is layered from cheapest first to
/// most specific: file existence → filename patterns → magic-byte reads only if needed.
pub fn detect_version(root: &Path) -> Option<(GameVersion, SourceFingerprint)> {
    // 2008 Xbox 360 prototype: the Bin360/ subdir is the giveaway; inside it, `Saboteur.xex`
    // (XEX2 magic) plus 1399 `.packlog` files pin it down. The user's rar layout puts everything
    // under `Bin360/` — accept either the parent (containing Bin360) or Bin360 itself.
    let bin360 = if root.join("Bin360").is_dir() {
        Some(root.join("Bin360"))
    } else if root.join("Saboteur.xex").is_file() {
        Some(root.to_path_buf())
    } else {
        None
    };
    if let Some(dir) = bin360 {
        let mut fp = SourceFingerprint::default();
        fp.matched(format!("Bin360 root: {}", dir.display()));
        if dir.join("Saboteur.xex").is_file() {
            fp.matched("Saboteur.xex (XEX2)");
        }
        if dir.join("Animations.pack").is_file() {
            fp.matched("Animations.pack");
        }
        if dir.join("symbols/build_info.txt").is_file() {
            fp.matched("symbols/build_info.txt");
        }
        return Some((GameVersion::Prototype2008Xbox360, fp));
    }

    // Retail PC (GOG or Steam SecuROM twin): `Global/Dynamic0.megapack` + `Saboteur.exe` at root.
    // Steam adds `steam_api.dll` but the assets/formats are identical to GOG (see the
    // `steam-build-is-securom-twin-of-gog` memory).
    let megapack = root.join("Global").join("Dynamic0.megapack");
    let exe = root.join("Saboteur.exe");
    if megapack.is_file() && exe.is_file() {
        let mut fp = SourceFingerprint::default();
        fp.matched("Global/Dynamic0.megapack");
        fp.matched("Saboteur.exe");
        if root.join("France.materials").is_file() {
            fp.matched("France.materials (WSAO)");
        } else {
            fp.warn("France.materials missing — WSAO material resolution will be unavailable");
        }
        if root.join("steam_api.dll").is_file() {
            fp.matched("steam_api.dll (Steam SecuROM twin)");
        }
        return Some((GameVersion::RetailPc, fp));
    }

    None
}

/// Open the folder as a live `GameSource`. `detect_version` first, then dispatch to the version's
/// impl. Errors when the fingerprint matches but the impl can't open (corrupt megapack, etc.).
pub fn open_source(root: &Path) -> Result<Box<dyn GameSource>, String> {
    let (v, _fp) = detect_version(root)
        .ok_or_else(|| format!("no known Saboteur build detected in {}", root.display()))?;
    match v {
        GameVersion::RetailPc => Ok(Box::new(crate::sources::retail_pc::RetailPcSource::open(root)?)),
        GameVersion::Prototype2008Xbox360 => {
            Ok(Box::new(crate::sources::prototype2008::Prototype2008Source::open(root)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_retail_pc_install() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no retail install detected");
            return;
        };
        let root = std::path::Path::new(&s.game_dir);
        let (v, fp) = detect_version(root).expect("retail detected");
        assert_eq!(v, GameVersion::RetailPc);
        assert!(fp.matched.iter().any(|m| m.contains("Dynamic0.megapack")));
    }

    #[test]
    fn detects_2008_prototype_when_bin360_present() {
        // Only runs on machines that have the prototype extracted. Skip cleanly otherwise.
        let root = std::path::Path::new("c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files");
        if !root.join("Bin360").is_dir() {
            eprintln!("skip: no Bin360/ prototype at expected path");
            return;
        }
        let (v, fp) = detect_version(root).expect("prototype detected");
        assert_eq!(v, GameVersion::Prototype2008Xbox360);
        assert!(fp.matched.iter().any(|m| m.contains("Bin360")));
    }

    #[test]
    fn rejects_a_random_folder() {
        let root = std::env::temp_dir();
        assert!(detect_version(&root).is_none());
    }
}
