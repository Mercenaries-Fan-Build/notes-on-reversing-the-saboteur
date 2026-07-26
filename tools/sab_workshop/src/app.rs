//! Application: load assets, run the winit event loop, draw the UI, drive playback.
//!
//! Data flow:
//!   startup        load SMSH + .skel + anim_bone_map.json + Animations.pack (parse AP0L/packfile
//!                  once -> list of spline-anim offsets `scas`); compute bind-pose joint matrices.
//!   select a clip  re-parse the packfile, `read_spline_anim(scas[clip.index])`, keep the
//!                  self-contained `SplineAnim` + its per-track bone map.
//!   each frame     `sample_at(time)` -> per-track local transforms -> `skinning::posed` -> joint
//!                  matrices uploaded to the GPU; the vertex shader skins the mesh.

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowBuilder;

use crate::anim_index::{self, AnimCatalog};
use crate::bundle;
use crate::camera::OrbitCamera;
use crate::formats::{self, Bone, Smsh, SubMesh};
use crate::gui::theme;
use crate::havok::{self, Packfile, SplineAnim};
use crate::meshload;
use crate::pack;
use crate::render::Renderer;
use crate::resolve;
use crate::skinning;
use crate::Config;

/// The animation pack, parsed once: raw bytes + the ordered spline-anim object offsets.
struct PackData {
    file: Vec<u8>,
    blob_off: usize,
    hk_size: usize,
    scas: Vec<usize>, // data-relative offset of the N-th hkaSplineCompressedAnimation
}

impl PackData {
    fn load(path: &str) -> Result<PackData, String> {
        let file = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        let ap = havok::parse_ap0l(&file)?;
        let blob = &file[ap.blob_off..ap.blob_off + ap.hk_size];
        let pk = Packfile::parse(blob)?;
        let scas: Vec<usize> = pk
            .vfixups
            .iter()
            .filter(|(_, c)| c == "hkaSplineCompressedAnimation")
            .map(|(s, _)| *s)
            .collect();
        Ok(PackData { file, blob_off: ap.blob_off, hk_size: ap.hk_size, scas })
    }
    fn blob(&self) -> &[u8] {
        &self.file[self.blob_off..self.blob_off + self.hk_size]
    }
}

/// A clip decoded and ready to sample.
struct LoadedClip {
    anim: SplineAnim,
    track_to_bone: Vec<i32>,
    name: String,
    duration: f32,
    frame_duration: f32,
    frame_count: usize,
}

/// Playback state.
struct Playback {
    time: f32,
    playing: bool,
    looping: bool,
    speed: f32,
}

