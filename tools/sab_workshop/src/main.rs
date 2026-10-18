//! sab_workshop — a native wgpu + egui workbench for The Saboteur's assets, the same for every build
//! it reads (retail PC and the 2008 Xbox 360 prototype).
//!
//! Assembles models out of the game's own packs (mesh + rig + textures) from its templates, lists
//! every animation clip in a searchable panel and plays the selected one on the loaded rig, exports
//! a lossless bundle of what is on screen, and browses (retail: edits) the game's text, templates and
//! textures. A **game folder is the only input** — no generated files, no repo checkout.
//!
//! Run:
//!   cargo run -p sab_workshop --release
//!   cargo run -p sab_workshop --release -- --game <build folder> --boot Mattias
//!   cargo run -p sab_workshop --release -- --mesh <smsh> --skel <skel>   # inspect loose files
//!   cargo run -p sab_workshop --release -- --help

mod anim_index;
mod app;
mod assets;
mod bone_names;
mod boot;
mod bundle;
mod camera;
mod clip;
mod dtex;
mod editor;
mod formats;
mod gui;
mod havok;
mod meshload;
mod models;
mod normalmap;
mod pack;
mod render;
mod resolve;
mod settings;
mod skinning;
mod source;
mod sources;
#[cfg(all(test, feature = "retail"))]
mod testenv;
mod wsao;

use crate::source::Overrides;

/// Resolved inputs (CLI overrides the persisted settings).
#[derive(Clone)]
pub struct Config {
    /// The build folder. Everything the app reads comes from the source opened on it.
    pub game_dir: String,
    /// `--mesh` / `--skel`: inspect a mesh (and rig) from LOOSE FILES instead of the build's player.
    /// `--mesh` alone is rigged on the player's rig — see `boot.rs`.
    pub mesh: Option<String>,
    pub skel: Option<String>,
    /// `--boot`: a case-insensitive asset-name token choosing the startup model; empty is the build's
    /// player.
    pub boot_model: String,
    /// Retail files the flags put in place of the ones under `game_dir`.
    pub overrides: Overrides,
    /// User settings (install location, default language, output slot, type scale).
    pub settings: crate::settings::Settings,
}

impl Config {
    pub fn from_settings(s: crate::settings::Settings) -> Config {
        Config { game_dir: s.game_dir.clone(), mesh: None, skel: None, boot_model: String::new(), overrides: Overrides::default(), settings: s }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }
    let get = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    // Settings supply the defaults; the flags below override for a one-off run WITHOUT persisting,
    // so scripting against another install never rewrites what the user chose in the UI.
    let mut settings = crate::settings::Settings::load();
    if let Some(v) = get("--game") {
        settings.game_dir = v;
        settings.clamp();
    }
    let mut cfg = Config::from_settings(settings);
    cfg.mesh = get("--mesh");
    cfg.skel = get("--skel");
    if let Some(v) = get("--boot") {
        cfg.boot_model = v;
    }
    cfg.overrides = Overrides {
        megapack: get("--megapack"),
        palettes: get("--palettes"),
        wsao: get("--wsao"),
        anim_pack: get("--pack"),
        anim_index: get("--index"),
        dtex_dir: get("--dtexdir"),
    };

    // Headless sweep of clips for insane poses: `--anim-sweep [N]`.
    if let Some(i) = args.iter().position(|a| a == "--anim-sweep") {
        let n: usize = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(200);
        std::process::exit(app::anim_sweep(cfg, n));
    }
    // Headless load -> decode -> skin check: `--selftest [N]` (N-th clip playable on the player).
    if let Some(i) = args.iter().position(|a| a == "--selftest") {
        let n: usize = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
        std::process::exit(app::selftest(cfg, n));
    }
    // Headless texture-binding check: `--texcheck [nameSubstr]` (default all assets).
    if let Some(i) = args.iter().position(|a| a == "--texcheck") {
        let filter = args.get(i + 1).filter(|s| !s.starts_with("--")).cloned().unwrap_or_default();
        std::process::exit(app::texcheck(cfg, &filter));
    }
    // Fingerprint a folder as a game source and print what was recognized.
    if let Some(i) = args.iter().position(|a| a == "--source-info") {
        let dir = args.get(i + 1).cloned().unwrap_or(cfg.game_dir.clone());
        std::process::exit(source_info(&dir, &cfg.overrides));
    }
    // Headless bundle export: `--export <asset> <out dir>`.
    if let Some(i) = args.iter().position(|a| a == "--export") {
        let (Some(name), Some(out)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("usage: sab_workshop --game <build> --export <asset name> <out dir>");
            std::process::exit(2);
        };
        std::process::exit(app::export(cfg, name, out));
    }

