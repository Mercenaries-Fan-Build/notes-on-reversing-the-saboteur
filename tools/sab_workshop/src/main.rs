//! sab_workshop — a native wgpu + egui viewer for The Saboteur characters.
//!
//! Assembles a character out of the game's own packs (mesh + rig + textures), lists every animation
//! clip from `Animations.pack` in a searchable panel, decodes the selected clip on demand and plays
//! it back with GPU skinning under an orbit camera. A **game install is the only input** — no
//! generated files, no repo checkout.
//!
//! Run:
//!   cargo run -p sab_workshop --release
//!   cargo run -p sab_workshop --release -- --game "C:/GOG Games/The Saboteur" --boot Mattias
//!   cargo run -p sab_workshop --release -- --mesh <smsh> --skel <skel>   # inspect loose files
//!   cargo run -p sab_workshop --release -- --help

mod anim_index;
mod app;
mod assets;
mod bone_names;
mod boot;
mod bundle;
mod camera;
mod dtex;
mod editor;
mod formats;
mod gui;
mod havok;
mod meshload;
mod models;
mod pack;
mod render;
mod resolve;
mod settings;
mod skinning;
mod wsao;

/// Resolved input paths (CLI overrides the built-in defaults).
#[derive(Clone)]
pub struct Config {
    /// The game install root. Every game-side path below is DERIVED from this (see `settings.rs`);
    /// it is carried here as a first-class field so nothing has to recover it by walking parents up
    /// from a megapack path, which is how it used to work in four separate places.
    pub game_dir: String,
    /// `--mesh` / `--skel`: inspect a mesh + rig from LOOSE FILES instead of the install. Both or
    /// neither. `None` (the normal case) means the startup character is assembled out of the
    /// megapack — see `boot.rs`.
    pub mesh: Option<String>,
    pub skel: Option<String>,
    /// `--index`: a generated `anim_bone_map.json`. `None` means the clip catalog is read from
    /// `Animations.pack` itself, which is where that file's contents came from.
    pub index: Option<String>,
    pub pack: String,
    /// Megapack the character's DTEX texture bundles live in (in-app texture resolution).
    pub megapack: String,
    /// Case-insensitive DTEX-name token identifying the character's textures (e.g. "SeanDevlinn").
    pub char_token: String,
    /// Case-insensitive mesh-name token choosing which character to boot with (`--boot`).
    pub boot_model: String,
    /// The shared palette archive — props/vehicles keep their skins here rather than beside the mesh.
    pub palettes: String,
    /// WSAO material library (`France.materials`) — the engine's material hash → texture binding.
    /// Defaults to the one in `game_dir`; `--wsao` overrides it.
    pub wsao: Option<String>,
    /// Directory of loose `<hash>.dtex` files (from `sab_poc mattias`) to load WSAO-resolved textures from.
    pub dtex_dir: Option<String>,
    /// User settings (install location, default language, output slot, type scale).
    pub settings: crate::settings::Settings,
}

impl Config {
    /// Build a config from persisted settings, deriving every game-side path from the one root.
    ///
    /// **Everything the app needs comes from the install.** There used to be three more defaults
    /// here, pointing into this repo's `output/` directory — a merged SMSH, a `.skel` and
    /// `anim_bone_map.json` — and the app refused to start without them, which meant a released
    /// build did not run at all: it demanded build artefacts of a checkout the user does not have.
    /// All three were re-encodings of bytes in `Dynamic0.megapack` / `Animations.pack`, so they are
    /// read from there now (`boot.rs`, `anim_index::from_pack`) and the flags are pure overrides.
    pub fn from_settings(s: crate::settings::Settings) -> Config {
        Config {
            game_dir: s.game_dir.clone(),
            mesh: None,
            skel: None,
            index: None,
            pack: s.anim_pack(),
            megapack: s.megapack(),
            char_token: "SeanDevlinn".into(),
            boot_model: "SeanDevlin".into(),
            palettes: s.palettes(),
            wsao: Some(s.wsao()),
            dtex_dir: None,
            settings: s,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::from_settings(crate::settings::Settings::load())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }
    let get = |flag: &str| {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
    };
    // Settings supply the defaults; the flags below override for a one-off run WITHOUT persisting,
    // so scripting against another install never rewrites what the user chose in the UI.
    let mut settings = crate::settings::Settings::load();
    if let Some(v) = get("--game") {
        settings.game_dir = v;
        settings.clamp();
    }
    let mut cfg = Config::from_settings(settings);
    if let Some(v) = get("--mesh") { cfg.mesh = Some(v); }
    if let Some(v) = get("--skel") { cfg.skel = Some(v); }
    if let Some(v) = get("--index") { cfg.index = Some(v); }
    if let Some(v) = get("--pack") { cfg.pack = v; }
    if let Some(v) = get("--megapack") { cfg.megapack = v; }
    if let Some(v) = get("--char") { cfg.char_token = v; }
    if let Some(v) = get("--boot") { cfg.boot_model = v; }
    if let Some(v) = get("--palettes") { cfg.palettes = v; }
    if let Some(v) = get("--wsao") { cfg.wsao = Some(v); }
    if let Some(v) = get("--dtexdir") { cfg.dtex_dir = Some(v); }

    // Headless verification of the load -> decode -> skin path (no window). Optional clip index
    // into the playable list: `--selftest [N]`.
    if let Some(i) = args.iter().position(|a| a == "--anim-sweep") {
        let n: usize = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(200);
        std::process::exit(app::anim_sweep(cfg, n));
    }
    if let Some(i) = args.iter().position(|a| a == "--selftest") {
        let n: usize = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        std::process::exit(app::selftest(cfg, n));
    }
    // Headless texture-binding verification: `--texcheck [nameSubstr]` (default all assets).
    if let Some(i) = args.iter().position(|a| a == "--texcheck") {
        let filter = args.get(i + 1).filter(|s| !s.starts_with("--")).cloned().unwrap_or_default();
        std::process::exit(app::texcheck(cfg, &filter));
    }

    app::run(cfg);
}

fn print_help() {
    println!(
        "sab_workshop — The Saboteur character/animation viewer (wgpu + egui)\n\n\
         USAGE:\n  sab_workshop [OPTIONS]\n\n\
         The game install is the ONLY thing this needs. Everything below is an override.\n\n\
         OPTIONS:\n\
         \x20 --game  <dir>    game install root   (default: the saved setting, else auto-detected)\n\
         \x20 --boot  <token>  character to open with, by mesh name (default: SeanDevlin)\n\
         \x20 --mesh  <path>   inspect a loose SMSH instead of the install (needs --skel)\n\
         \x20 --skel  <path>   the rig for --mesh (flat .skel text)\n\
         \x20 --index <path>   a generated anim_bone_map.json (default: read Animations.pack)\n\
         \x20 --pack  <path>   Animations.pack     (default: <game>/Animations.pack)\n\
         \x20 --help           show this help\n\n\
         PATHS:\n\
         \x20 <game>   from settings.json, or auto-detected (GOG / Galaxy / Steam / Pandemic layouts)\n\n\
         CONTROLS (in the window):\n\
         \x20 LMB drag  orbit    |  MMB drag  pan    |  wheel  zoom\n\
         \x20 left panel: search + click a clip to play; bottom panel: play/pause, loop, speed, scrubber"
    );
}