pub fn run(cfg: Config) {
    let mut errors: Vec<String> = Vec::new();

    // --- the startup model: assembled from the install, or loaded from --mesh/--skel ---
    let boot = match crate::boot::load(&cfg) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: {e}");
            if cfg.game_dir.is_empty() {
                eprintln!(
                    "[sab_workshop] No game install was found. Pass --game <dir> (the folder holding \
                     Global/, Animations.pack, France.materials)."
                );
            } else {
                eprintln!("[sab_workshop] Install checked: {}", cfg.game_dir);
            }
            return;
        }
    };
    let boot_parts = boot.parts;
    let boot_ranges = boot.part_ranges;
    let from_pack = !boot_parts.is_empty();
    let mesh: Smsh = boot.mesh;
    let mut skel: Vec<Bone> = boot.bones;
    println!(
        "[sab_workshop] mesh: {} verts, {} tris | skeleton: {} bones",
        mesh.positions.len(),
        mesh.indices.len() / 3,
        skel.len()
    );

    // --- bounding sphere for the camera ---
    let (center, radius) = bounds(&mesh);

    // --- window + renderer ---
    let event_loop = match EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: event loop: {e}");
            return;
        }
    };
    let window = match WindowBuilder::new()
        .with_title("sab_workshop — The Saboteur character viewer")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
    {
        Ok(w) => Arc::new(w),
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: window: {e}");
            return;
        }
    };
    let mut renderer = match pollster::block_on(Renderer::new(window.clone(), &mesh, skel.len())) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: renderer init: {e}");
            return;
        }
    };
    let mut gui = crate::gui::Gui::new(renderer.device(), renderer.surface_format(), &window);

    // --- initial GPU state: bind pose ---
    let bind = skinning::bind_pose(&skel);
    renderer.update_joints(&bind);

    let mut mesh_stats = (mesh.positions.len(), mesh.indices.len() / 3, skel.len());
    let mut submeshes = renderer.submeshes().to_vec();
    // The clip catalog's track→bone indices are authored against the startup rig; a model with a
    // different bone count cannot use them.
    let rig_bones = skel.len();
    let mut model_name = boot.name;
    // Where this model's material assignments persist. A `--mesh` file keeps its sidecar beside
    // itself (that is the port workflow's record); a pack model has no file to sit next to, so it
    // goes in the user's config dir keyed by model name — which also stops every clicked model from
    // overwriting one shared sidecar, as it used to.
    let mut tex_profile =
        cfg.mesh.clone().unwrap_or_else(|| resolve::profile_path(&model_name));

    // --- background asset load ---
    // The megapacks (~1.1 GB) + Animations.pack + texture resolve are slow; load them OFF the main
    // thread so the window is interactive immediately (the character shows in bind pose, the editor
    // pages work at once). Results stream back and are applied in RedrawRequested; wgpu uploads stay
    // on this thread. The worker gets clones of the config + submeshes it needs.
    let n_sub = submeshes.len();
    let mut catalog = AnimCatalog { clips: Vec::new(), skeleton_bones: skel.len(), num_main_clips: 0 };
    let mut playable_rows: Vec<usize> = Vec::new();
    let mut pack: Option<PackData> = None;
    let mut megapack: Option<pack::Megapack> = None;
    let mut palettes: Option<pack::Megapack> = None;
    let mut mesh_list: Vec<meshload::MeshEntry> = Vec::new();
    let mut assets: Vec<resolve::TexAsset> = Vec::new();
    let mut submesh_tex: Vec<Option<usize>> = vec![None; n_sub];
    let mut submesh_prov: Vec<resolve::Prov> = vec![resolve::Prov::Unresolved; n_sub];
    let (bg_tx, bg_rx) = std::sync::mpsc::channel::<BgMsg>();
    {
        let cfgw = cfg.clone();
        let subw = submeshes.clone();
        // A pack-assembled boot model is textured through the engine's own material table on the
        // click path below, so the worker must not ALSO run the legacy name-token seed over it —
        // that path exists for `--mesh` files, which have no material bindings to look up.
        std::thread::spawn(move || background_load(cfgw, subw, rig_bones, !from_pack, bg_tx));
    }


    // --- interactive state (all locals, captured by the move closure) ---
    let mut camera = OrbitCamera::framing(center, radius);
    let mut show_all = false; // include non-playable clips in the list
    let mut current: Option<LoadedClip> = None;
    let mut playback = Playback { time: 0.0, playing: true, looping: true, speed: 1.0 };
    let mut pending_load: Option<usize> = None; // catalog.clips index requested this frame
    // Materials picker request: (submesh, Some(asset) | None to unassign), applied after the UI runs.
    let mut pending_tex: Option<(usize, Option<usize>)> = None;
    let mut pending_save = false;
    // Navigator: which category is shown, and a click-to-load request (index into `mesh_list`).
    // Root motion walks the character out of a fixed-camera preview; lock it by default.
    let mut lock_root = true;
    let mut nav_tab = NavTab::Models;
    let mut page = Page::Inspect;
    // The mod-editor pages (Templates / GameText / Icons). Every game-side path comes from the one
    // configured install root (Settings page), not from unwinding a megapack path.
    let game_dir = cfg.game_dir.clone();
    let mut editor_state =
        crate::editor::Editor::new(&game_dir, cfg.settings.lang, &cfg.settings.dlc_slot, &cfg.settings.mod_name);
    // The Settings page edits a COPY; the running app keeps using what it booted with until restart.
    let mut settings_ctx = SettingsCtx {
        dir_edit: cfg.settings.game_dir.clone(),
        launched_with: cfg.settings.game_dir.clone(),
        s: cfg.settings.clone(),
        note: None,
    };
    // Model-browser groupings (rule-based + user overrides saved next to the game) and the
    // right-click "move to group" request applied after each UI pass.
    let mut groups = crate::models::Groups::load(&game_dir);
    let mut pending_group: Option<(usize, String)> = None;
    // True until the ModelList stage streams in — the browser shows a spinner meanwhile.
    let mut models_loading = true;
    // The assembled assets (from GameTemplates) the browser lists, and the one selected (whose parts
    // the inspector shows).
    let mut browse_assets: Vec<crate::assets::Asset> = Vec::new();
    let mut sel_asset: Option<usize> = None;
    // Background texture resolve for a clicked model: a monotonic request id (stale results from a
    // superseded click are discarded) and the result channel.
    let mut model_req: u64 = 0;
    let (mtex_tx, mtex_rx) = std::sync::mpsc::channel::<ModelTex>();
    // The boot model gets the SAME engine-faithful texture resolve a click does — it is the same kind
    // of model, assembled the same way — so kick it off now that the submesh cover exists.
    if from_pack {
        model_req += 1;
        spawn_tex_resolve(
            &cfg,
            &boot_parts,
            &boot_ranges,
            &submeshes,
            model_req,
            mtex_tx.clone(),
        );
    }
    let mut sel_submesh: Option<usize> = None;
    let mut sel_bone: Option<usize> = None;
    let mut sel_tex: Option<usize> = None;
    let mut thumbs = Thumbs::default();
    let mut bone_depth: Vec<usize> = bone_depths(&skel);
    // Every part of the clicked asset — a character is assembled, not shown one piece at a time.
    let mut pending_model: Option<Vec<usize>> = None;
    let mut nav_search = String::new();
    // Outcome of the last click-to-load, surfaced in the status bar: (message, is_error).
    let mut load_status: Option<(String, bool)> = None;
    // The parts of the currently-viewed character, for bundle export (re-assembled on demand).
    //
    // SEEDED FROM THE BOOT MODEL. This only ever filled on a browser click, so the character the app
    // opens on — the one you are looking at before you touch anything — counted as "nothing loaded"
    // and could not be exported until you clicked something else. The boot parts come from the same
    // `cfg.megapack` the exporter reopens, so their offsets are valid there.
    let mut current_parts: Vec<meshload::MeshEntry> = boot_parts.clone();
    // In-flight background bundle export, polled once per frame.
    let mut exporter: Option<Exporter> = None;
    // Is a texture resolve still in flight for the current model? An export started before it lands
    // would write a bundle with an empty texture pool, so the verb bar says so and the deferred
    // "load & export" waits for it.
    let mut tex_pending = from_pack;
    // A context-menu export that must wait for the click-to-load (and its texture resolve) first.
    let mut export_when_ready = false;
    let mut last_frame = Instant::now();

    // mouse
    let mut mouse: (f32, f32) = (0.0, 0.0);
    let mut lmb = false;
    let mut mmb = false;

    let result = event_loop.run(move |event, elwt| match event {
        Event::AboutToWait => window.request_redraw(),
        Event::WindowEvent { window_id, event } if window_id == window.id() => {
            let gui_took = gui.on_event(&event);
            match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == ElementState::Pressed && !gui.ctx.wants_keyboard_input() {
                        match event.physical_key {
                            PhysicalKey::Code(KeyCode::Escape) => elwt.exit(),
                            PhysicalKey::Code(KeyCode::Space) => playback.playing = !playback.playing,
                            _ => {}
                        }
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    let pressed = state == ElementState::Pressed;
                    match button {
                        MouseButton::Left => lmb = pressed && !gui_took,
                        MouseButton::Middle => mmb = pressed && !gui_took,
                        _ => {}
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let (nx, ny) = (position.x as f32, position.y as f32);
                    let (dx, dy) = (nx - mouse.0, ny - mouse.1);
                    mouse = (nx, ny);
                    if !gui_took {
                        if lmb {
                            camera.rotate(dx, dy);
                        } else if mmb {
                            camera.pan(dx, dy);
                        }
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if !gui_took {
                        let s = match delta {
                            MouseScrollDelta::LineDelta(_, y) => y,
                            MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                        };
                        camera.zoom(s);
                    }
                }
                WindowEvent::RedrawRequested => {
                    // Set by the verb bar, F10, or a texture resolve landing for a deferred
                    // context-menu export. Declared up here because the texture drain below can
                    // raise it before the UI runs.
                    let mut export_req = false;
                    // Drain whatever background stages have arrived and paint them (progressive fill).
                    while let Ok(msg) = bg_rx.try_recv() {
                        match msg {
                            BgMsg::Catalog { catalog: c, playable_rows: pr } => {
                                catalog = c;
                                playable_rows = pr;
                            }
                            BgMsg::ModelList { mesh_list: ml, assets: a } => {
                                mesh_list = ml;
                                browse_assets = a;
                                models_loading = false;
                            }
                            BgMsg::Textures { assets: a, submesh_tex: st, submesh_prov: sp, decoded } => {
                                assets = a;
                                submesh_tex = st;
                                submesh_prov = sp;
                                for (i, tex) in &decoded {
                                    renderer.set_submesh_texture(*i, tex);
                                }
                            }
                            BgMsg::Packs { megapack: mp, palettes: pal } => {
                                megapack = mp;
                                palettes = pal;
                            }
                            BgMsg::AnimPack(p) => pack = p,
                        }
                    }
                    // Apply a model's textures once the worker resolves them (newest request wins).
                    while let Ok(mt) = mtex_rx.try_recv() {
                        if mt.req == model_req {
                            assets = mt.assets;
                            submesh_tex = mt.submesh_tex;
                            submesh_prov = mt.submesh_prov;
                            for (i, tex) in &mt.decoded {
                                renderer.set_submesh_texture(*i, tex);
                            }
                            // A hand-authored sidecar is a DECISION and outranks what the material
                            // table answered — otherwise the picker's Save button did nothing the
                            // moment the model reloaded.
                            if let Some((st, sp)) =
                                resolve::load_sidecar(&tex_profile, submeshes.len(), &assets)
                            {
                                for i in 0..submeshes.len() {
                                    if sp[i] == resolve::Prov::Unresolved {
                                        continue; // the sidecar says nothing about this submesh
                                    }
                                    submesh_tex[i] = st[i];
                                    submesh_prov[i] = sp[i];
                                    match st[i].map(|ai| assets[ai].decode()) {
                                        Some(Ok(tex)) => renderer.set_submesh_texture(i, &tex),
                                        Some(Err(e)) => eprintln!("[sab_workshop] decode: {e}"),
                                        None => renderer.clear_submesh_texture(i),
                                    }
                                }
                            }
                            // The bindings are now what the bundle would write, so a deferred
                            // "load & export" can finally fire.
                            tex_pending = false;
                            if export_when_ready {
                                export_when_ready = false;
                                export_req = true;
                            }
                        }
                    }

                    // --- advance playback ---
                    let now = Instant::now();
                    let dt = (now - last_frame).as_secs_f32().min(0.1);
                    last_frame = now;
                    if let Some(clip) = &current {
                        if playback.playing && clip.duration > 0.0 {
                            playback.time += dt * playback.speed;
                            if playback.looping {
                                playback.time = playback.time.rem_euclid(clip.duration);
                            } else if playback.time > clip.duration {
                                playback.time = clip.duration;
                                playback.playing = false;
                            }
                        }
                    }

                    // --- pose -> joint matrices ---
                    if let (Some(clip), Some(pk)) = (&current, &pack) {
                        let pose = clip.anim.sample_at(pk.blob(), playback.time);
                        let mats = skinning::posed(&skel, &pose, &clip.track_to_bone, lock_root);
                        renderer.update_joints(&mats);
                    }

                    renderer.update_camera(camera.view_proj(renderer.aspect()));

                    // --- UI ---
                    let rig_ok = skel.len() == rig_bones;
                    let mut pick_export_dir = false;
                    let mut export_ctx = ExportCtx {
                        request: &mut export_req,
                        pick_dir: &mut pick_export_dir,
                        busy: exporter.as_ref().map(|e| e.label.clone()),
                        // Export what is on screen: the bundle carries the texture bindings, and
                        // those only exist for the model that is actually loaded.
                        ready: megapack.is_some() && !current_parts.is_empty(),
                        tex_pending,
                        dest: bundle_root(&settings_ctx.s).join(&model_name).display().to_string(),
                    };
                    gui.run(|ctx| {
                        build_ui(
                            ctx, &catalog, &playable_rows, &mut show_all,
                            &current, &mut playback, &mut pending_load, &mut renderer.show_grid,
                            &mut renderer.show_textures, &mut lock_root, &errors, pack.is_some(), rig_ok,
                            mesh_stats, &model_name, &load_status, &mut page, &mut thumbs, &mut export_ctx,
                            &mut MatsCtx {
                                submeshes: &submeshes,
                                assets: &assets,
                                submesh_tex: &submesh_tex,
                                submesh_prov: &submesh_prov,
                                selected: &mut sel_submesh,
                                pending_tex: &mut pending_tex,
                                pending_save: &mut pending_save,
                            },
                            &mut NavCtx {
                                tab: &mut nav_tab,
                                search: &mut nav_search,
                                browse_assets: &browse_assets,
                                sel_asset: &mut sel_asset,
                                assets: &assets,
                                pending_model: &mut pending_model,
                                current_model: &model_name,
                                bones: &skel,
                                bone_depth: &bone_depth,
                                selected_bone: &mut sel_bone,
                                selected_tex: &mut sel_tex,
                                groups: &groups,
                                pending_group: &mut pending_group,
                                export_after_load: &mut export_when_ready,
                                models_loading,
                            },
                            &mut editor_state,
                            &mut settings_ctx,
                        );
                    });

                    // --- pick a new bundle destination (verb bar "→ path") ---
                    if pick_export_dir {
                        if let Some(d) = rfd::FileDialog::new()
                            .set_title("Where to write export bundles")
                            .set_directory(bundle_root(&settings_ctx.s))
                            .pick_folder()
                        {
                            settings_ctx.s.export_dir = d.to_string_lossy().replace('\\', "/");
                            if let Err(e) = settings_ctx.s.save() {
                                load_status = Some((format!("settings: {e}"), true));
                            }
                        }
                    }

                    // --- start a bundle export (verb bar / F10 / navigator context menu) ---
                    //
                    // Handing this to a WORKER is the whole point: assembling the character,
                    // BC-decoding its skins to PNG and writing the raw records takes seconds, and
                    // doing it inline painted the window "Not Responding" until it finished.
                    if export_req && exporter.is_none() {
                        if current_parts.is_empty() || megapack.is_none() {
                            load_status = Some(("open a character from the browser first".into(), true));
                        } else {
                            exporter = Some(spawn_bundle_export(bundle::Job {
                                megapack: cfg.megapack.clone(),
                                label: model_name.clone(),
                                parts: current_parts.clone(),
                                submeshes: submeshes.clone(),
                                assets: assets.clone(),
                                submesh_tex: submesh_tex.clone(),
                                submesh_prov: submesh_prov.clone(),
                                outroot: bundle_root(&settings_ctx.s),
                            }));
                        }
                    }

                    // --- poll the export worker ---
                    if let Some(e) = &exporter {
                        match e.rx.try_recv() {
                            Ok(r) => {
                                load_status = Some(match r {
                                    Ok(dir) => (format!("exported → {dir}"), false),
                                    Err(err) => (format!("export: {err}"), true),
                                });
                                exporter = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                load_status = Some(("export worker died".into(), true));
                                exporter = None;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                    }

                    // --- act on a right-click "move to group" (persists the override) ---
                    if let Some((ai, cat)) = pending_group.take() {
                        if let Some(a) = browse_assets.get(ai) {
                            groups.set(&a.name, &cat, a.category);
                        }
                    }

                    // --- act on a navigator model click: swap mesh + rig + textures + camera ---
                    // Don't consume the click until the megapack has streamed in (staged load), else a
                    // click made while the model list is up but the pack isn't yet would be dropped.
                    if megapack.is_some() {
                    if let Some(mis) = pending_model.take() {
                        let picked: Vec<meshload::MeshEntry> =
                            mis.iter().filter_map(|i| mesh_list.get(*i).cloned()).collect();
                        if let (Some(mp), false) = (&megapack, picked.is_empty()) {
                            match meshload::assemble(mp.raw(), &picked) {
                                Ok(lm) => {
                                    current_parts = picked.clone(); // for glTF export
                                    let nbones = lm.bones.len();
                                    renderer.set_mesh(&lm.mesh, nbones);
                                    skel = lm.bones;
                                    renderer.update_joints(&skinning::bind_pose(&skel));
                                    submeshes = renderer.submeshes().to_vec();
                                    mesh_stats =
                                        (lm.mesh.positions.len(), lm.mesh.indices.len() / 3, nbones);
                                    // The model shows untextured immediately; its texture resolve (a
                                    // token scan that can touch the whole pack for a character) runs on
                                    // a WORKER so this click never freezes the UI. Results stream back
                                    // and are applied in the mtex drain below.
                                    assets = Vec::new();
                                    submesh_tex = vec![None; submeshes.len()];
                                    submesh_prov = vec![resolve::Prov::Unresolved; submeshes.len()];
                                    model_req = model_req.wrapping_add(1);
                                    tex_pending = true;
                                    spawn_tex_resolve(
                                        &cfg,
                                        &picked,
                                        &lm.part_ranges,
                                        &submeshes,
                                        model_req,
                                        mtex_tx.clone(),
                                    );
                                    bone_depth = bone_depths(&skel);
                                    sel_submesh = None;
                                    sel_bone = None;
                                    sel_tex = None;
                                    thumbs.clear();
                                    let (c, r) = bounds(&lm.mesh);
                                    camera = OrbitCamera::framing(c, r);
                                    // The clip catalog is authored against the startup rig.
                                    current = None;
                                    playback.time = 0.0;
                                    model_name = lm.name.clone();
                                    // Material picks follow the model, not the app: a clicked model
                                    // came from the pack, so its sidecar is its own, never the
                                    // startup mesh's (which is what it used to overwrite).
                                    tex_profile = resolve::profile_path(&model_name);
                                    load_status = Some((
                                        format!(
                                            "{}: {} tris, {nbones} bones, {} textures",
                                            lm.name,
                                            mesh_stats.1,
                                            assets.len()
                                        ),
                                        false,
                                    ));
                                    println!(
                                        "[sab_workshop] loaded {}: {} verts, {} tris, {nbones} bones, {} submeshes, {} textures",
                                        lm.name,
                                        mesh_stats.0,
                                        mesh_stats.1,
                                        submeshes.len(),
                                        assets.len()
                                    );
                                }
                                Err(e) => {
                                    // Most pack entries are STATIC props; the MESH reader we ported
                                    // from sab_mesh only handles skinned meshes, so say so in the UI
                                    // rather than appearing to ignore the click.
                                    eprintln!("[sab_workshop] assemble: {e}");
                                    load_status = Some((format!("assemble: {e}"), true));
                                    // Disarm a context-menu "Export bundle": there is no texture
                                    // resolve coming, so leaving it armed would fire the export on
                                    // whichever model the user loaded NEXT.
                                    export_when_ready = false;
                                }
                            }
                        }
                    }
                    } // end `if megapack.is_some()`

                    // --- act on a Materials picker change: rebind + persist the sidecar ---
                    if let Some((i, ai)) = pending_tex.take() {
                        match ai {
                            Some(ai) => match assets[ai].decode() {
                                Ok(tex) => renderer.set_submesh_texture(i, &tex),
                                Err(e) => eprintln!("[sab_workshop] decode {}: {e}", assets[ai].name),
                            },
                            None => renderer.clear_submesh_texture(i),
                        }
                        submesh_tex[i] = ai;
                        // A pick is a decision, so it stops being a guess — and unpicking returns
                        // the submesh to occupied rather than to "seeded".
                        submesh_prov[i] =
                            if ai.is_some() { resolve::Prov::Bound } else { resolve::Prov::Unresolved };
                        match resolve::save_sidecar(&tex_profile, &submesh_tex, &submesh_prov, &assets) {
                            Ok(()) => println!(
                                "[sab_workshop] submesh {i} -> {} (saved {})",
                                ai.map(|x| assets[x].name.as_str()).unwrap_or("(none)"),
                                resolve::sidecar_path(&tex_profile)
                            ),
                            Err(e) => eprintln!("[sab_workshop] sidecar save failed: {e}"),
                        }
                    }

                    // --- act on an explicit sidecar write (the stamp) ---
                    if std::mem::take(&mut pending_save) {
                        match resolve::save_sidecar(&tex_profile, &submesh_tex, &submesh_prov, &assets) {
                            Ok(()) => {
                                let n = submesh_prov.iter().filter(|p| **p == resolve::Prov::Bound).count();
                                load_status = Some((
                                    format!("wrote {} ({n} bound)", resolve::sidecar_path(&tex_profile)),
                                    false,
                                ));
                            }
                            Err(e) => {
                                eprintln!("[sab_workshop] sidecar save failed: {e}");
                                load_status = Some((format!("sidecar: {e}"), true));
                            }
                        }
                    }

                    // --- act on a clip request ---
                    if let Some(ci) = pending_load.take() {
                        match load_clip(&pack, &catalog, ci) {
                            Ok(c) => {
                                println!(
                                    "[sab_workshop] loaded clip '{}' ({} tracks, {} frames, {:.3}s)",
                                    c.name, c.anim.num_transform_tracks, c.frame_count, c.duration
                                );
                                current = Some(c);
                                playback.time = 0.0;
                                playback.playing = true;
                            }
                            Err(e) => {
                                eprintln!("[sab_workshop] load clip failed: {e}");
                            }
                        }
                    }

                    // The editor pages own the whole window with opaque panels; skip the scene pass.
                    renderer.draw_scene = page.editor().is_none();
                    match renderer.render(&mut gui) {
                        Ok(()) => {}
                        Err(wgpu::SurfaceError::OutOfMemory) => elwt.exit(),
                        Err(e) => eprintln!("[sab_workshop] surface error: {e:?}"),
                    }
                }
                _ => {}
            }
        }
        _ => {}
    });
    if let Err(e) = result {
        eprintln!("[sab_workshop] event loop error: {e}");
    }
}

/// One stage of the background load, delivered to the main thread as soon as it is ready so the UI
/// fills in progressively instead of waiting for everything. All payloads are owned (Send). `decoded`
/// textures are uploaded on the main thread (wgpu is main-thread only).
enum BgMsg {
    /// The clip catalog (anim_bone_map.json) — the clip list. Cheap; arrives first.
    Catalog { catalog: AnimCatalog, playable_rows: Vec<usize> },
    /// The flat mesh list (for the loader) + the assembled assets built from GameTemplates (what the
    /// browser shows). Arrives after the mmap chain-walk + the GameTemplates parse.
    ModelList { mesh_list: Vec<meshload::MeshEntry>, assets: Vec<crate::assets::Asset> },
    /// The current character's resolved textures + which submesh each is on + decoded pixels.
    Textures {
        assets: Vec<resolve::TexAsset>,
        submesh_tex: Vec<Option<usize>>,
        submesh_prov: Vec<resolve::Prov>,
        decoded: Vec<(usize, crate::dtex::CpuTexture)>,
    },
    /// The mmapped megapacks themselves — hands click-to-load model switching to the main thread.
    Packs { megapack: Option<pack::Megapack>, palettes: Option<pack::Megapack> },
    /// Animations.pack (187 MB) — needed only to PLAY a clip, so it is loaded last.
    AnimPack(Option<PackData>),
}

/// Resolve a pack model's textures on a WORKER, so neither startup nor a click ever freezes the UI
/// (the resolve can touch the whole 715 MB pack). `req` tags the request; a stale result from a
/// superseded one is discarded on the main thread.
///
/// Used for the boot model AND for every click — they are the same kind of model, assembled the same
/// way, so they must be textured by the same rules (the engine's material table).
fn spawn_tex_resolve(
    cfg: &Config,
    parts: &[meshload::MeshEntry],
    part_ranges: &[(String, u32, u32)],
    submeshes: &[SubMesh],
    req: u64,
    tx: std::sync::mpsc::Sender<ModelTex>,
) {
    // Every part, with the slice of the index buffer it owns.
    let pinfo: Vec<(String, usize, u32, u32)> = part_ranges
        .iter()
        .map(|(n, i0, ilen)| {
            let off = parts.iter().find(|e| &e.name == n).map(|e| e.file_off).unwrap_or(0);
            (n.clone(), off, *i0, *ilen)
        })
        .collect();
    let starts: Vec<u32> = submeshes.iter().map(|s| s.index_start).collect();
    // materials[0] per submesh — the drawcall material hash WSAO keys on; None where a range carries
    // no material.
    let mats: Vec<Option<u32>> = submeshes.iter().map(|s| s.materials.first().copied()).collect();
    let cfgw = cfg.clone();
    std::thread::spawn(move || {
        if let Some(mt) = resolve_model_textures(cfgw, pinfo, starts, mats, req) {
            let _ = tx.send(mt);
        }
    });
}

/// Textures resolved (on a worker) for a model the user clicked in the browser. `req` tags the click
/// so a stale result from a superseded click is discarded on the main thread.
struct ModelTex {
    req: u64,
    assets: Vec<resolve::TexAsset>,
    submesh_tex: Vec<Option<usize>>,
    submesh_prov: Vec<resolve::Prov>,
    decoded: Vec<(usize, crate::dtex::CpuTexture)>,
}

/// The last two components of a path — `…/workshop_export/CH_AL_SeanDevlin_01`.
///
/// Enough to recognise where a bundle lands; the absolute path belongs in a tooltip, not in a bar
/// whose job is to show a button.
fn short_path(p: &str) -> String {
    let norm = p.replace('\\', "/");
    let tail: Vec<&str> = norm.rsplit('/').take(2).collect();
    match tail.as_slice() {
        [last, parent] => format!("…/{parent}/{last}"),
        _ => p.to_string(),
    }
}

/// Handle to an in-flight background bundle export (poll `rx` once per frame).
struct Exporter {
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
    /// The asset being exported — for the progress card and the completion message.
    label: String,
}

/// Where export bundles land: the configured directory, else `workshop_export/` beside wherever the
/// app was launched.
///
/// NOT a hardcoded absolute path — a machine path baked into the binary is dead for everyone whose
/// copy lives elsewhere, which is the same failure `Settings::game_dir` exists to fix.
fn bundle_root(s: &crate::settings::Settings) -> std::path::PathBuf {
    if s.export_dir.is_empty() {
        std::env::current_dir().unwrap_or_default().join("workshop_export")
    } else {
        std::path::PathBuf::from(&s.export_dir)
    }
}

/// Run a bundle export off the UI thread. The job owns everything it needs and opens its OWN
/// megapack mmap, so it touches neither the app's pack handle nor the GPU.
fn spawn_bundle_export(job: bundle::Job) -> Exporter {
    let (tx, rx) = std::sync::mpsc::channel();
    let label = job.label.clone();
    std::thread::spawn(move || {
        let _ = tx.send(bundle::run(&job));
    });
    Exporter { rx, label }
}

/// Resolve + BC-decode a clicked model's textures OFF the main thread. Reopens the megapacks (mmap is
/// cheap) and binds each submesh the engine's way — `materials[0]` → WSMA record → WSTX slot 0
/// (colour) → DTEX-by-name-hash — via `France.materials`. The name-suffix heuristic is kept only as a
/// fallback for when WSAO can't answer (table missing, or a submesh with no material). Returns `None`
/// if the pack can't be opened.
fn resolve_model_textures(
    cfg: Config,
    parts: Vec<(String, usize, u32, u32)>, // (part name, file_off, index_start, index_count)
    submesh_starts: Vec<u32>,              // each submesh's index_start, to attribute it to a part
    submesh_mats: Vec<Option<u32>>,        // materials[0] per submesh — the WSAO material hash
    req: u64,
) -> Option<ModelTex> {
    use std::collections::{HashMap, HashSet};

    let mp = pack::Megapack::open(&cfg.megapack).ok()?;
    let palettes = pack::Megapack::open(&cfg.palettes).ok();
    let mut packs: Vec<&pack::Megapack> = vec![&mp];
    if let Some(p) = &palettes {
        packs.push(p);
    }

    let nsub = submesh_starts.len();
    let mut assets: Vec<resolve::TexAsset> = Vec::new();
    let mut submesh_tex: Vec<Option<usize>> = vec![None; nsub];
    let mut submesh_prov: Vec<resolve::Prov> = vec![resolve::Prov::Unresolved; nsub];

    // ── primary: WSAO, the engine's own material→texture binding ──
    // France.materials is a loose file at the GAME ROOT (parent of the "Global" dir the megapack is
    // in). `cfg.wsao` overrides the derived path when set.
    let wsao_path = cfg.wsao.clone().or_else(|| {
        std::path::Path::new(&cfg.megapack)
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("France.materials").to_string_lossy().replace('\\', "/"))
    });
    let wsao = wsao_path.as_deref().and_then(|p| crate::wsao::Wsao::open(p).ok());

    if let Some(w) = &wsao {
        // 1. material hash → colour texture name-hash (WSTX slot 0), per submesh. A `numTex == 0`
        //    material is a container range with no textures — `textures()` yields an empty slice, so
        //    `.first()` is None and that submesh is left for the fallback (or unresolved).
        let want: Vec<Option<u32>> = submesh_mats
            .iter()
            .map(|m| m.and_then(|mat| w.textures(mat).and_then(|t| t.first().copied())))
            .collect();
        let mut needed: HashSet<u32> = want.iter().flatten().copied().collect();

        // 2. resolve those name-hashes to DTEX records: the parts' OWN bundles first (a character's
        //    skins usually co-locate), then a whole-pack sweep for the ~23% that cross bundles / the
        //    props that live in Palettes0.
        let mut found: HashMap<u32, resolve::TexAsset> = HashMap::new();
        if !needed.is_empty() {
            let own: Vec<Vec<u8>> = parts
                .iter()
                .filter_map(|(_, off, _, _)| mp.entry_containing(*off).map(|e| mp.slice(&e).to_vec()))
                .collect();
            let own_ref: Vec<&[u8]> = own.iter().map(|v| v.as_slice()).collect();
            resolve::take_hashes_from_slices(&own_ref, &mut needed, &mut found);
            if !needed.is_empty() {
                resolve::take_hashes_from_packs(&packs, &mut needed, &mut found);
            }
        }

        // 3. bind each submesh to its colour record. This is the game's decision, so provenance is
        //    Bound, not Seeded.
        let mut idx: HashMap<u32, usize> = HashMap::new();
        for (h, a) in found {
            let ai = assets.len();
            idx.insert(h, ai);
            assets.push(a);
        }
        for (k, wh) in want.iter().enumerate() {
            if let Some(ai) = wh.and_then(|h| idx.get(&h).copied()) {
                submesh_tex[k] = Some(ai);
                submesh_prov[k] = resolve::Prov::Bound;
            }
        }
    }

    // ── fallback: name-suffix heuristic, only for submeshes WSAO left unbound ──
    // Fires when France.materials can't answer (absent, or a submesh with no material / a material
    // not in the table). Resolves per part and seeds only the still-unbound submeshes of that part.
    if submesh_tex.iter().any(|t| t.is_none()) {
        for (name, file_off, i0, ilen) in &parts {
            let mine: Vec<usize> = (0..nsub)
                .filter(|k| {
                    submesh_tex[*k].is_none()
                        && submesh_starts[*k] >= *i0
                        && submesh_starts[*k] < *i0 + *ilen
                })
                .collect();
            if mine.is_empty() {
                continue;
            }
            let bundle =
                mp.entry_containing(*file_off).map(|e| mp.slice(&e).to_vec()).unwrap_or_default();
            let pool = resolve::texture_pool_for(&packs, &resolve::outfit_token(name), &bundle);
            if pool.is_empty() {
                continue;
            }
            let seeded = resolve::autoseed_for_part(name, mine.len(), &pool);
            let base = assets.len();
            assets.extend(pool);
            for (slot, k) in mine.iter().enumerate() {
                if let Some(ai) = seeded.get(slot).copied().flatten() {
                    submesh_tex[*k] = Some(base + ai);
                    submesh_prov[*k] = resolve::Prov::Seeded;
                }
            }
        }
    }

    let mut decoded = Vec::new();
    for (i, slot) in submesh_tex.iter().enumerate() {
        if let Some(ai) = slot {
            if let Ok(t) = assets[*ai].decode() {
                decoded.push((i, t));
            }
        }
    }
    Some(ModelTex { req, assets, submesh_tex, submesh_prov, decoded })
}

/// The clip catalog for a rig of `rig_bones` bones: the game's `Animations.pack`, or the generated
/// `anim_bone_map.json` when `--index` names one.
fn load_catalog(cfg: &Config, rig_bones: usize) -> Result<AnimCatalog, String> {
    match &cfg.index {
        Some(p) => anim_index::load(p),
        None => anim_index::from_pack(&cfg.pack, rig_bones),
    }
}

/// The slow boot work, on a worker thread, streamed in stages via `tx`. Each `send` lets the main
/// thread paint that section immediately. The megapacks are mmapped (instant open); the model list is
/// enumerated from the index (only sub-pack header windows are touched), so it appears fast.
fn background_load(
    cfg: Config,
    submeshes: Vec<SubMesh>,
    rig_bones: usize,
    legacy_textures: bool,
    tx: std::sync::mpsc::Sender<BgMsg>,
) {
    let n = submeshes.len();

    // 1. clip catalog — first thing to fill. From the pack's own ANIM block unless `--index` names a
    // generated catalog (see `anim_index`).
    let catalog = match load_catalog(&cfg, rig_bones) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[sab_workshop] anim catalog: {e}");
            AnimCatalog { clips: Vec::new(), skeleton_bones: rig_bones, num_main_clips: 0 }
        }
    };
    let playable_rows: Vec<usize> =
        catalog.clips.iter().enumerate().filter(|(_, c)| c.playable).map(|(i, _)| i).collect();
    let _ = tx.send(BgMsg::Catalog { catalog, playable_rows });

    // 2. megapack (mmap = instant) + the model list (MSHA chain-walk — only headers).
    let megapack = match pack::Megapack::open(&cfg.megapack) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("[sab_workshop] megapack unavailable ({e}) — no browsing / textures");
            None
        }
    };
    let mesh_list = megapack.as_ref().map(meshload::list_meshes).unwrap_or_default();
    // Assemble assets from the game's own GameTemplates (FxHumanBodySetup / Weapon / CAR / Prop …).
    let game_dir = cfg.game_dir.clone();
    let assets = match crate::assets::load_gametemplates(&game_dir) {
        Some(gt) => crate::assets::build(&gt, &mesh_list),
        None => {
            eprintln!("[sab_workshop] GameTemplates unavailable — browser falls back to raw meshes");
            crate::assets::build_flat(&mesh_list)
        }
    };
    let _ = tx.send(BgMsg::ModelList { mesh_list, assets });

    // 3. hand the megapacks to the main thread NOW — click-to-load must work the moment the list
    // shows, not after the slow texture scan below. The mmaps are cheap to open a second time, so the
    // worker keeps `megapack` (for the texture resolve) and the main thread gets its own.
    let _ = tx.send(BgMsg::Packs {
        megapack: pack::Megapack::open(&cfg.megapack).ok(),
        palettes: pack::Megapack::open(&cfg.palettes).ok(),
    });

    // 4. the LEGACY texture path for a `--mesh` file: a name-token bundle scan + a per-part suffix
    // seed. Skipped entirely for a pack-assembled model, which is textured through the engine's own
    // material table on the main thread's resolve (see `spawn_tex_resolve`).
    let mut assets: Vec<resolve::TexAsset> = Vec::new();
    let mut submesh_tex: Vec<Option<usize>> = vec![None; n];
    let mut submesh_prov: Vec<resolve::Prov> = vec![resolve::Prov::Unresolved; n];
    let mut decoded: Vec<(usize, crate::dtex::CpuTexture)> = Vec::new();
    let mesh_path = cfg.mesh.clone().unwrap_or_default();
    if let (true, Some(mp)) = (legacy_textures, &megapack) {
        let a = resolve::textures_in(mp, &cfg.char_token);
        (submesh_tex, submesh_prov) = resolve::load_sidecar(&mesh_path, n, &a).unwrap_or_else(|| {
            let assign = resolve::autoseed(&mesh_path, &submeshes, &a);
            let prov = resolve::seed_prov(&assign);
            (assign, prov)
        });
        for (i, slot) in submesh_tex.iter().enumerate() {
            if let Some(ai) = slot {
                match a[*ai].decode() {
                    Ok(tex) => decoded.push((i, tex)),
                    Err(e) => eprintln!("[sab_workshop] decode {}: {e}", a[*ai].name),
                }
            }
        }
        assets = a;
    }
    // Engine-faithful WSAO override (Mattias port): material hash → diffuse texture hash → loose DTEX.
    if let (Some(wp), Some(dd)) = (&cfg.wsao, &cfg.dtex_dir) {
        if let Ok(w) = crate::wsao::Wsao::open(wp) {
            for (i, sm) in submeshes.iter().enumerate() {
                let Some(&mat) = sm.materials.first() else { continue };
                let Some(texes) = w.textures(mat) else { continue };
                let Some(&diffuse) = texes.first() else { continue };
                if let Ok(tex) = crate::wsao::load_loose_dtex(dd, diffuse) {
                    decoded.push((i, tex));
                }
            }
        }
    }
    // Only send if one of the two paths above actually produced something — an empty result would
    // CLEAR the material-table bindings the boot resolve had already applied.
    if legacy_textures || !decoded.is_empty() {
        let _ = tx.send(BgMsg::Textures { assets, submesh_tex, submesh_prov, decoded });
    }
    drop(megapack); // the main thread has its own copy (sent in stage 3)

    // 5. Animations.pack (187 MB) — needed only to PLAY a clip, so it is loaded last.
    let pack = PackData::load(&cfg.pack).map_err(|e| eprintln!("[sab_workshop] pack: {e}")).ok();
    let _ = tx.send(BgMsg::AnimPack(pack));
}