    app::run(cfg);
}

fn source_info(dir: &str, overrides: &Overrides) -> i32 {
    let path = std::path::Path::new(dir);
    println!("Fingerprinting: {}\n", path.display());
    match source::detect_version(path) {
        None => {
            println!("  ✗ no known Saboteur build detected");
            println!("  (recognized: Retail PC [GOG/Steam], Prototype 2008 Xbox 360)");
            1
        }
        Some((v, fp)) => {
            println!("  ✓ detected: {} ({v:?})", v.label());
            println!();
            println!("  matched:");
            for m in &fp.matched {
                println!("    · {m}");
            }
            if !fp.warnings.is_empty() {
                println!();
                println!("  warnings:");
                for w in &fp.warnings {
                    println!("    ! {w}");
                }
            }
            println!();
            match source::open_source(path, overrides) {
                Ok(src) => {
                    println!("  opened as: {}", src.label());
                    println!("    meshes: {}", src.meshes().len());
                    println!("    world blocks: {}", src.world_blocks().len());
                    println!("    clips: {}", src.clips().len());
                    println!("    game-text languages: {}", src.game_text_files().len());
                    println!();
                    println!("  post-open fingerprint (final):");
                    for m in &src.fingerprint().matched {
                        println!("    · {m}");
                    }
                    for w in &src.fingerprint().warnings {
                        println!("    ! {w}");
                    }
                    0
                }
                Err(e) => {
                    println!("  ✗ open failed: {e}");
                    2
                }
            }
        }
    }
}

fn print_help() {
    println!(
        "sab_workshop — The Saboteur asset workbench (wgpu + egui), retail PC and the 2008 prototype\n\n\
         USAGE:\n  sab_workshop [OPTIONS]\n\n\
         The game folder is the ONLY thing this needs. Everything below is an override.\n\n\
         OPTIONS:\n\
         \x20 --game  <dir>    the build folder    (default: the saved setting; set it in the app's Settings page)\n\
         \x20 --boot  <token>  asset to open with, by name (default: the build's player)\n\
         \x20 --mesh  <path>   inspect a loose SMSH; rigged on --skel, else on the player's rig\n\
         \x20 --skel  <path>   the rig for --mesh (flat .skel text)\n\
         \x20 --export <asset> <out>  write the asset's bundle (model.glb, textures, raw records, manifest)\n\
         \x20 --source-info <dir>  fingerprint a folder as a game source and print what was recognized\n\
         \x20 --selftest [N]   headless load, clip and skinning check\n\
         \x20 --texcheck [s]   headless texture-binding report for assets whose name contains s\n\
         \x20 --anim-sweep [N] headless check of N clips for insane poses\n\
         \x20 --help           show this help\n\n\
         RETAIL FILE OVERRIDES:\n\
         \x20 --megapack <path>  in place of Global/Dynamic0.megapack\n\
         \x20 --palettes <path>  in place of Global/Palettes0.megapack\n\
         \x20 --wsao <path>      in place of France.materials\n\
         \x20 --pack <path>      in place of Animations.pack\n\
         \x20 --index <path>     a generated anim_bone_map.json in place of Animations.pack's ANIM block\n\
         \x20 --dtexdir <dir>    loose <hash>.dtex textures, consulted before the packs\n\n\
         CONTROLS (in the window):\n\
         \x20 LMB drag  orbit    |  MMB drag  pan    |  wheel  zoom\n\
         \x20 left panel: search + click a model or clip; bottom panel: play/pause, loop, speed, scrubber"
    );
}