/// Headless verification of the WSAO texture-binding path (no window). For every assembled asset
/// whose name contains `filter` (case-insensitive), assemble its parts and run the EXACT click-path
/// resolver `resolve_model_textures`, then print each submesh's material hash → bound texture (or
/// "unresolved") with its provenance, and a per-asset + overall coverage tally. This is how we prove
/// non-base outfits (SeanRacing …) bind through France.materials rather than the name heuristic.
pub fn texcheck(cfg: Config, filter: &str) -> i32 {
    let mp = match pack::Megapack::open(&cfg.megapack) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("texcheck: megapack: {e}");
            return 1;
        }
    };
    let mesh_list = meshload::list_meshes(&mp);
    let game_dir = cfg.game_dir.clone();
    let assets = match crate::assets::load_gametemplates(&game_dir) {
        Some(gt) => crate::assets::build(&gt, &mesh_list),
        None => crate::assets::build_flat(&mesh_list),
    };

    let filt = filter.to_ascii_lowercase();
    let (mut n_assets, mut tot_sub, mut tot_bound, mut tot_wsao) = (0usize, 0usize, 0usize, 0usize);
    for a in &assets {
        if !filt.is_empty() && !a.name.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        let idxs = a.assembly();
        let picked: Vec<meshload::MeshEntry> =
            idxs.iter().filter_map(|i| mesh_list.get(*i).cloned()).collect();
        if picked.is_empty() {
            continue;
        }
        let lm = match meshload::assemble(mp.raw(), &picked) {
            Ok(l) => l,
            Err(_) => continue, // static/unskinned — the loader only handles skinned meshes
        };
        let submeshes =
            formats::submesh_cover(&lm.mesh.prims, lm.mesh.indices.len() as u32);
        let pinfo: Vec<(String, usize, u32, u32)> = lm
            .part_ranges
            .iter()
            .map(|(nm, i0, ilen)| {
                let off = picked.iter().find(|e| &e.name == nm).map(|e| e.file_off).unwrap_or(0);
                (nm.clone(), off, *i0, *ilen)
            })
            .collect();
        let starts: Vec<u32> = submeshes.iter().map(|s| s.index_start).collect();
        let mats: Vec<Option<u32>> = submeshes.iter().map(|s| s.materials.first().copied()).collect();
        let Some(mt) = resolve_model_textures(cfg.clone(), pinfo, starts, mats.clone(), 0) else {
            continue;
        };

        n_assets += 1;
        let bound = mt.submesh_tex.iter().filter(|t| t.is_some()).count();
        let wsao = mt.submesh_prov.iter().filter(|p| **p == resolve::Prov::Bound).count();
        tot_sub += submeshes.len();
        tot_bound += bound;
        tot_wsao += wsao;
        println!(
            "\n{}  ({} parts, {} submeshes)  bound {}/{}  [WSAO {}, seeded {}]",
            a.name,
            picked.len(),
            submeshes.len(),
            bound,
            submeshes.len(),
            wsao,
            bound - wsao,
        );
        for (i, sm) in submeshes.iter().enumerate() {
            let mat = mats[i].map(|m| format!("{m:08X}")).unwrap_or_else(|| "--------".into());
            let (tex, prov) = match mt.submesh_tex[i] {
                Some(ai) => (mt.assets[ai].name.as_str(), match mt.submesh_prov[i] {
                    resolve::Prov::Bound => "WSAO",
                    resolve::Prov::Seeded => "seed",
                    resolve::Prov::Unresolved => "??",
                }),
                None => ("(unresolved)", "--"),
            };
            println!("  [{i:2}] mat {mat}  {prov:>4}  {tex}");
        }
    }
    println!(
        "\ntexcheck '{filter}': {n_assets} assets, {tot_bound}/{tot_sub} submeshes bound ({tot_wsao} via WSAO, {} via heuristic)",
        tot_bound - tot_wsao
    );
    0
}

/// Headless self-test: load assets, decode the N-th playable clip, run skinning over a few
/// frames, and print sanity stats. Returns a process exit code. Exercises the whole
/// load -> decode -> skin path without opening a window.
pub fn selftest(cfg: Config, n: usize) -> i32 {
    let boot = match crate::boot::load(&cfg) {
        Ok(b) => b,
        Err(e) => { eprintln!("selftest: boot model: {e}"); return 1; }
    };
    println!("selftest: boot model {} ({})", boot.name, if boot.from_pack() { "from the install" } else { "from files" });
    let mesh = boot.mesh;
    let skel = boot.bones;
    let catalog = match load_catalog(&cfg, skel.len()) {
        Ok(c) => c,
        Err(e) => { eprintln!("selftest: catalog: {e}"); return 1; }
    };
    let pack = match PackData::load(&cfg.pack) {
        Ok(p) => Some(p),
        Err(e) => { eprintln!("selftest: pack: {e}"); return 1; }
    };
    println!(
        "selftest: mesh {} verts / {} tris, skeleton {} bones, catalog {} clips",
        mesh.positions.len(), mesh.indices.len() / 3, skel.len(), catalog.clips.len()
    );

    // Max joint index referenced by the mesh — must be < bone count for skinning to be valid.
    let max_joint = mesh.joints.iter().flat_map(|j| j.iter()).copied().max().unwrap_or(0);
    println!("selftest: max mesh joint index = {max_joint} (skeleton has {} bones)", skel.len());

    // Bind-pose joint matrices: each should be ~identity if inv_bind is consistent.
    let bind = skinning::bind_pose(&skel);
    let mut worst = 0f32;
    for m in &bind {
        let d = *m - glam::Mat4::IDENTITY;
        for c in d.to_cols_array() { worst = worst.max(c.abs()); }
    }
    println!("selftest: bind-pose max deviation of jointMatrix from identity = {worst:.5} (expect ~0)");

    let playable: Vec<usize> = catalog.clips.iter().enumerate().filter(|(_, c)| c.playable).map(|(i, _)| i).collect();
    let ci = *playable.get(n).unwrap_or(&0);
    let clip = match load_clip(&pack, &catalog, ci) {
        Ok(c) => c,
        Err(e) => { eprintln!("selftest: load clip: {e}"); return 1; }
    };
    println!(
        "selftest: clip '{}' — {} tracks, {} frames, {:.3}s, frameDur {:.5}",
        clip.name, clip.anim.num_transform_tracks, clip.frame_count, clip.duration, clip.frame_duration
    );
    let pk = pack.as_ref().unwrap();
    let mut max_disp = 0f32;
    let steps = clip.frame_count.min(8).max(1);
    for f in 0..steps {
        let t = f as f32 * clip.duration / steps as f32;
        let pose = clip.anim.sample_at(pk.blob(), t);
        // Root locked: otherwise this measures root TRAVEL (metres), not pose error.
        let mats = skinning::posed(&skel, &pose, &clip.track_to_bone, true);
        // How far a unit vertex at each bone origin moves vs bind — a coarse "is it animating" probe.
        for (m, b) in mats.iter().zip(&bind) {
            let d = m.w_axis - b.w_axis;
            max_disp = max_disp.max(d.truncate().length());
        }
        if mats.len() != skel.len() { eprintln!("selftest: joint count mismatch!"); return 1; }
    }
    println!("selftest: sampled {steps} frames, max joint-origin displacement vs bind = {max_disp:.4} m");

    // DIAGNOSTIC: how many tracks does the clip actually store translation for? Havok animates most
    // bones by ROTATION only, leaving translation absent = "keep the reference pose". If the decoder
    // hands back (0,0,0) for those, every such bone collapses onto its parent's origin.
    {
        let pose0 = clip.anim.sample_at(pk.blob(), 0.0);
        let zero_t = pose0.iter().filter(|q| q.t[0] == 0.0 && q.t[1] == 0.0 && q.t[2] == 0.0).count();
        let ident_r = pose0
            .iter()
            .filter(|q| q.r[0] == 0.0 && q.r[1] == 0.0 && q.r[2] == 0.0 && q.r[3] == 1.0)
            .count();
        println!(
            "selftest: at t=0 of {} tracks -> {} have translation exactly (0,0,0), {} have identity rotation",
            pose0.len(),
            zero_t,
            ident_r
        );
        // What SHOULD those bones' translations be? Compare against the rig's bind locals.
        let mut nonzero_bind = 0;
        for (k, q) in pose0.iter().enumerate() {
            let bone = clip.track_to_bone.get(k).copied().unwrap_or(-1);
            if bone < 0 || bone as usize >= skel.len() {
                continue;
            }
            let bt = skel[bone as usize].t;
            let bind_len = (bt[0] * bt[0] + bt[1] * bt[1] + bt[2] * bt[2]).sqrt();
            if q.t[0] == 0.0 && q.t[1] == 0.0 && q.t[2] == 0.0 && bind_len > 1e-4 {
                nonzero_bind += 1;
                // NAME it. A bone that takes (0,0,0) where its bind translation is non-zero
                // collapses onto its parent's origin, and drags its whole subtree with it — which
                // is what the long spikes in the viewport are.
                println!(
                    "selftest:   COLLAPSE track {k} -> bone {bone} '{}' — decoded t=(0,0,0) but bind t=({:.3},{:.3},{:.3}), |bind|={bind_len:.3}m",
                    skel[bone as usize].name, bt[0], bt[1], bt[2]
                );
            }
        }
        println!(
            "selftest: {} of those zero-translation tracks drive a bone whose BIND translation is non-zero",
            nonzero_bind
        );

        // WHICH bones actually fly?
        //
        // NOT by jointMatrix translation — that is the bind→posed transform, not a position, and
        // for a bone 1.7 m off the floor an honest rotation puts metres in it. The real question is
        // where the bone's ORIGIN ends up, and since jointMatrix = world · inv_bind (with
        // inv_bind = bind_world⁻¹), `jointMatrix · bind_world` recovers the posed world matrix.
        {
            let mid = clip.duration * 0.5;
            let pose_m = clip.anim.sample_at(pk.blob(), mid);
            let posed_m = skinning::posed(&skel, &pose_m, &clip.track_to_bone, true);
            let bind_w = skinning::bind_world(&skel);
            let mut worst: Vec<(f32, usize)> = posed_m
                .iter()
                .zip(&bind_w)
                .enumerate()
                .map(|(i, (jm, bw))| {
                    let posed_pos = (*jm * *bw).w_axis.truncate();
                    let bind_pos = bw.w_axis.truncate();
                    ((posed_pos - bind_pos).length(), i)
                })
                .collect();
            worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            // How many vertices does each bone actually carry? A bone that flies but has no skin
            // weighted to it is invisible; one with thousands of verts is the thing on screen.
            let mut vert_count = vec![0usize; skel.len()];
            for (j4, w4) in mesh.joints.iter().zip(&mesh.weights) {
                for k in 0..4 {
                    if w4[k] > 0.5 {
                        let b = j4[k] as usize;
                        if b < vert_count.len() {
                            vert_count[b] += 1;
                        }
                    }
                }
            }
            let flying: usize = worst
                .iter()
                .filter(|(d, _)| *d > 0.5)
                .map(|(_, i)| vert_count[*i])
                .sum();
            println!(
                "selftest: vertices dominantly weighted to a bone that travels >0.5 m: {flying} of {}",
                mesh.positions.len()
            );
            println!("selftest: worst BONE-ORIGIN travel vs bind at t={mid:.2}s (root locked):");
            for (d, i) in worst.iter().take(10) {
                let _ = vert_count[*i];
                let tracked = clip.track_to_bone.iter().position(|b| *b == *i as i32);
                println!(
                    "selftest:   {d:7.3} m  bone {i:3} '{}' parent={} verts={} {}",
                    skel[*i].name,
                    skel[*i].parent,
                    vert_count[*i],
                    match tracked {
                        Some(t) => format!("(track {t})"),
                        None => "(NO TRACK — holds bind local)".into(),
                    }
                );
            }
        }
        // Compare decoded values against the rig's bind locals: at t=0 a sane clip should sit near
        // the bind pose in scale/order-of-magnitude, and quats must be unit.
        println!("selftest: track -> bone | decoded t | bind t | decoded q (xyzw) | euler° | decoded s");
        for (k, q) in pose0.iter().enumerate().take(8) {
            let bone = clip.track_to_bone.get(k).copied().unwrap_or(-1);
            let (bt, nm) = if bone >= 0 && (bone as usize) < skel.len() {
                (skel[bone as usize].t, skel[bone as usize].name.clone())
            } else {
                ([0.0; 3], "(unbound)".into())
            };
            // The quaternion COMPONENTS, plus the same rotation as XYZ Euler degrees. A
            // coordinate-space conversion hiding in a track shows up here as a clean ±90° about a
            // single axis, which |q| (always 1.0 for any rotation) can never reveal.
            let quat = glam::Quat::from_xyzw(q.r[0], q.r[1], q.r[2], q.r[3]).normalize();
            let (ex, ey, ez) = quat.to_euler(glam::EulerRot::XYZ);
            println!(
                "  {k:2} -> {bone:3} {nm:22} t=({:8.3},{:8.3},{:8.3}) bind=({:8.3},{:8.3},{:8.3}) q=({:6.3},{:6.3},{:6.3},{:6.3}) euler=({:7.1},{:7.1},{:7.1}) s=({:.2},{:.2},{:.2})",
                q.t[0], q.t[1], q.t[2], bt[0], bt[1], bt[2],
                q.r[0], q.r[1], q.r[2], q.r[3],
                ex.to_degrees(), ey.to_degrees(), ez.to_degrees(),
                q.s[0], q.s[1], q.s[2]
            );
        }
        // Magnitude sweep: how far do decoded translations stray from bind across all tracks?
        let mut worst = (0usize, 0f32);
        for (k, q) in pose0.iter().enumerate() {
            let bone = clip.track_to_bone.get(k).copied().unwrap_or(-1);
            if bone < 0 || bone as usize >= skel.len() {
                continue;
            }
            let bt = skel[bone as usize].t;
            let d = ((q.t[0] - bt[0]).powi(2) + (q.t[1] - bt[1]).powi(2) + (q.t[2] - bt[2]).powi(2)).sqrt();
            if d > worst.1 {
                worst = (k, d);
            }
        }
        println!(
            "selftest: worst |decoded_t - bind_t| = {:.3} m at track {} (a rotation-only rig should be ~0)",
            worst.1, worst.0
        );
        // track -> bone mapping sanity: in range? duplicated? how much of the rig is driven?
        let n_tracks = clip.anim.num_transform_tracks;
        let map = &clip.track_to_bone;
        let unbound = map.iter().take(n_tracks).filter(|&&b| b < 0).count();
        let oob = map.iter().take(n_tracks).filter(|&&b| b >= skel.len() as i32).count();
        let mut seen = std::collections::HashSet::new();
        let dupes = map
            .iter()
            .take(n_tracks)
            .filter(|&&b| b >= 0 && !seen.insert(b))
            .count();
        println!(
            "selftest: track_to_bone: len={} tracks={} unbound={} out_of_range={} duplicate_targets={} distinct_bones={} / {} rig bones",
            map.len(), n_tracks, unbound, oob, dupes, seen.len(), skel.len()
        );
        // Which bones are NOT driven? They keep their bind local (correct), but if a driven bone's
        // PARENT is undriven-but-should-be, the pose tears.
        println!(
            "selftest: {} rig bones have no track (they hold bind local)",
            skel.len() - seen.len()
        );
    }

    // Texture pipeline (no GPU): submesh cover + in-app resolve + auto-seed.
    let cover = formats::submesh_cover(&mesh.prims, mesh.indices.len() as u32);
    println!("selftest: submesh cover = {} draw ranges", cover.len());
    // Model list via the fast MSHA chain-walk, then assemble assets from GameTemplates + count them.
    if let Ok(mp) = pack::Megapack::open(&cfg.megapack) {
        let list = meshload::list_meshes(&mp);
        let game_dir = cfg.game_dir.clone();
        let assets = match crate::assets::load_gametemplates(&game_dir) {
            Some(gt) => crate::assets::build(&gt, &list),
            None => { println!("selftest: GameTemplates not found — flat fallback"); crate::assets::build_flat(&list) }
        };
        let mut by_cat: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for a in &assets {
            *by_cat.entry(a.category).or_default() += 1;
        }
        println!("selftest: {} meshes (chain-walk) -> {} assembled assets (GameTemplates); categories:", list.len(), assets.len());
        for (c, n) in &by_cat {
            println!("  {c:12} {n}");
        }
        // spot-check the assembled Sean
        if let Some(a) = assets.iter().find(|a| a.name == "FBS_RS_Sean") {
            println!("  FBS_RS_Sean parts: {}", a.parts.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join(", "));
        }
    }

    match resolve::load_character_textures(&cfg.megapack, &cfg.char_token) {
        Ok(assets) => {
            let diffuse = assets.iter().filter(|a| a.role == resolve::Role::Diffuse).count();
            let seed = resolve::autoseed(cfg.mesh.as_deref().unwrap_or_default(), &cover, &assets);
            let applied = seed.iter().filter(|a| a.is_some()).count();
            println!("selftest: resolved {} textures ({diffuse} diffuse); auto-seeded {applied}/{} submeshes", assets.len(), cover.len());
            for (i, a) in seed.iter().enumerate() {
                let tex = a.map(|ai| assets[ai].name.as_str()).unwrap_or("(none)");
                println!(
                    "  submesh {i:2} [{:6}..{:6}] -> {tex}",
                    cover[i].index_start,
                    cover[i].index_start + cover[i].index_count
                );
            }
        }
        Err(e) => println!("selftest: textures unavailable ({e})"),
    }
    println!("selftest: OK");
    0
}

/// Sweep many clips and report which ones decode to an insane pose — the fast way to tell a
/// systemic decoder bug from one confined to a specific path (multi-block, a rarer rotation
/// quantization, …). A track's translation should stay near its bone's BIND translation; only a
/// root-motion bone legitimately travels.
pub fn anim_sweep(cfg: Config, limit: usize) -> i32 {
    let skel = match crate::boot::load(&cfg) {
        Ok(b) => b.bones,
        Err(e) => { eprintln!("sweep: rig: {e}"); return 1; }
    };
    let catalog = match load_catalog(&cfg, skel.len()) {
        Ok(c) => c,
        Err(e) => { eprintln!("sweep: catalog: {e}"); return 1; }
    };
    let pack = match PackData::load(&cfg.pack) {
        Ok(p) => Some(p),
        Err(e) => { eprintln!("sweep: pack: {e}"); return 1; }
    };
    let pk = pack.as_ref().unwrap();
    let playable: Vec<usize> =
        catalog.clips.iter().enumerate().filter(|(_, c)| c.playable).map(|(i, _)| i).collect();
    let n = limit.min(playable.len());
    println!("sweep: checking {n} clips (bad = a NON-root-chain limb bone >0.75 m from its bind local translation)");
    let (mut bad, mut multi_block_bad, mut single_block_bad) = (0usize, 0usize, 0usize);
    let mut by_blocks: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    let mut ctrl_tally: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut max_scale_dev = (String::new(), 0f32);
    let (mut tot_nonfinite, mut tot_nonunit) = (0usize, 0usize);
    for &ci in playable.iter().take(n) {
        let clip = match load_clip(&pack, &catalog, ci) { Ok(c) => c, Err(_) => continue };
        let nb = clip.anim.num_blocks;
        let mut worst = 0f32;
        let mut worst_bone = (-1i32, String::new(), 0f32);
        let mut worst_scale = 0f32;
        let mut nonfinite = 0usize;
        let mut nonunit = 0usize;
        for f in 0..4 {
            let t = f as f32 * clip.duration / 4.0;
            let pose = clip.anim.sample_at(pk.blob(), t);
            for (k, q) in pose.iter().enumerate() {
                let bone = clip.track_to_bone.get(k).copied().unwrap_or(-1);
                if bone < 0 || bone as usize >= skel.len() {
                    continue;
                }
                // The root chain (GlobalSRT / Bone_Root / Bone_Hips) LEGITIMATELY translates —
                // climbing/hanging/getup clips move the body metres. A LIMB bone's local
                // translation is its fixed bone length, so any drift there is a decode fault.
                let nm = skel[bone as usize].name.to_ascii_lowercase();
                let is_root_chain = nm.contains("globalsrt") || nm.contains("bone_root") || nm.contains("hips");
                if is_root_chain { continue; }
                let bt = skel[bone as usize].t;
                let d = ((q.t[0]-bt[0]).powi(2) + (q.t[1]-bt[1]).powi(2) + (q.t[2]-bt[2]).powi(2)).sqrt();
                if d > worst { worst = d; worst_bone = (bone, skel[bone as usize].name.clone(), d); }
            }
            // SCALE check: the memory/decomp says the corpus is uniformly ctrl=0x45 with scale == 1.
            // A bad scale is the only channel that can STRETCH geometry (rotations can't), so this is
            // the prime suspect for the shredded mesh.
            for q in pose.iter() {
                for c in 0..3 {
                    let dev = (q.s[c] - 1.0).abs();
                    if dev > worst_scale { worst_scale = dev; }
                }
                if !q.s[0].is_finite() || !q.s[1].is_finite() || !q.s[2].is_finite() { nonfinite += 1; }
                if !q.t[0].is_finite() || !q.t[1].is_finite() || !q.t[2].is_finite() { nonfinite += 1; }
                let qn = (q.r[0]*q.r[0]+q.r[1]*q.r[1]+q.r[2]*q.r[2]+q.r[3]*q.r[3]).sqrt();
                if (qn - 1.0).abs() > 1e-3 { nonunit += 1; }
            }
        }
        let e = by_blocks.entry(nb).or_insert((0, 0));
        e.0 += 1;
        // Which quantization paths does this clip use? Corpus was reported uniformly ctrl=0x45.
        let masks = clip.anim.track_masks(pk.blob(), 0);
        let mut ctrls: Vec<u8> = masks.iter().map(|m| m[0]).collect();
        ctrls.sort_unstable();
        ctrls.dedup();
        let ctrl_str = ctrls.iter().map(|c| format!("{c:#04x}")).collect::<Vec<_>>().join(",");
        let te = ctrl_tally.entry(ctrl_str.clone()).or_insert((0usize, 0usize));
        te.0 += 1;
        if worst > 0.75 { te.1 += 1; }
        if worst_scale > max_scale_dev.1 { max_scale_dev = (clip.name.clone(), worst_scale); }
        tot_nonfinite += nonfinite;
        tot_nonunit += nonunit;
        if worst > 0.75 {
            bad += 1;
            e.1 += 1;
            if nb > 1 { multi_block_bad += 1 } else { single_block_bad += 1 }
            if bad <= 8 {
                println!(
                    "  BAD {:<38} frames={} worst={:.2} m on bone {} '{}'",
                    clip.name, clip.frame_count, worst, worst_bone.0, worst_bone.1
                );
            }
        }
    }
    println!("sweep: {bad}/{n} bad  (single-block bad: {single_block_bad}, multi-block bad: {multi_block_bad})");
    println!("sweep: worst |scale-1| = {:.4} in '{}'  (corpus should be scale==1)", max_scale_dev.1, max_scale_dev.0);
    println!("sweep: non-finite components: {tot_nonfinite}   non-unit quaternions: {tot_nonunit}");
    println!("sweep: by ctrl-byte set -> (checked, bad):");
    for (c, (tot, b)) in &ctrl_tally {
        println!("   ctrl={{{c}}}  checked={tot:<5} bad={b}");
    }
    println!("sweep: by num_blocks -> (checked, bad):");
    for (nb, (tot, b)) in by_blocks {
        println!("   blocks={nb:<3} checked={tot:<5} bad={b}");
    }
    0
}

/// Decode the requested clip. `ci` indexes `catalog.clips`.
fn load_clip(pack: &Option<PackData>, catalog: &AnimCatalog, ci: usize) -> Result<LoadedClip, String> {
    let pk = pack.as_ref().ok_or("no Animations.pack loaded")?;
    let clip = catalog.clips.get(ci).ok_or("clip index out of range")?;
    let n = clip.index;
    if n >= pk.scas.len() {
        return Err(format!("clip #{n} beyond pack ({} spline anims)", pk.scas.len()));
    }
    // Re-parse the packfile (cheap; only needed to resolve the object's hkArrays).
    let blob = pk.blob();
    let packfile = Packfile::parse(blob)?;
    let anim = havok::read_spline_anim(&packfile, pk.scas[n]);
    let frame_count = anim.num_frames.max(1);
    let frame_duration = if anim.frame_duration.is_finite() && anim.frame_duration > 0.0 {
        anim.frame_duration
    } else {
        1.0 / 30.0
    };
    let duration = if clip.duration > 0.0 { clip.duration } else { anim.duration };
    Ok(LoadedClip {
        anim,
        track_to_bone: clip.track_to_bone.clone(),
        name: clip.name.clone(),
        duration,
        frame_duration,
        frame_count,
    })
}

/// Axis-aligned bounding sphere (center of bbox + half-diagonal) of the mesh.
fn bounds(mesh: &Smsh) -> (Vec3, f32) {
    if mesh.positions.is_empty() {
        return (Vec3::ZERO, 1.0);
    }
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    for p in &mesh.positions {
        let v = Vec3::from_array(*p);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let center = (lo + hi) * 0.5;
    let radius = (hi - lo).length() * 0.5;
    (center, radius.max(0.1))
}

/// Contact-sheet thumbnails, decoded on first sight and kept.
///
/// The pool runs to a couple of hundred records, so this is deliberately lazy in two ways: a
/// thumbnail is only decoded when its tile is actually drawn (egui's ScrollArea culls the rest),
/// and it decodes a small mip rather than the finest one. Opening the Textures page therefore costs
/// roughly a screenful of tiny BC decodes, not the whole pool.
///
/// A record that fails to decode is remembered as a failure (`None`), or we would retry the same
/// broken record every frame for as long as the page is open.
#[derive(Default)]
struct Thumbs {
    cache: std::collections::HashMap<usize, Option<egui::TextureHandle>>,
}

impl Thumbs {
    fn get(
        &mut self,
        i: usize,
        ctx: &egui::Context,
        assets: &[resolve::TexAsset],
    ) -> Option<egui::TextureHandle> {
        if let Some(hit) = self.cache.get(&i) {
            return hit.clone();
        }
        let made = assets.get(i).and_then(|a| match a.decode_preview(96) {
            Ok(t) => {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                );
                Some(ctx.load_texture(format!("thumb{i}"), img, egui::TextureOptions::LINEAR))
            }
            Err(e) => {
                eprintln!("[sab_workshop] thumb {}: {e}", a.name);
                None
            }
        });
        self.cache.insert(i, made.clone());
        made
    }

    /// Drop everything — the asset pool changed, so every index is stale.
    fn clear(&mut self) {
        self.cache.clear();
    }
}

/// Depth of each bone in the hierarchy, so the Rig tree can indent by it.
///
/// Bones are stored parent-first (a child's `parent` always indexes an earlier bone), so one
/// forward pass is enough. A bone whose parent index is out of range or self-referential is treated
/// as a root rather than trusted — a malformed `.skel` should indent oddly, not hang.
fn bone_depths(skel: &[formats::Bone]) -> Vec<usize> {
    let mut d = vec![0usize; skel.len()];
    for i in 0..skel.len() {
        let p = skel[i].parent;
        d[i] = if p >= 0 && (p as usize) < i { d[p as usize] + 1 } else { 0 };
    }
    d
}

/// A rail page. Each one exists because its LAYOUT differs, not merely its subject: a contact sheet
/// is not a list, and a bone tree is not either. Pages that would share a layout share a page.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    /// The character/animation viewer — pick a model or clip and look at it (wgpu scene + rig).
    Inspect,
    /// Edit GameText.dlg — everything the player reads, in seven languages.
    Strings,
    /// Edit GameTemplates.wsd (AULB) — what a thing IS: its property pairs.
    Objects,
    /// Browse the texture pool as pictures, and wire one into an object property.
    Icons,
    /// Where the game is installed, and the defaults every other page starts from.
    Settings,
}

impl Page {
    const ALL: [Page; 5] =
        [Page::Inspect, Page::Strings, Page::Objects, Page::Icons, Page::Settings];
    fn label(self) -> &'static str {
        match self {
            Page::Inspect => "inspect",
            Page::Strings => "strings",
            Page::Objects => "objects",
            Page::Icons => "icons",
            Page::Settings => "settings",
        }
    }
    /// The rail mark. Defined in [`theme::sym`], not inline: a glyph no bundled font draws paints an
    /// empty box with no error, which is exactly how `▤`/`▦` sat blank on this rail.
    fn glyph(self) -> &'static str {
        use theme::sym;
        match self {
            Page::Inspect => sym::INSPECT,
            Page::Strings => sym::STRINGS,
            Page::Objects => sym::OBJECTS,
            Page::Icons => sym::ICONS,
            Page::Settings => sym::SETTINGS,
        }
    }
    /// The editor page this maps to, or `None` for the pages that are not editor views
    /// (Inspect is the wgpu viewer; Settings is app configuration).
    fn editor(self) -> Option<crate::editor::EdPage> {
        use crate::editor::EdPage;
        match self {
            Page::Inspect | Page::Settings => None,
            Page::Strings => Some(EdPage::Strings),
            Page::Objects => Some(EdPage::Objects),
            Page::Icons => Some(EdPage::Icons),
        }
    }
    /// The 1-5 shortcut that selects this page.
    fn key(self) -> egui::Key {
        match self {
            Page::Inspect => egui::Key::Num1,
            Page::Strings => egui::Key::Num2,
            Page::Objects => egui::Key::Num3,
            Page::Icons => egui::Key::Num4,
            Page::Settings => egui::Key::Num5,
        }
    }
}

/// Which category the Inspect navigator is browsing. Models and clips share this page because they
/// share a shape — a filtered list you pick from. (`textures` retired to its own rail page: a
/// contact sheet is a different layout, which is exactly when a page is warranted.)
#[derive(Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Models,
    Animations,
}

impl NavTab {
    const ALL: [NavTab; 2] = [NavTab::Models, NavTab::Animations];
    fn label(self) -> &'static str {
        match self {
            NavTab::Models => "models",
            NavTab::Animations => "anims",
        }
    }
}

/// The Materials panel's borrowed state: the draw list, the resolved texture pool, the current
/// assignment, and the out-param a picker click writes to.
/// The export verb's state for one frame — what the verb bar reads, and what it raises.
///
/// A struct rather than four more `&mut bool` parameters on `build_ui`, which is already at the
/// point where positional arguments stop being readable.
struct ExportCtx<'a> {
    /// Raised by the verb-bar button or F10: start an export of what is loaded.
    request: &'a mut bool,
    /// Raised by clicking the destination path: pick a new bundle root.
    pick_dir: &'a mut bool,
    /// The asset an export is already running for, if any — the verb is disabled while set.
    busy: Option<String>,
    /// Is there a loaded model to export at all?
    ready: bool,
    /// Textures are still resolving; a bundle written now would be untextured.
    tex_pending: bool,
    /// Where this export would land, shown so it is never a mystery.
    dest: String,
}

struct MatsCtx<'a> {
    submeshes: &'a [SubMesh],
    assets: &'a [resolve::TexAsset],
    submesh_tex: &'a [Option<usize>],
    /// How each binding was arrived at — bound / seeded / unresolved, parallel to `submesh_tex`.
    submesh_prov: &'a [resolve::Prov],
    /// Which submesh the Materials page is working on.
    selected: &'a mut Option<usize>,
    pending_tex: &'a mut Option<(usize, Option<usize>)>,
    /// Set by the stamp: re-write the sidecar as it currently stands.
    pending_save: &'a mut bool,
}

/// The navigator's borrowed state: the browsable catalogs + the click out-params.
struct NavCtx<'a> {
    tab: &'a mut NavTab,
    search: &'a mut String,
    /// The assembled assets the browser lists (from GameTemplates).
    browse_assets: &'a [crate::assets::Asset],
    /// The selected asset (whose parts the inspector shows).
    sel_asset: &'a mut Option<usize>,
    assets: &'a [resolve::TexAsset],
    pending_model: &'a mut Option<Vec<usize>>,
    /// Loaded model's name, so the browser can mark the active row.
    current_model: &'a str,
    /// The rig, for the Rig page's tree.
    bones: &'a [formats::Bone],
    /// Depth per bone, precomputed once — the tree indents by it.
    bone_depth: &'a [usize],
    selected_bone: &'a mut Option<usize>,
    /// Which DTEX record the Textures page is showing.
    selected_tex: &'a mut Option<usize>,
    /// Model-browser groupings (rule-based + user overrides), for the grouped model tree.
    groups: &'a crate::models::Groups,
    /// A right-click "move to group" request: (model index, new category), applied after the UI.
    pending_group: &'a mut Option<(usize, String)>,
    /// A right-click "Export bundle" on a row that is not the loaded model: arm an export to fire
    /// once `pending_model` has loaded AND its texture resolve has landed.
    export_after_load: &'a mut bool,
    /// True until the model list has streamed in from the megapack — show a spinner meanwhile.
    models_loading: bool,
}

/// Live state for the Settings page: the values being edited, plus what changing them implies.
pub struct SettingsCtx {
    pub s: crate::settings::Settings,
    /// The install path as typed. Kept separate from `s.game_dir` so a half-typed path does not
    /// re-run the existence checks against nonsense on every keystroke — it commits on Apply.
    pub dir_edit: String,
    /// The install `s` was launched with. Changing the install cannot take effect in place (the
    /// megapacks are mmapped and every derived index is built from them at boot), so we compare
    /// against this to decide whether to say "restart to apply".
    pub launched_with: String,
    pub note: Option<(String, bool)>, // (message, is_error)
}

/// The Settings page. One column of labelled groups; nothing here is a mod edit, so nothing here
/// touches the changelist or the DLC overlay.
fn settings_page(ctx: &egui::Context, sx: &mut SettingsCtx) {
    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(720.0);
            ui.add_space(8.0);
            ui.label(theme::poster_text("SETTINGS", 22.0, theme::TX));
            ui.label(theme::data_text(
                &format!("stored at {}", crate::settings::settings_path().display()),
                10.0,
                theme::FAINT,
            ));
            ui.add_space(14.0);

            // ---- game install ----
            theme::eyebrow(ui, "Game install");
            ui.label(theme::data_text(
                "Everything the workshop reads comes from this folder. It is opened READ-ONLY; mods publish to a DLC overlay inside it.",
                11.0,
                theme::DIM,
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut sx.dir_edit)
                        .desired_width(430.0)
                        // Describe the folder, do not name one: the install lives somewhere
                        // different on every machine, which is what Detect is for.
                        .hint_text("the folder holding Global/, France.materials and Saboteur.exe"),
                );
                if ui.button("Browse…").clicked() {
                    if let Some(d) = rfd::FileDialog::new()
                        .set_title("Select the Saboteur install folder")
                        .pick_folder()
                    {
                        sx.dir_edit = d.to_string_lossy().replace('\\', "/");
                    }
                }
                if ui.button("Detect").clicked() {
                    match crate::settings::detect_install() {
                        Some(d) => {
                            sx.dir_edit = d;
                            sx.note = Some(("Found an install.".into(), false));
                        }
                        None => {
                            sx.note = Some((
                                "No install found in the usual GOG/Steam locations — Browse to it.".into(),
                                true,
                            ));
                        }
                    }
                }
            });

            // Validate what is TYPED, so the checklist reacts before committing.
            let probe = crate::settings::Settings { game_dir: sx.dir_edit.replace('\\', "/"), ..sx.s.clone() };
            ui.add_space(8.0);
            for c in probe.check() {
                ui.horizontal(|ui| {
                    let (mark, col) = match (c.ok, c.required) {
                        (true, _) => (theme::sym::OK, theme::COLD),
                        (false, true) => (theme::sym::NO, theme::RED),
                        (false, false) => (theme::sym::NA, theme::FAINT),
                    };
                    ui.label(theme::data_text(mark, 12.0, col));
                    ui.label(theme::data_text(c.label, 11.0, if c.ok { theme::TX } else { theme::DIM }));
                    ui.label(theme::data_text(&c.rel, 10.0, theme::FAINT));
                    if !c.ok && !c.required {
                        ui.label(theme::data_text("(optional — one feature degrades)", 10.0, theme::FAINT));
                    }
                });
            }

            ui.add_space(12.0);
            theme::eyebrow(ui, "Defaults");
            egui::Grid::new("settings_defaults").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label(theme::data_text("Language", 11.0, theme::DIM));
                egui::ComboBox::from_id_source("set_lang")
                    .selected_text(crate::settings::LANG_NAMES[sx.s.lang].1)
                    .show_ui(ui, |ui| {
                        for (i, (_, name)) in crate::settings::LANG_NAMES.iter().enumerate() {
                            ui.selectable_value(&mut sx.s.lang, i, *name);
                        }
                    });
                ui.end_row();

                ui.label(theme::data_text("Mod name", 11.0, theme::DIM));
                ui.add(egui::TextEdit::singleline(&mut sx.s.mod_name).desired_width(240.0));
                ui.end_row();

                ui.label(theme::data_text("Publish to", 11.0, theme::DIM));
                ui.horizontal(|ui| {
                    ui.label(theme::data_text("DLC/", 11.0, theme::FAINT));
                    ui.add(egui::TextEdit::singleline(&mut sx.s.dlc_slot).desired_width(48.0));
                    ui.label(theme::data_text(
                        "02–04 only: the engine probes four slots and 01 is the game's own",
                        10.0,
                        theme::FAINT,
                    ));
                });
                ui.end_row();
            });

            ui.add_space(12.0);
            theme::eyebrow(ui, "Appearance");
            ui.horizontal(|ui| {
                ui.label(theme::data_text("Type scale", 11.0, theme::DIM));
                if ui
                    .add(egui::Slider::new(&mut sx.s.type_scale, 0.8..=2.0).step_by(0.05).show_value(true))
                    .changed()
                {
                    // Applies live: set the scale, then re-install so egui's cached TextStyle table
                    // picks up the new sizes too.
                    theme::set_type_scale(sx.s.type_scale);
                    theme::install(ctx);
                }
            });
            ui.label(theme::data_text(
                "Scales every voice together, so the hierarchy between them is preserved.",
                10.0,
                theme::FAINT,
            ));

            ui.add_space(12.0);
            theme::eyebrow(ui, "Voice audio");
            ui.horizontal(|ui| {
                ui.label(theme::data_text("vgmstream-cli", 11.0, theme::DIM));
                ui.add(
                    egui::TextEdit::singleline(&mut sx.s.vgmstream_path)
                        .desired_width(320.0)
                        .hint_text("path to vgmstream-cli.exe"),
                );
                if ui.button("Browse…").clicked() {
                    let mut dlg = rfd::FileDialog::new().set_title("Select vgmstream-cli.exe");
                    if cfg!(windows) {
                        dlg = dlg.add_filter("executable", &["exe"]);
                    }
                    if let Some(f) = dlg.pick_file() {
                        sx.s.vgmstream_path = f.to_string_lossy().replace('\\', "/");
                    }
                }
            });
            {
                let (msg, col) = if sx.s.vgmstream_path.trim().is_empty() {
                    ("Optional — lets the Strings page play a line's voice. vgmstream is a separate tool (not bundled); point here or put vgmstream-cli on PATH.", theme::FAINT)
                } else if sx.s.vgmstream().is_some() {
                    ("Found — VO playback enabled on the Strings page.", theme::COLD)
                } else {
                    ("Not found at that path.", theme::RED)
                };
                ui.label(theme::data_text(msg, 10.0, col));
            }

            // ---- apply ----
            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);
            let typed = sx.dir_edit.replace('\\', "/");
            let typed = typed.trim_end_matches('/').to_string();
            let dir_changed = typed != sx.s.game_dir;
            ui.horizontal(|ui| {
                if ui.button("Save settings").clicked() {
                    sx.s.game_dir = typed.clone();
                    sx.s.clamp();
                    sx.dir_edit = sx.s.game_dir.clone();
                    theme::set_type_scale(sx.s.type_scale);
                    theme::install(ctx);
                    sx.note = match sx.s.save() {
                        Ok(()) => Some(("Saved.".into(), false)),
                        Err(e) => Some((e, true)),
                    };
                }
                if ui.button("Revert").clicked() {
                    sx.s = crate::settings::Settings::load();
                    sx.dir_edit = sx.s.game_dir.clone();
                    theme::set_type_scale(sx.s.type_scale);
                    theme::install(ctx);
                    sx.note = Some(("Reloaded from disk.".into(), false));
                }
            });

            if let Some((msg, err)) = &sx.note {
                ui.add_space(6.0);
                ui.label(theme::data_text(msg, 11.0, if *err { theme::RED } else { theme::COLD }));
            }

            // The install cannot be swapped in place: the megapacks are mmapped at boot and the model
            // list, asset tree and editor document are all built from them. Say so plainly rather
            // than half-reloading and leaving stale indices behind.
            if sx.s.game_dir != sx.launched_with || dir_changed {
                ui.add_space(8.0);
                ui.label(theme::data_text(
                    "Restart the workshop to load from the new install.",
                    11.0,
                    theme::EMBER,
                ));
            }
            ui.add_space(20.0);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn build_ui(
    ctx: &egui::Context,
    catalog: &AnimCatalog,
    playable_rows: &[usize],
    show_all: &mut bool,
    current: &Option<LoadedClip>,
    playback: &mut Playback,
    pending_load: &mut Option<usize>,
    show_grid: &mut bool,
    show_textures: &mut bool,
    lock_root: &mut bool,
    errors: &[String],
    have_pack: bool,
    rig_ok: bool,
    mesh_stats: (usize, usize, usize), // verts, tris, bones
    model_name: &str,
    load_status: &Option<(String, bool)>,
    page: &mut Page,
    thumbs: &mut Thumbs,
    exp: &mut ExportCtx,
    mats: &mut MatsCtx,
    nav: &mut NavCtx,
    ed: &mut crate::editor::Editor,
    settings: &mut SettingsCtx,
) {
    // How much of each page's subject is accounted for. Only Materials is strictly work-remaining
    // (a binding is a decision you make); the rest are coverage. Both answer the same question —
    // how much of this is resolved — which is what the rail meter reports.
    let bound = mats.submesh_prov.iter().filter(|p| **p == resolve::Prov::Bound).count();
    let textured = mats.submesh_tex.iter().filter(|s| s.is_some()).count();
    let used: usize = {
        let mut seen: Vec<usize> = mats.submesh_tex.iter().flatten().copied().collect();
        seen.sort_unstable();
        seen.dedup();
        seen.len()
    };
    let on_rig = playable_rows.len();

    // 1-4 select a page — but not while a text field has focus, or you cannot type "1" into the
    // search box.
    if !ctx.wants_keyboard_input() {
        for p in Page::ALL {
            if ctx.input(|i| i.key_pressed(p.key())) {
                *page = p;
            }
        }
    }

    // Land any editor assets that finished loading since the last frame, before anything renders.
    ed.pump(ctx);

    // ── COMMAND BAR ──
    //
    // Fixed height with the row centred inside it, rather than `add_space` either side of an
    // auto-sized row. A panel that sizes to its tallest child cannot be padded symmetrically: the
    // brand mark is 26 px while the title's line box is ~28, so equal spacers still produced 6 px
    // above and 13 px below. Pinning the height and centring makes the spacing symmetric by
    // construction, at any type scale.
    egui::TopBottomPanel::top("cmdbar")
        .exact_height(theme::bar_h())
        .frame(theme::bar_frame())
        .show(ctx, |ui| {
        // `horizontal_centered` lays out left-to-right AND takes the panel's full height, so every
        // child — including the right-aligned group that nests its own layout — centres against the
        // whole band instead of against whatever row height earlier items happened to set.
        ui.horizontal_centered(|ui| {
            theme::brand_mark(ui);
            ui.add_space(2.0);
            ui.label(theme::poster_text("SAB WORKSHOP", 18.0, theme::TX));
            ui.label(theme::data_text("ASSET WORKBENCH", 9.0, theme::FAINT));
            ui.separator();
            // On Inspect the subject is the model; on an editor page the subject is the MOD — it is
            // the document those pages are all editing, so it belongs where a filename would be.
            match page.editor() {
                None => {
                    ui.label(theme::data_text(model_name, 11.5, theme::RED));
                }
                Some(_) => {
                    let (name, out) = ed.mod_chip();
                    ui.label(theme::disp_text("MOD", 10.0, theme::FAINT));
                    ui.label(theme::data_text(name, 11.5, theme::RED));
                    ui.label(theme::data_text(out, 10.0, theme::COLD));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match page.editor() {
                    None => {
                        theme::chip(ui, "orbit", true, None);
                        theme::chip(
                            ui,
                            if *show_textures { "textured" } else { "untextured" },
                            *show_textures,
                            None,
                        );
                    }
                    Some(_) => {
                        let n = ed.total_pending();
                        theme::chip(ui, &format!("{n} staged"), n > 0, None);
                        // Not "sources read-only" any more: an edit of a retail string has to patch
                        // the base GameText.dlg, because a DLC slot cannot override a hash the base
                        // map already holds. Retail is kept as .sabbak and Unpublish restores it —
                        // reversible, which is the promise this chip can still honestly make.
                        theme::chip(ui, "retail backed up", false, None);
                    }
                }
                });
        });
    });

    // Non-fatal load failures — visible but non-blocking.
    if !errors.is_empty() {
        egui::TopBottomPanel::top("errors").show(ctx, |ui| {
            for e in errors {
                ui.colored_label(theme::RED, format!("load warning: {e}"));
            }
        });
    }

    // ── STATUS BAR ──
    egui::TopBottomPanel::bottom("status")
        .exact_height(theme::status_h())
        .frame(theme::bar_frame())
        .show(ctx, |ui| {
        // `horizontal_centered` lays out left-to-right AND takes the panel's full height, so every
        // child — including the right-aligned group that nests its own layout — centres against the
        // whole band instead of against whatever row height earlier items happened to set.
        ui.horizontal_centered(|ui| {
            let (ok, msg) = if have_pack { (theme::EMBER, "ready") } else { (theme::RED, "no pack") };
            theme::status_dot(ui, msg, ok);
            ui.separator();
            ui.label(theme::data_text(
                format!("{} verts · {} tris · {} bones", mesh_stats.0, mesh_stats.1, mesh_stats.2),
                10.0,
                theme::DIM,
            ));
            ui.separator();
            // Each page reports the fact IT is about; the status bar is not a fixed readout.
            ui.label(theme::data_text(
                match page.editor() {
                    None => format!("{textured}/{} submeshes textured", mats.submeshes.len()),
                    Some(ep) => ed.status(ep),
                },
                10.0,
                theme::DIM,
            ));
            if let Some((msg, is_err)) = load_status {
                ui.separator();
                ui.label(
                    theme::data_text(msg, 10.0, if *is_err { theme::RED } else { theme::EMBER }),
                );
            }
        });
    });

    // ── VERB BAR ──
    //
    // The one thing this page is FOR, given its own band. Export used to be a 10pt button wedged
    // between two monospace readouts in the 24pt status bar, which is a place for facts, not verbs —
    // it was unfindable, and it vanished entirely whenever the model had no bones. Lifting it into a
    // dedicated bar (the shape `mercs2_workshop` settled on) gives it room for the shortcut, the
    // destination, and the caveat that the bundle would come out untextured.
    if page.editor().is_none() {
        // Exact height + `horizontal_centered`, NOT `add_space` either side of an auto-sized row.
        // A panel that sizes to its tallest child cannot be padded symmetrically — the button's box
        // and the labels' line boxes differ, so equal spacers still landed more air under the button
        // than over it. `bar_frame` carries no vertical margin for exactly this reason: the centring
        // owns that axis. Same construction as the command and status bars.
        egui::TopBottomPanel::bottom("verbbar")
            .exact_height(theme::verb_h())
            .frame(theme::bar_frame())
            .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                let busy = exp.busy.is_some();
                // The shortcut rides IN the label, the way the Mercs2 verb bar states its verbs —
                // one element instead of a button plus a chip arguing for the same attention.
                let label = match &exp.busy {
                    Some(l) => format!("exporting {l}…"),
                    None => "Export  F10".into(),
                };
                let r = theme::action_button(ui, &label, exp.ready && !busy);
                if r.clicked() {
                    *exp.request = true;
                }
                // The label says the verb; the tooltip says what lands on disk. Spelling the whole
                // bundle out on the button made it the widest thing in the app.
                r.on_hover_text(
                    "Export bundle — writes model.glb (skinned, one primitive per submesh), \
                     textures/*.png, the verbatim raw/ source records, and manifest.json (the \
                     reassembly map).",
                );
                ui.separator();

                // Where it lands, and how to change that. Shown as the tail only: the absolute path
                // is ~100 characters and swallowed the whole bar, which buried the very button this
                // panel exists to show off. The full path is one hover away.
                ui.label(theme::data_text("→", 11.0, theme::FAINT));
                if ui
                    .add(
                        egui::Label::new(theme::data_text(short_path(&exp.dest), 10.0, theme::COLD))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("{}\n\nClick to choose where bundles are written", exp.dest))
                    .clicked()
                {
                    *exp.pick_dir = true;
                }

                if exp.ready && exp.tex_pending {
                    ui.separator();
                    ui.label(theme::data_text(
                        "textures still resolving — exporting now writes an untextured bundle",
                        10.0,
                        theme::EMBER,
                    ));
                } else if !exp.ready {
                    ui.separator();
                    ui.label(theme::data_text("load a model to export", 10.0, theme::FAINT));
                }
            });
        });

        // F10 anywhere on the page, matching the label on the button. Not while a text field has
        // focus, for the same reason the 1-5 page keys are gated.
        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.key_pressed(egui::Key::F10))
            && exp.ready
            && exp.busy.is_none()
        {
            *exp.request = true;
        }
    }

    // ── EXPORT PROGRESS ──
    //
    // Deliberately NOT modal: the export runs on a worker against its own copy of the data, so
    // there is nothing to protect the app from — orbiting the model while it writes is fine.
    if let Some(l) = &exp.busy {
        ctx.request_repaint(); // keep the spinner turning without pointer input
        egui::Window::new(theme::disp_text("EXPORTING", 12.0, theme::TX))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(15.0).color(theme::EMBER));
                    ui.add_space(6.0);
                    ui.label(theme::data_text(format!("{l} — assembling, decoding skins…"), 11.0, theme::TX));
                });
                ui.add_space(3.0);
                ui.label(theme::data_text(
                    "Decoding a character's skins takes a few seconds. The viewport stays live.",
                    10.0,
                    theme::FAINT,
                ));
            });
    }

    // ── RAIL ──
    // Declared before the navigator so it owns the far-left edge, and after the status bar so it
    // does not eat the full window height.
    egui::SidePanel::left("rail")
        .exact_width(theme::RAIL_W)
        .resizable(false)
        .frame(egui::Frame::none().fill(theme::G0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            ui.add_space(4.0);
            for (i, p) in Page::ALL.iter().enumerate() {
                // Inspect meters coverage; an editor page meters WORK — how many edits are staged on
                // it. Same widget, and on an editor the count is the more useful fact.
                let (done, total) = match p {
                    Page::Inspect => (textured, mats.submeshes.len()),
                    _ => (0, 0),
                };
                let r = theme::rail_button(
                    ui,
                    &format!("{}", i + 1),
                    p.glyph(),
                    p.label(),
                    *page == *p,
                    done,
                    total,
                );
                // staged-edit badge, top-right of the button
                if let Some(ep) = p.editor() {
                    let n = ed.pending(ep);
                    if n > 0 {
                        let b = egui::Rect::from_min_size(
                            r.rect.right_top() + egui::vec2(-19.0, 5.0),
                            egui::vec2(14.0, 14.0),
                        );
                        ui.painter().rect_filled(b, egui::Rounding::ZERO, theme::EMBER);
                        ui.painter().text(
                            b.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{n}"),
                            egui::FontId::new(9.0, theme::data()),
                            theme::G0,
                        );
                    }
                }
                if r.clicked() {
                    *page = *p;
                }
                let note = match p.editor() {
                    None => format!("{textured}/{} submeshes textured", mats.submeshes.len()),
                    Some(ep) => ed.status(ep),
                };
                r.on_hover_text(format!("{}  ({})", p.label(), note));
            }
        });

    // ── SETTINGS ──
    // A single column, not the three-column shell: there is no navigator (nothing to browse) and no
    // changelist (these are app preferences, not mod edits — they do not publish).
    if *page == Page::Settings {
        settings_page(ctx, settings);
        return;
    }

    // ── EDITOR PAGES ──
    // Same three-column shell as Inspect — navigator, work surface, right rail — because these are
    // views onto one document, not standalone file editors. The right rail is the CHANGELIST, and it
    // is present on every editor page: a mod spans all three, so the record of it has to as well.
    if let Some(ed_page) = page.editor() {
        egui::SidePanel::left("ed_nav")
            .resizable(true)
            .default_width(320.0)
            .width_range(260.0..=480.0)
            .show(ctx, |ui| ed.nav(ui, ed_page));
        egui::SidePanel::right("ed_side")
            .resizable(true)
            .default_width(350.0)
            .width_range(290.0..=480.0)
            .show(ctx, |ui| ed.side(ui, ed_page));
        ed.central(ctx, ed_page);
        return;
    }

    // ── TRANSPORT ──
    egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
        ui.add_space(4.0);


        match current {
            None => {
                ui.label(
                    egui::RichText::new("No clip loaded — pick one from the navigator. Showing bind pose.")
                        .color(theme::DIM),
                );
            }
            Some(clip) => {
                ui.horizontal(|ui| {
                    ui.label(theme::data_text(&clip.name, 11.5, theme::RED));
                    ui.separator();
                    let frame = if clip.frame_duration > 0.0 {
                        (playback.time / clip.frame_duration).round() as i64
                    } else {
                        0
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.3}s · {} frames · frame {}/{}",
                            clip.duration,
                            clip.frame_count,
                            frame.max(0),
                            clip.frame_count.saturating_sub(1)
                        ))
                        .monospace()
                        .size(11.0)
                        .color(theme::DIM),
                    );
                });
                ui.horizontal(|ui| {
                    let transport = if playback.playing {
                        format!("{} Pause", theme::sym::PAUSE)
                    } else {
                        format!("{} Play", theme::sym::PLAY)
                    };
                    if theme::primary_button(ui, &transport, true).clicked() {
                        playback.playing = !playback.playing;
                    }
                    if theme::pill(ui, "loop", playback.looping).clicked() {
                        playback.looping = !playback.looping;
                    }
                    if theme::pill(ui, "grid", *show_grid).clicked() {
                        *show_grid = !*show_grid;
                    }
                    if theme::pill(ui, "textures", *show_textures).clicked() {
                        *show_textures = !*show_textures;
                    }
                    if theme::pill(ui, "lock root", *lock_root).clicked() {
                        *lock_root = !*lock_root;
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("speed").color(theme::DIM).size(11.0));
                    ui.add(egui::Slider::new(&mut playback.speed, 0.0..=3.0).fixed_decimals(2));
                    if ui.button("Reset").clicked() {
                        playback.time = 0.0;
                    }
                });
                let dur = clip.duration.max(0.0001);
                let mut t = playback.time.clamp(0.0, dur);
                if ui
                    .add(egui::Slider::new(&mut t, 0.0..=dur).text("time (s)").fixed_decimals(3))
                    .changed()
                {
                    playback.time = t;
                    playback.playing = false; // scrubbing pauses
                }
            }
        }
        ui.add_space(4.0);
    });

    // ── NAVIGATOR (left): the clip list ──
    egui::SidePanel::left("navigator")
        .resizable(true)
        .default_width(300.0)
        // Hard clamp: a resizable panel sizes to its content's minimum, so ANY row whose width is
        // derived from `available_width()` can feed back and walk the panel across the screen. The
        // rows below are feedback-free, but this bounds the blast radius of a future one.
        .width_range(240.0..=460.0)
        .show(ctx, |ui| {
            theme::eyebrow(
                ui,
                match *page {
                    Page::Inspect => "Browser",
                    _ => "",
                },
            );
            ui.add_space(4.0);
            // Category selector — Inspect browses the PACK, not just clips. The other pages each
            // have exactly one subject, so they get no selector.
            if *page == Page::Inspect {
                ui.horizontal(|ui| {
                    for t in NavTab::ALL {
                        if theme::pill(ui, t.label(), *nav.tab == t).clicked() {
                            *nav.tab = t;
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                // Right-to-left: the clear button is placed first (at the right), then the field fills EXACTLY
                // the remainder — so total content == available. Sizing the field from
                // `available_width() - k` and then adding the button overflows by (button - k) every
                // frame, which is what made this panel creep rightward.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(theme::sym::NO).clicked() {
                        nav.search.clear();
                    }
                    ui.add_sized(
                        [ui.available_width(), 20.0],
                        egui::TextEdit::singleline(nav.search).hint_text("filter by name"),
                    );
                });
            });
            let needle = nav.search.to_ascii_lowercase();
            ui.add_space(2.0);

            // Materials and Rig have their own subjects; Inspect's pills pick between models and
            // clips. Textures lists the pool the contact sheet is showing.
            match (*page, *nav.tab) {
                (Page::Inspect, NavTab::Models) if nav.models_loading => {
                    // The model list is still streaming from the megapack — show a spinner, not an
                    // empty tree.
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(theme::data_text("loading from megapack…", 11.0, theme::DIM));
                    });
                }
                (Page::Inspect, NavTab::Models) => {
                    // List assembled ASSETS (from GameTemplates), grouped by category. A user override
                    // wins over the template-derived default; nothing is hidden (Ungrouped catches the
                    // rest). Selecting an asset loads its primary part and reveals its parts (inspector).
                    let mut by_cat: std::collections::HashMap<&str, Vec<usize>> =
                        std::collections::HashMap::new();
                    let mut shown = 0usize;
                    for (i, a) in nav.browse_assets.iter().enumerate() {
                        if !needle.is_empty() && !a.name.to_ascii_lowercase().contains(&needle) {
                            continue;
                        }
                        by_cat.entry(nav.groups.category(&a.name, a.category)).or_default().push(i);
                        shown += 1;
                    }
                    ui.label(theme::data_text(
                        format!("{shown} of {} asset(s)", nav.browse_assets.len()),
                        10.0,
                        theme::FAINT,
                    ));
                    ui.label(theme::data_text(
                        "click to load · parts on the right · right-click to regroup",
                        9.0,
                        theme::FAINT,
                    ));
                    ui.separator();
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for cat in crate::models::CATEGORIES {
                            let Some(idxs) = by_cat.get(*cat) else { continue };
                            if idxs.is_empty() {
                                continue;
                            }
                            let open = !needle.is_empty() || *cat == "Characters";
                            egui::CollapsingHeader::new(
                                egui::RichText::new(format!("{cat}  ({})", idxs.len()))
                                    .color(theme::TX)
                                    .size(11.0),
                            )
                            .id_source(("modelcat", cat))
                            .default_open(open)
                            .show(ui, |ui| {
                                for &i in idxs {
                                    let a = &nav.browse_assets[i];
                                    let sel = *nav.sel_asset == Some(i);
                                    let label = if a.parts.len() > 1 {
                                        format!("{}  ({} parts)", a.name, a.parts.len())
                                    } else {
                                        a.name.clone()
                                    };
                                    let r = ui.selectable_label(sel, label);
                                    if r.clicked() {
                                        *nav.sel_asset = Some(i);
                                        *nav.pending_model = Some(a.assembly());
                                    }
                                    r.context_menu(|ui| {
                                        // Load-then-export: the bundle carries the texture
                                        // bindings, and those only come into existence once this
                                        // asset's resolve lands. `defer` fires the export then.
                                        if ui.button("Export bundle").clicked() {
                                            *nav.sel_asset = Some(i);
                                            *nav.pending_model = Some(a.assembly());
                                            *nav.export_after_load = true;
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        ui.label(theme::data_text("Move to group", 10.0, theme::FAINT));
                                        for c in crate::models::CATEGORIES {
                                            if ui.button(*c).clicked() {
                                                *nav.pending_group = Some((i, (*c).to_string()));
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                }
                            });
                        }
                    });
                }
                (Page::Inspect, NavTab::Animations) => {
                    if theme::pill(ui, "show all (incl. non-rig)", *show_all).clicked() {
                        *show_all = !*show_all;
                    }
                    let rows: Vec<usize> = if *show_all {
                        (0..catalog.clips.len()).collect()
                    } else {
                        playable_rows.to_vec()
                    };
                    let matches: Vec<usize> = rows
                        .into_iter()
                        .filter(|&i| {
                            needle.is_empty() || catalog.clips[i].name.to_ascii_lowercase().contains(&needle)
                        })
                        .collect();
                    ui.label(theme::data_text(format!("{} clip(s)", matches.len()), 10.0, theme::FAINT));
                    if !have_pack {
                        ui.colored_label(theme::RED, "Animations.pack not loaded — bind pose only.");
                    }
                    if !rig_ok {
                        ui.colored_label(
                            theme::RED,
                            "Loaded model's rig differs from the clip catalog — clips disabled.",
                        );
                    }
                    ui.separator();
                    ui.add_space(4.0);
                    let cur_name = current.as_ref().map(|c| c.name.clone());
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                        ui,
                        18.0,
                        matches.len(),
                        |ui, range| {
                            for r in range {
                                let i = matches[r];
                                let c = &catalog.clips[i];
                                let selected = cur_name.as_deref() == Some(c.name.as_str());
                                let label = if c.playable {
                                    c.name.clone()
                                } else {
                                    format!("{}  (non-rig)", c.name)
                                };
                                if ui.add_enabled(rig_ok, egui::SelectableLabel::new(selected, label)).clicked() {
                                    *pending_load = Some(i);
                                }
                            }
                        },
                    );
                }
                // Editor pages return before the navigator; only Inspect reaches this match.
                (_, _) => {}
            }
        });

    // ── INSPECTOR (right): character + materials + clip ──
    egui::SidePanel::right("inspector")
        .resizable(true)
        .default_width(340.0)
        .width_range(280.0..=460.0) // see the navigator's note on content-width feedback
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {

                // ---- Inspect: what you are looking at ----
                if *page == Page::Inspect {
                    theme::card(ui, "Character", None, |ui| {
                        theme::kv(ui, "vertices", egui::RichText::new(mesh_stats.0.to_string()));
                        theme::kv(ui, "triangles", egui::RichText::new(mesh_stats.1.to_string()));
                        theme::kv(ui, "bones", egui::RichText::new(mesh_stats.2.to_string()));
                        theme::kv(ui, "clips", egui::RichText::new(catalog.clips.len().to_string()));
                    });
                    // Parts of the selected asset — each is loadable (inspectable) individually.
                    if let Some(a) = nav.sel_asset.and_then(|ai| nav.browse_assets.get(ai)) {
                        let badge = format!("{}", a.parts.len());
                        theme::card(ui, "Parts", Some(&badge), |ui| {
                            ui.label(theme::data_text(&a.name, 10.5, theme::RED));
                            ui.add_space(3.0);
                            for p in &a.parts {
                                let loaded = p.name.eq_ignore_ascii_case(nav.current_model);
                                let r = ui.selectable_label(
                                    loaded,
                                    egui::RichText::new(format!("{}   {}", p.label, p.name))
                                        .monospace()
                                        .size(10.5),
                                );
                                if r.clicked() {
                                    *nav.pending_model = Some(vec![p.mesh_index]);
                                }
                            }
                        });
                    }
                    if let Some(clip) = current {
                        theme::card(ui, "Clip", None, |ui| {
                            theme::kv(
                                ui,
                                "name",
                                egui::RichText::new(&clip.name).color(theme::RED),
                            );
                            theme::kv(ui, "duration", egui::RichText::new(format!("{:.2} s", clip.duration)));
                            theme::kv(ui, "tracks", egui::RichText::new(clip.track_to_bone.len().to_string()));
                        });
                    }
                    return;
                }

                // ---- Materials: the bind ledger ----
                let badge = format!("{bound}/{} bound", mats.submeshes.len());
                theme::section(ui, "Materials", Some(&badge), true, |ui| {
                    if mats.assets.is_empty() {
                        ui.colored_label(theme::RED, "No textures resolved — check --megapack / --char.");
                        return;
                    }
                    ui.label(
                        egui::RichText::new(
                            "Auto-seeded by body part. WSAO (which would name each material) isn't in \
                             the PC build, so fix any wrong slot here — saved next to the mesh.",
                        )
                        .color(theme::FAINT)
                        .size(10.5),
                    );
                    ui.add_space(6.0);
                    for (i, sm) in mats.submeshes.iter().enumerate() {
                        let assigned = mats.submesh_tex[i];
                        let cur = assigned.map(|ai| mats.assets[ai].name.as_str()).unwrap_or("(none)");
                        // Will to Fight, as information design: a submesh with a texture bound has
                        // had its colour returned; one without is still occupied, and stays grey.
                        // This is the resting state — nothing toggles it.
                        let (fill, border) = if assigned.is_some() {
                            (theme::EMBER_SOFT, theme::EMBER_DK)
                        } else {
                            (theme::G0, theme::LINE)
                        };
                        theme::row_chip(ui, fill, border, |ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let ix = if assigned.is_some() { theme::EMBER } else { theme::FAINT };
                                    ui.label(theme::data_text(format!("{i:02}"), 10.0, ix));
                                    ui.label(theme::data_text(
                                        format!("{} tris", sm.index_count / 3),
                                        10.0,
                                        theme::DIM,
                                    ));
                                    // The prim's material hashes — what WSAO would have resolved.
                                    let hashes: Vec<String> =
                                        sm.materials.iter().map(|m| format!("{m:08X}")).collect();
                                    ui.label(theme::data_text(hashes.join(" "), 9.0, theme::FAINT))
                                        .on_hover_text("prim material hashes (pandemic_hash of a WSAO material name)");
                                    // A binding never pretends to be a resolution: say how we got it.
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let (label, kind) = match mats.submesh_prov[i] {
                                                resolve::Prov::Bound => ("bound", theme::Badge::Lit),
                                                resolve::Prov::Seeded => {
                                                    ("seeded", theme::Badge::Outline)
                                                }
                                                resolve::Prov::Unresolved => {
                                                    ("unresolved", theme::Badge::Muted)
                                                }
                                            };
                                            theme::badge(ui, label, kind);
                                        },
                                    );
                                });
                                egui::ComboBox::from_id_source(("texpick", i))
                                    .selected_text(egui::RichText::new(cur).size(11.0))
                                    // Never wider than what's actually available (a `.max(k)` floor
                                    // here would overflow the row and creep the panel — see above).
                                    .width((ui.available_width() - 8.0).max(0.0))
                                    .show_ui(ui, |ui| {
                                        if ui.selectable_label(assigned.is_none(), "(none)").clicked() {
                                            *mats.pending_tex = Some((i, None));
                                        }
                                        for (ai, a) in mats.assets.iter().enumerate() {
                                            let lbl = format!("{}  · {}", a.name, a.role.label());
                                            if ui.selectable_label(assigned == Some(ai), lbl).clicked() {
                                                *mats.pending_tex = Some((i, Some(ai)));
                                            }
                                        }
                                    });
                                if let Some(ai) = assigned {
                                    let a = &mats.assets[ai];
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{}x{} · {} · {}",
                                            a.width,
                                            a.height,
                                            a.format,
                                            a.role.label()
                                        ))
                                        .monospace()
                                        .size(9.5)
                                        .color(theme::FAINT),
                                    );
                                }
                            });
                        });
                    }
                });

                if let Some(clip) = current {
                    theme::card(ui, "Clip", None, |ui| {
                        theme::kv(ui, "name", egui::RichText::new(&clip.name));
                        theme::kv(ui, "duration", egui::RichText::new(format!("{:.3}s", clip.duration)));
                        theme::kv(ui, "frames", egui::RichText::new(clip.frame_count.to_string()));
                        theme::kv(
                            ui,
                            "tracks",
                            egui::RichText::new(clip.anim.num_transform_tracks.to_string()),
                        );
                    });
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verb bar shows the destination's tail, not a 100-character absolute path.
    #[test]
    fn short_path_keeps_the_last_two_components() {
        assert_eq!(
            short_path(r"C:\Users\Shadow\Desktop\repo\tools\sab_workshop\workshop_export\Sean"),
            "…/workshop_export/Sean"
        );
        assert_eq!(short_path("/home/x/workshop_export/Sean"), "…/workshop_export/Sean");
        // Too short to trim: hand it back rather than inventing an ellipsis.
        assert_eq!(short_path("Sean"), "Sean");
    }
}
