//! Application: open a build, run the winit event loop, draw the UI, drive playback.
//!
//! Everything the app shows comes through one [`GameSource`], so retail and the 2008 prototype
//! behave the same:
//!   open           on a worker: the source, its template-assembled assets, the startup model (the
//!                  build's player). Then the model's textures, from the build's material data.
//!   click a model  on a worker: assemble its parts; then resolve its textures.
//!   click a clip   decode it and bind its tracks to the rig on screen by bone name hash.
//!   each frame     sample the clip -> per-track local transforms -> `skinning::posed` -> joint
//!                  matrices uploaded to the GPU; the vertex shader skins the mesh.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::EventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowBuilder;

use crate::assets::{Asset, Assembly};
use crate::boot::Model;
use crate::bundle;
use crate::camera::OrbitCamera;
use crate::clip::{self, AnimClip};
use crate::formats::{self, Bone, Smsh, SubMesh};
use crate::gui::theme;
use crate::render::Renderer;
use crate::resolve::{self, ModelTex, Prov, SubmeshBinding, TexAsset};
use crate::skinning;
use crate::source::{ClipRow, GameSource, PartRange};
use crate::sources::retail_pc::RetailClip;
use crate::Config;

/// A clip decoded and bound to the rig on screen.
struct LoadedClip {
    clip: Box<dyn AnimClip>,
    /// Each track's rig bone (`-1`: none), from the clip's bone hashes.
    track_to_bone: Vec<i32>,
    name: String,
    duration: f32,
    frame_duration: f32,
    frame_count: usize,
}

impl LoadedClip {
    fn load(src: &dyn GameSource, index: usize, rig_hashes: &[u32]) -> Result<LoadedClip, String> {
        let clip = src.load_clip(index)?;
        let name = src.clips()[index].name.clone();
        let duration = clip.duration();
        let frame_count = clip.num_frames().max(1);
        let frame_duration = if frame_count > 1 { duration / (frame_count - 1) as f32 } else { duration };
        let track_to_bone = clip::bind_to_rig(clip.as_ref(), rig_hashes);
        Ok(LoadedClip { clip, track_to_bone, name, duration, frame_duration, frame_count })
    }

    /// Tracks bound to a rig bone, of all the clip's bound tracks.
    fn bound(&self) -> (usize, usize) {
        let wanted = self.clip.bind_hashes().iter().filter(|h| **h != 0).count();
        (self.track_to_bone.iter().filter(|b| **b >= 0).count(), wanted)
    }
}

/// Playback state.
struct Playback {
    time: f32,
    playing: bool,
    looping: bool,
    speed: f32,
}

/// A single degenerate triangle standing in for "no model yet". wgpu rejects zero-length vertex and
/// index buffers, so the empty scene still needs *some* geometry; it is never drawn.
fn placeholder_mesh() -> Smsh {
    Smsh {
        positions: vec![[0.0; 3]; 3],
        normals: vec![[0.0, 1.0, 0.0]; 3],
        uvs: vec![[0.0; 2]; 3],
        joints: vec![[0; 4]; 3],
        weights: vec![[0.0; 4]; 3],
        indices: vec![0, 1, 2],
        prims: Vec::new(),
    }
}

/// The model on screen and everything derived from it.
#[derive(Default)]
struct View {
    /// A model is on screen (the placeholder is not).
    loaded: bool,
    model_name: String,
    /// The mesh items it was assembled from; empty for a model from loose files.
    picks: Vec<(usize, u8)>,
    notes: Vec<String>,
    part_ranges: Vec<PartRange>,
    uvless: Vec<(u32, u32)>,
    skel: Vec<Bone>,
    rig_hashes: Vec<u32>,
    bone_depth: Vec<usize>,
    submeshes: Vec<SubMesh>,
    /// vertices, triangles, bones
    mesh_stats: (usize, usize, usize),
    /// Triangles and parts per level of detail.
    levels: Vec<(u8, usize, u32)>,
    assets: Vec<TexAsset>,
    submesh_tex: Vec<SubmeshBinding>,
    submesh_prov: Vec<Prov>,
    /// Where this model's material assignments persist.
    tex_profile: String,
    /// The clip rows whose every bound track names a bone of this rig.
    playable: Vec<usize>,
    /// The asset's damage parts, and the damage states its geometry draws in.
    damage: Vec<(u32, i32)>,
    states: Vec<i32>,
}

impl View {
    /// Put `m` on screen in bind pose, untextured.
    fn show(&mut self, m: Model, profile: String, renderer: &mut Renderer, camera: &mut OrbitCamera, clips: &[ClipRow]) {
        let nbones = m.lm.bones.len();
        renderer.set_mesh(&m.lm.mesh, nbones);
        renderer.update_joints(&skinning::bind_pose(&m.lm.bones));
        renderer.damage_state = 0;
        self.submeshes = renderer.submeshes().to_vec();
        let mut states: Vec<i32> = self.submeshes.iter().filter_map(|s| s.damage).collect();
        states.sort_unstable();
        states.dedup();
        self.states = states;
        self.damage = m.damage;
        self.mesh_stats = (m.lm.mesh.positions.len(), m.lm.mesh.indices.len() / 3, nbones);
        let mut levels: std::collections::BTreeMap<u8, (usize, u32)> = Default::default();
        for r in &m.lm.part_ranges {
            let e = levels.entry(r.lod).or_default();
            e.0 += 1;
            e.1 += r.index_count / 3;
        }
        self.levels = levels.into_iter().map(|(l, (p, t))| (l, p, t)).collect();
        let (c, r) = bounds(&m.lm.mesh);
        *camera = OrbitCamera::framing(c, r);
        self.bone_depth = bone_depths(&m.lm.bones);
        self.skel = m.lm.bones;
        self.rig_hashes = m.lm.bone_hashes;
        self.part_ranges = m.lm.part_ranges;
        self.uvless = m.lm.uvless;
        self.model_name = m.name;
        self.picks = m.picks;
        self.notes = m.notes;
        self.assets = Vec::new();
        self.submesh_tex = vec![SubmeshBinding::default(); self.submeshes.len()];
        self.submesh_prov = vec![Prov::Unresolved; self.submeshes.len()];
        self.tex_profile = profile;
        let rig: HashSet<u32> = self.rig_hashes.iter().copied().collect();
        self.playable = clips.iter().enumerate().filter(|(_, c)| c.bones.iter().all(|h| *h == 0 || rig.contains(h))).map(|(i, _)| i).collect();
        self.loaded = true;
    }

    /// Back to the empty scene.
    fn clear(&mut self, renderer: &mut Renderer) {
        renderer.set_mesh(&placeholder_mesh(), 0);
        *self = View::default();
    }

    /// Apply a texture resolve: the build's bindings, then any saved by the user. Returns a summary.
    fn apply_textures(&mut self, mt: ModelTex, renderer: &mut Renderer) -> Result<String, String> {
        self.assets = mt.assets;
        self.submesh_tex = mt.submesh_tex;
        self.submesh_prov = mt.submesh_prov;
        let mut counts = [0usize; SubmeshBinding::NUM_SLOTS];
        for (i, slot, tex) in &mt.decoded {
            upload(renderer, *i, *slot, Some(tex));
            counts[*slot] += 1;
        }
        // A hand-authored sidecar is a DECISION and outranks what the material data answered.
        let mut sidecar = 0usize;
        if let Some((st, sp)) = resolve::load_sidecar(&self.tex_profile, self.submeshes.len(), &self.assets) {
            for i in 0..self.submeshes.len() {
                if sp[i] == Prov::Unresolved {
                    continue;
                }
                sidecar += 1;
                self.submesh_tex[i] = st[i];
                self.submesh_prov[i] = sp[i];
                for slot in [SubmeshBinding::DIFFUSE, SubmeshBinding::SPEC, SubmeshBinding::NORMAL] {
                    match st[i].slot(slot) {
                        Some(ai) => {
                            let tex = self.assets[ai].decode().map_err(|e| format!("texture {}: {e}", self.assets[ai].name))?;
                            upload(renderer, i, slot, Some(&tex));
                        }
                        None => upload(renderer, i, slot, None),
                    }
                }
            }
        }
        Ok(format!(
            "{}: {} diffuse, {} spec, {} normal maps bound to {} submeshes{}",
            self.model_name,
            counts[SubmeshBinding::DIFFUSE],
            counts[SubmeshBinding::SPEC],
            counts[SubmeshBinding::NORMAL],
            self.submeshes.len(),
            if sidecar > 0 { format!(", {sidecar} from saved bindings") } else { String::new() }
        ))
    }
}

/// Route a texture into the renderer slot the shader samples it in. The mask slot is carried in the
/// Materials panel and the bundle; the shader does not sample it.
fn upload(renderer: &mut Renderer, i: usize, slot: usize, tex: Option<&crate::dtex::CpuTexture>) {
    match (slot, tex) {
        (SubmeshBinding::DIFFUSE, Some(t)) => renderer.set_submesh_texture(i, t),
        (SubmeshBinding::DIFFUSE, None) => renderer.clear_submesh_texture(i),
        (SubmeshBinding::SPEC, Some(t)) => renderer.set_submesh_spec(i, t),
        (SubmeshBinding::SPEC, None) => renderer.clear_submesh_spec(i),
        (SubmeshBinding::NORMAL, Some(t)) => renderer.set_submesh_normal(i, t),
        (SubmeshBinding::NORMAL, None) => renderer.clear_submesh_normal(i),
        _ => {}
    }
}

/// Results from the workers, applied on the main thread (wgpu uploads stay there).
enum BgMsg {
    Opened { source: Arc<dyn GameSource>, assets: Vec<Asset>, boot: Result<Model, String> },
    OpenFailed(String),
    Model { req: u64, result: Result<Model, String> },
    Textures { req: u64, result: Result<ModelTex, String> },
}

/// Open the build in `cfg` on a worker: the source, its assets, the startup model.
fn spawn_open(cfg: Config, tx: Sender<BgMsg>) {
    std::thread::spawn(move || {
        let opened = (|| -> Result<BgMsg, String> {
            let source: Arc<dyn GameSource> = Arc::from(crate::source::open_source(std::path::Path::new(&cfg.game_dir), &cfg.overrides)?);
            let assets = crate::assets::build(source.as_ref());
            let boot = crate::boot::load(&cfg, source.as_ref(), &assets);
            Ok(BgMsg::Opened { source, assets, boot })
        })();
        let _ = tx.send(opened.unwrap_or_else(BgMsg::OpenFailed));
    });
}

/// Assemble `asm` on a worker.
fn spawn_assemble(src: Arc<dyn GameSource>, name: String, asm: Assembly, req: u64, tx: Sender<BgMsg>) {
    std::thread::spawn(move || {
        let _ = tx.send(BgMsg::Model { req, result: crate::boot::assemble(src.as_ref(), &name, &asm) });
    });
}

/// Resolve a model's textures on a worker.
fn spawn_tex_resolve(src: Arc<dyn GameSource>, view: &View, req: u64, tx: Sender<BgMsg>) {
    let (parts, submeshes, uvless) = (view.part_ranges.clone(), view.submeshes.clone(), view.uvless.clone());
    std::thread::spawn(move || {
        let _ = tx.send(BgMsg::Textures { req, result: src.resolve_textures(&parts, &submeshes, &uvless) });
    });
}

/// Where a model's material bindings persist: a `--mesh` file keeps them beside itself; a model from
/// the build, beside the settings, by build and name.
fn profile_for(cfg: &Config, src: &dyn GameSource, model: &Model) -> String {
    match (&cfg.mesh, model.picks.is_empty()) {
        (Some(path), true) => path.clone(),
        _ => resolve::profile_path(&format!("{}{}", src.version().profile_prefix(), model.name)),
    }
}

/// The status line for a freshly shown model.
fn model_status(view: &View) -> (String, bool) {
    let levels = view.levels.iter().map(|(l, p, t)| format!("LOD{l} {p} parts / {t} tris")).collect::<Vec<_>>().join(", ");
    let levels = if levels.is_empty() { format!("{} tris", view.mesh_stats.1) } else { levels };
    let notes = if view.notes.is_empty() { String::new() } else { format!(" — {} part(s) missing: {}", view.notes.len(), view.notes.join("; ")) };
    (format!("{}: {levels}, {} bones{notes}", view.model_name, view.mesh_stats.2), !view.notes.is_empty())
}

pub fn run(mut cfg: Config) {
    let mut errors: Vec<String> = Vec::new();
    let have_install = cfg.settings.is_valid();

    let event_loop = match EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: event loop: {e}");
            return;
        }
    };
    let window = match WindowBuilder::new()
        .with_title("sab_workshop — The Saboteur asset workbench")
        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
    {
        Ok(w) => Arc::new(w),
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: window: {e}");
            return;
        }
    };
    let mut renderer = match pollster::block_on(Renderer::new(window.clone(), &placeholder_mesh(), 0)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[sab_workshop] FATAL: renderer init: {e}");
            return;
        }
    };
    let mut gui = crate::gui::Gui::new(renderer.device(), renderer.surface_format(), &window);

    let (bg_tx, bg_rx): (Sender<BgMsg>, Receiver<BgMsg>) = std::sync::mpsc::channel();
    let mut source: Option<Arc<dyn GameSource>> = None;
    let mut browse_assets: Vec<Asset> = Vec::new();
    let mut opening = have_install;
    if have_install {
        spawn_open(cfg.clone(), bg_tx.clone());
    }

    let mut view = View::default();
    let mut camera = OrbitCamera::framing(Vec3::ZERO, 1.0);
    let mut show_all = false;
    let mut current: Option<LoadedClip> = None;
    let mut playback = Playback { time: 0.0, playing: true, looping: true, speed: 1.0 };
    let mut pending_clip: Option<usize> = None;
    let mut pending_tex: Option<(usize, u8, Option<usize>)> = None;
    let mut pending_save = false;
    let mut lock_root = true;
    let mut nav_tab = NavTab::Models;
    let mut page = if have_install { Page::Inspect } else { Page::Settings };
    let mut editor_state: Option<crate::editor::Editor> = None;
    let mut settings_ctx = SettingsCtx {
        dir_edit: cfg.settings.game_dir.clone(),
        launched_with: cfg.settings.game_dir.clone(),
        s: cfg.settings.clone(),
        note: None,
        needs_install: !have_install,
        apply: None,
    };
    let mut groups = crate::models::Groups::load(&cfg.game_dir);
    let mut pending_group: Option<(usize, String)> = None;
    let mut sel_asset: Option<usize> = None;
    let mut sel_part: Option<usize> = None;
    // Monotonic request id: a worker's result for a superseded click is discarded.
    let mut model_req: u64 = 0;
    let mut pending_model: Option<(String, Assembly)> = None;
    let mut sel_submesh: Option<usize> = None;
    let mut sel_bone: Option<usize> = None;
    let mut sel_tex: Option<usize> = None;
    let mut thumbs = Thumbs::default();
    let mut nav_search = String::new();
    let mut load_status: Option<(String, bool)> = None;
    let mut exporter: Option<Exporter> = None;
    // A texture resolve is in flight for the current model; an export now would be untextured.
    let mut tex_pending = false;
    // A context-menu export waiting for its model and textures to land.
    let mut export_when_ready = false;
    let mut last_frame = Instant::now();

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
                    // --- a newly chosen install opens in place ---
                    if let Some(new_dir) = settings_ctx.apply.take() {
                        let mut s = settings_ctx.s.clone();
                        s.game_dir = new_dir;
                        s.clamp();
                        if let Err(e) = s.save() {
                            load_status = Some((format!("settings: {e}"), true));
                        }
                        cfg.settings = s.clone();
                        cfg.game_dir = s.game_dir.clone();
                        settings_ctx.launched_with = cfg.game_dir.clone();
                        source = None;
                        browse_assets = Vec::new();
                        editor_state = None;
                        view.clear(&mut renderer);
                        current = None;
                        sel_asset = None;
                        sel_part = None;
                        sel_submesh = None;
                        sel_bone = None;
                        sel_tex = None;
                        thumbs.clear();
                        model_req = model_req.wrapping_add(1);
                        groups = crate::models::Groups::load(&cfg.game_dir);
                        opening = true;
                        load_status = Some((format!("opening {}…", cfg.game_dir), false));
                        spawn_open(cfg.clone(), bg_tx.clone());
                    }

                    // Raised by the verb bar, F10, or a deferred context-menu export.
                    let mut export_req = false;
                    while let Ok(msg) = bg_rx.try_recv() {
                        match msg {
                            BgMsg::Opened { source: src, assets, boot } => {
                                opening = false;
                                browse_assets = assets;
                                editor_state = match crate::editor::Editor::new(src.clone(), &cfg.game_dir, cfg.settings.lang, &cfg.settings.dlc_slot, &cfg.settings.mod_name) {
                                    Ok(e) => Some(e),
                                    Err(e) => {
                                        errors.push(format!("editor pages: {e}"));
                                        None
                                    }
                                };
                                settings_ctx.needs_install = false;
                                settings_ctx.s = cfg.settings.clone();
                                settings_ctx.dir_edit = cfg.game_dir.clone();
                                settings_ctx.note = Some((format!("Opened {}.", src.label()), false));
                                println!("[sab_workshop] opened {}", src.label());
                                for m in &src.fingerprint().matched {
                                    println!("[sab_workshop]   {m}");
                                }
                                match boot {
                                    Ok(m) => {
                                        let profile = profile_for(&cfg, src.as_ref(), &m);
                                        view.show(m, profile, &mut renderer, &mut camera, src.clips());
                                        load_status = Some(model_status(&view));
                                        model_req = model_req.wrapping_add(1);
                                        tex_pending = true;
                                        spawn_tex_resolve(src.clone(), &view, model_req, bg_tx.clone());
                                    }
                                    Err(e) => {
                                        errors.push(format!("startup model: {e}"));
                                        load_status = Some((format!("startup model: {e}"), true));
                                    }
                                }
                                source = Some(src);
                                page = Page::Inspect;
                            }
                            BgMsg::OpenFailed(e) => {
                                opening = false;
                                eprintln!("[sab_workshop] open: {e}");
                                settings_ctx.note = Some((format!("Could not open that folder: {e}"), true));
                                load_status = Some((format!("open: {e}"), true));
                                page = Page::Settings;
                            }
                            BgMsg::Model { req, result } => {
                                if req != model_req {
                                    continue;
                                }
                                match (result, &source) {
                                    (Ok(m), Some(src)) => {
                                        let profile = profile_for(&cfg, src.as_ref(), &m);
                                        view.show(m, profile, &mut renderer, &mut camera, src.clips());
                                        if let Some(c) = &mut current {
                                            c.track_to_bone = clip::bind_to_rig(c.clip.as_ref(), &view.rig_hashes);
                                        }
                                        sel_submesh = None;
                                        sel_bone = None;
                                        sel_tex = None;
                                        thumbs.clear();
                                        load_status = Some(model_status(&view));
                                        tex_pending = true;
                                        spawn_tex_resolve(src.clone(), &view, model_req, bg_tx.clone());
                                    }
                                    (Ok(_), None) => {}
                                    (Err(e), _) => {
                                        eprintln!("[sab_workshop] assemble: {e}");
                                        load_status = Some((format!("assemble: {e}"), true));
                                        export_when_ready = false;
                                    }
                                }
                            }
                            BgMsg::Textures { req, result } => {
                                if req != model_req {
                                    continue;
                                }
                                tex_pending = false;
                                match result.and_then(|mt| view.apply_textures(mt, &mut renderer)) {
                                    Ok(summary) => {
                                        println!("[sab_workshop] {summary}");
                                        if export_when_ready {
                                            export_when_ready = false;
                                            export_req = true;
                                        }
                                    }
                                    Err(e) => {
                                        load_status = Some((format!("textures for {}: {e}", view.model_name), true));
                                        export_when_ready = false;
                                    }
                                }
                            }
                        }
                    }

                    // --- advance playback ---
                    let now = Instant::now();
                    let dt = (now - last_frame).as_secs_f32().min(0.1);
                    last_frame = now;
                    if let Some(c) = &current {
                        if playback.playing && c.duration > 0.0 {
                            playback.time += dt * playback.speed;
                            if playback.looping {
                                playback.time = playback.time.rem_euclid(c.duration);
                            } else if playback.time > c.duration {
                                playback.time = c.duration;
                                playback.playing = false;
                            }
                        }
                        let pose = c.clip.sample_all(playback.time);
                        renderer.update_joints(&skinning::posed(&view.skel, &pose, &c.track_to_bone, lock_root));
                    }
                    renderer.update_camera(camera.view_proj(renderer.aspect()));

                    // --- UI ---
                    let mut pick_export_dir = false;
                    if settings_ctx.needs_install {
                        page = Page::Settings;
                    }
                    let exportable = view.loaded && !view.picks.is_empty();
                    gui.run(|ctx| {
                        build_ui(
                            ctx,
                            &mut UiCtx {
                                source: source.as_ref(),
                                opening,
                                errors: &errors,
                                load_status: &load_status,
                                page: &mut page,
                                view: &view,
                                current: &current,
                                playback: &mut playback,
                                show_grid: &mut renderer.show_grid,
                                show_textures: &mut renderer.show_textures,
                                damage_state: &mut renderer.damage_state,
                                lock_root: &mut lock_root,
                                show_all: &mut show_all,
                                pending_clip: &mut pending_clip,
                                exp: ExportCtx {
                                    request: &mut export_req,
                                    pick_dir: &mut pick_export_dir,
                                    busy: exporter.as_ref().map(|e| e.label.clone()),
                                    ready: exportable,
                                    from_files: view.loaded && view.picks.is_empty(),
                                    tex_pending,
                                    dest: bundle_root(&settings_ctx.s).join(&view.model_name).display().to_string(),
                                },
                                mats: MatsCtx {
                                    selected: &mut sel_submesh,
                                    pending_tex: &mut pending_tex,
                                    pending_save: &mut pending_save,
                                    thumbs: &mut thumbs,
                                },
                                nav: NavCtx {
                                    tab: &mut nav_tab,
                                    search: &mut nav_search,
                                    browse_assets: &browse_assets,
                                    sel_asset: &mut sel_asset,
                                    sel_part: &mut sel_part,
                                    pending_model: &mut pending_model,
                                    selected_bone: &mut sel_bone,
                                    selected_tex: &mut sel_tex,
                                    groups: &groups,
                                    pending_group: &mut pending_group,
                                    export_after_load: &mut export_when_ready,
                                },
                                ed: editor_state.as_mut(),
                                settings: &mut settings_ctx,
                            },
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

                    // --- start a bundle export on a worker ---
                    if export_req && exporter.is_none() {
                        match (&source, exportable) {
                            (Some(src), true) => {
                                exporter = Some(spawn_bundle_export(bundle::Job {
                                    source: src.clone(),
                                    label: view.model_name.clone(),
                                    picks: view.picks.clone(),
                                    lod_notes: view.notes.clone(),
                                    damage: view.damage.clone(),
                                    submeshes: view.submeshes.clone(),
                                    assets: view.assets.clone(),
                                    submesh_tex: view.submesh_tex.clone(),
                                    submesh_prov: view.submesh_prov.clone(),
                                    outroot: bundle_root(&settings_ctx.s),
                                }));
                            }
                            (_, false) if view.loaded => {
                                load_status = Some(("export: a model from loose files has no build parts to re-assemble".into(), true));
                            }
                            _ => load_status = Some(("export: open a model from the browser first".into(), true)),
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

                    // --- a right-click "move to group" (persists the override) ---
                    if let Some((ai, cat)) = pending_group.take() {
                        if let Some(a) = browse_assets.get(ai) {
                            groups.set(&a.name, &cat, a.category);
                        }
                    }

                    // --- a navigator model click: assemble on a worker ---
                    if let (Some((name, asm)), Some(src)) = (pending_model.take(), &source) {
                        model_req = model_req.wrapping_add(1);
                        tex_pending = false;
                        load_status = Some((format!("assembling {name}…"), false));
                        spawn_assemble(src.clone(), name, asm, model_req, bg_tx.clone());
                    }

                    // --- a Materials picker change: rebind + persist the sidecar ---
                    if let Some((i, slot, ai)) = pending_tex.take() {
                        let s = slot as usize;
                        let tex = match ai.map(|a| view.assets[a].decode()) {
                            Some(Ok(t)) => Some(t),
                            Some(Err(e)) => {
                                load_status = Some((format!("decode {}: {e}", view.assets[ai.unwrap_or(0)].name), true));
                                None
                            }
                            None => None,
                        };
                        if ai.is_none() || tex.is_some() {
                            upload(&mut renderer, i, s, tex.as_ref());
                            view.submesh_tex[i].set(s, ai);
                            view.submesh_prov[i] = if view.submesh_tex[i].is_bound() { Prov::Bound } else { Prov::Unresolved };
                            match resolve::save_sidecar(&view.tex_profile, &view.submesh_tex, &view.submesh_prov, &view.assets) {
                                Ok(()) => println!(
                                    "[sab_workshop] submesh {i} {} -> {} (saved {})",
                                    SubmeshBinding::SLOT_NAMES[s],
                                    ai.map(|x| view.assets[x].name.as_str()).unwrap_or("(none)"),
                                    resolve::sidecar_path(&view.tex_profile)
                                ),
                                Err(e) => load_status = Some((format!("sidecar: {e}"), true)),
                            }
                        }
                    }

                    // --- an explicit sidecar write (the stamp) ---
                    if std::mem::take(&mut pending_save) {
                        match resolve::save_sidecar(&view.tex_profile, &view.submesh_tex, &view.submesh_prov, &view.assets) {
                            Ok(()) => {
                                let n = view.submesh_prov.iter().filter(|p| **p == Prov::Bound).count();
                                load_status = Some((format!("wrote {} ({n} bound)", resolve::sidecar_path(&view.tex_profile)), false));
                            }
                            Err(e) => load_status = Some((format!("sidecar: {e}"), true)),
                        }
                    }

                    // --- a clip request ---
                    if let (Some(ci), Some(src)) = (pending_clip.take(), &source) {
                        match LoadedClip::load(src.as_ref(), ci, &view.rig_hashes) {
                            Ok(c) => {
                                let (bound, wanted) = c.bound();
                                println!(
                                    "[sab_workshop] clip '{}': {} tracks ({bound} of {wanted} bound on this rig), {} frames, {:.3}s",
                                    c.name,
                                    c.clip.num_transform_tracks(),
                                    c.frame_count,
                                    c.duration
                                );
                                load_status = Some((format!("{}: {bound} of {wanted} tracks on this rig", c.name), bound < wanted));
                                current = Some(c);
                                playback.time = 0.0;
                                playback.playing = true;
                            }
                            Err(e) => load_status = Some((format!("clip: {e}"), true)),
                        }
                    }

                    renderer.draw_scene = page.editor().is_none() && page != Page::Settings && view.loaded;
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
    rx: Receiver<Result<String, String>>,
    /// The asset being exported — for the progress card and the completion message.
    label: String,
}

/// Where export bundles land: the configured directory, else `workshop_export/` beside wherever the
/// app was launched.
fn bundle_root(s: &crate::settings::Settings) -> std::path::PathBuf {
    if s.export_dir.is_empty() {
        std::env::current_dir().unwrap_or_default().join("workshop_export")
    } else {
        std::path::PathBuf::from(&s.export_dir)
    }
}

/// Run a bundle export off the UI thread.
fn spawn_bundle_export(job: bundle::Job) -> Exporter {
    let (tx, rx) = std::sync::mpsc::channel();
    let label = job.label.clone();
    std::thread::spawn(move || {
        let _ = tx.send(bundle::run(&job));
    });
    Exporter { rx, label }
}

/// Open the build in `cfg`, for the headless commands.
fn open(cfg: &Config) -> Result<(Arc<dyn GameSource>, Vec<Asset>), String> {
    let src: Arc<dyn GameSource> = Arc::from(crate::source::open_source(std::path::Path::new(&cfg.game_dir), &cfg.overrides)?);
    let assets = crate::assets::build(src.as_ref());
    Ok((src, assets))
}

/// Headless bundle export of the asset named `name`, textured from the build's material data —
/// exactly what the app writes for that asset with no hand-made bindings.
pub fn export(cfg: Config, name: &str, out: &str) -> i32 {
    let run = || -> Result<String, String> {
        let (src, assets) = open(&cfg)?;
        let a = assets.iter().find(|a| a.name == name).ok_or_else(|| format!("no asset named {name} in {}", src.label()))?;
        let asm = a.assembly();
        let model = crate::boot::assemble(src.as_ref(), &a.name, &asm)?;
        let submeshes = formats::submesh_cover(&model.lm.mesh.prims, model.lm.mesh.indices.len() as u32);
        let mt = src.resolve_textures(&model.lm.part_ranges, &submeshes, &model.lm.uvless)?;
        bundle::run(&bundle::Job {
            source: src.clone(),
            label: a.name.clone(),
            picks: asm.picks.clone(),
            lod_notes: asm.notes.clone(),
            damage: asm.damage.clone(),
            submeshes,
            assets: mt.assets,
            submesh_tex: mt.submesh_tex,
            submesh_prov: mt.submesh_prov,
            outroot: std::path::PathBuf::from(out),
        })
    };
    match run() {
        Ok(dir) => {
            println!("{dir}");
            0
        }
        Err(e) => {
            eprintln!("export {name}: {e}");
            1
        }
    }
}

/// Headless verification of texture binding: for every asset whose name contains `filter`
/// (case-insensitive), assemble it and run the click path's resolve, then print each submesh's
/// material → bound texture with its provenance, and a coverage tally.
pub fn texcheck(cfg: Config, filter: &str) -> i32 {
    let (src, assets) = match open(&cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("texcheck: {e}");
            return 1;
        }
    };
    let filt = filter.to_ascii_lowercase();
    let (mut n_assets, mut tot_sub, mut tot_bound) = (0usize, 0usize, 0usize);
    let mut failed: Vec<String> = Vec::new();
    for a in assets.iter().filter(|a| filt.is_empty() || a.name.to_ascii_lowercase().contains(&filt)) {
        let model = match crate::boot::assemble(src.as_ref(), &a.name, &a.assembly()) {
            Ok(m) => m,
            Err(e) => {
                failed.push(format!("{}: assemble: {e}", a.name));
                continue;
            }
        };
        let submeshes = formats::submesh_cover(&model.lm.mesh.prims, model.lm.mesh.indices.len() as u32);
        let mt = match src.resolve_textures(&model.lm.part_ranges, &submeshes, &model.lm.uvless) {
            Ok(mt) => mt,
            Err(e) => {
                failed.push(format!("{}: textures: {e}", a.name));
                continue;
            }
        };
        n_assets += 1;
        let bound = mt.submesh_tex.iter().filter(|b| b.is_bound()).count();
        tot_sub += submeshes.len();
        tot_bound += bound;
        println!("\n{}  ({} parts, {} submeshes)  bound {bound}/{}", a.name, model.picks.len(), submeshes.len(), submeshes.len());
        for n in &model.notes {
            println!("  note: {n}");
        }
        for (i, sm) in submeshes.iter().enumerate() {
            let mat = sm.materials.first().map(|m| format!("{m:08X}")).unwrap_or_else(|| "--------".into());
            let slots: Vec<String> = mt.submesh_tex[i]
                .iter_bound()
                .map(|(s, ai)| format!("{}={}", SubmeshBinding::SLOT_NAMES[s], mt.assets[ai].name))
                .collect();
            let slots = if slots.is_empty() { "(unresolved)".into() } else { slots.join(", ") };
            println!("  [{i:2}] LOD{} mat {mat}  {slots}", sm.lod);
        }
    }
    println!("\ntexcheck '{filter}' on {}: {n_assets} assets, {tot_bound}/{tot_sub} submeshes bound; {} FAILED", src.label(), failed.len());
    for f in &failed {
        println!("  FAILED {f}");
    }
    if failed.is_empty() { 0 } else { 1 }
}

/// Headless self-test: open the build, boot its model, decode the N-th clip playable on that rig,
/// run skinning over a few frames, and print sanity stats. Returns a process exit code.
pub fn selftest(cfg: Config, n: usize) -> i32 {
    let (src, assets) = match open(&cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("selftest: {e}");
            return 1;
        }
    };
    let boot = match crate::boot::load(&cfg, src.as_ref(), &assets) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("selftest: boot model: {e}");
            return 1;
        }
    };
    let (mesh, skel) = (&boot.lm.mesh, &boot.lm.bones);
    println!("selftest: {} — boot model {}: {} verts / {} tris, {} bones, {} clips", src.label(), boot.name, mesh.positions.len(), mesh.indices.len() / 3, skel.len(), src.clips().len());
    for n in &boot.notes {
        println!("selftest:   note: {n}");
    }
    let max_joint = mesh.joints.iter().flat_map(|j| j.iter()).copied().max().unwrap_or(0);
    println!("selftest: max mesh joint index = {max_joint} (skeleton has {} bones)", skel.len());
    let bind = skinning::bind_pose(skel);
    let worst = bind.iter().flat_map(|m| (*m - glam::Mat4::IDENTITY).to_cols_array()).fold(0f32, |a, c| a.max(c.abs()));
    println!("selftest: bind-pose max deviation of jointMatrix from identity = {worst:.5} (expect ~0)");

    let rig: HashSet<u32> = boot.lm.bone_hashes.iter().copied().collect();
    let playable: Vec<usize> = src.clips().iter().enumerate().filter(|(_, c)| c.bones.iter().all(|h| *h == 0 || rig.contains(h))).map(|(i, _)| i).collect();
    println!("selftest: {} of {} clips are on this rig", playable.len(), src.clips().len());
    let Some(&ci) = playable.get(n) else {
        eprintln!("selftest: no playable clip {n}");
        return 1;
    };
    let clip = match LoadedClip::load(src.as_ref(), ci, &boot.lm.bone_hashes) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("selftest: load clip: {e}");
            return 1;
        }
    };
    let (bound, wanted) = clip.bound();
    println!(
        "selftest: clip '{}' — {} tracks ({bound} of {wanted} bound), {} frames, {:.3}s",
        clip.name,
        clip.clip.num_transform_tracks(),
        clip.frame_count,
        clip.duration
    );
    let steps = clip.frame_count.clamp(1, 8);
    let mut max_disp = 0f32;
    for f in 0..steps {
        let t = f as f32 * clip.duration / steps as f32;
        let mats = skinning::posed(skel, &clip.clip.sample_all(t), &clip.track_to_bone, true);
        if mats.len() != skel.len() {
            eprintln!("selftest: joint count mismatch");
            return 1;
        }
        for (m, b) in mats.iter().zip(&bind) {
            max_disp = max_disp.max((m.w_axis - b.w_axis).truncate().length());
        }
    }
    println!("selftest: sampled {steps} frames, max joint-origin displacement vs bind = {max_disp:.4} m");
    let pose0 = clip.clip.sample_all(0.0);
    let nonunit = pose0.iter().filter(|q| ((q.r[0] * q.r[0] + q.r[1] * q.r[1] + q.r[2] * q.r[2] + q.r[3] * q.r[3]).sqrt() - 1.0).abs() > 1e-3).count();
    println!("selftest: t=0: {} tracks, {nonunit} non-unit rotations", pose0.len());
    if nonunit > 0 || bound < wanted {
        eprintln!("selftest: the clip does not bind or sample cleanly on the boot model's rig");
        return 1;
    }
    let cover = formats::submesh_cover(&mesh.prims, mesh.indices.len() as u32);
    let mt = match src.resolve_textures(&boot.lm.part_ranges, &cover, &boot.lm.uvless) {
        Ok(mt) => mt,
        Err(e) => {
            eprintln!("selftest: textures: {e}");
            return 1;
        }
    };
    let textured = mt.submesh_tex.iter().filter(|b| b.diffuse().is_some()).count();
    println!("selftest: {textured}/{} submeshes bind a diffuse ({} textures in the pool)", cover.len(), mt.assets.len());
    if textured == 0 {
        eprintln!("selftest: the boot model is untextured");
        return 1;
    }
    let mut by_cat: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for a in &assets {
        *by_cat.entry(a.category).or_default() += 1;
    }
    println!("selftest: {} meshes -> {} assets; categories:", src.meshes().len(), assets.len());
    for (c, n) in &by_cat {
        println!("  {c:12} {n}");
    }
    println!("selftest: OK");
    0
}

/// Sweep clips playable on the build's model and report which decode to an insane pose. A track's
/// translation should stay near its bone's BIND translation; only the root chain legitimately
/// travels.
pub fn anim_sweep(cfg: Config, limit: usize) -> i32 {
    let (src, assets) = match open(&cfg) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sweep: {e}");
            return 1;
        }
    };
    let boot = match crate::boot::load(&cfg, src.as_ref(), &assets) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("sweep: rig: {e}");
            return 1;
        }
    };
    let skel = &boot.lm.bones;
    let rig: HashSet<u32> = boot.lm.bone_hashes.iter().copied().collect();
    let playable: Vec<usize> = src.clips().iter().enumerate().filter(|(_, c)| c.bones.iter().all(|h| *h == 0 || rig.contains(h))).map(|(i, _)| i).collect();
    let n = limit.min(playable.len());
    println!("sweep: {} — checking {n} clips (bad = a NON-root-chain bone >0.75 m from its bind local translation)", src.label());
    let (mut bad, mut failed) = (0usize, 0usize);
    let mut by_blocks: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
    let mut ctrl_tally: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut max_scale_dev = (String::new(), 0f32);
    let (mut tot_nonfinite, mut tot_nonunit) = (0usize, 0usize);
    for &ci in playable.iter().take(n) {
        let clip = match LoadedClip::load(src.as_ref(), ci, &boot.lm.bone_hashes) {
            Ok(c) => c,
            Err(e) => {
                println!("  FAILED {}: {e}", src.clips()[ci].name);
                failed += 1;
                continue;
            }
        };
        // The spline block count and mask bytes exist only for retail's codec.
        let spline = clip.clip.as_any().downcast_ref::<RetailClip>().map(|r| r.spline());
        let nb = spline.map(|(a, _)| a.num_blocks).unwrap_or(1);
        let (mut worst, mut worst_bone, mut worst_scale) = (0f32, (-1i32, String::new()), 0f32);
        let (mut nonfinite, mut nonunit) = (0usize, 0usize);
        for f in 0..4 {
            let pose = clip.clip.sample_all(f as f32 * clip.duration / 4.0);
            for (k, q) in pose.iter().enumerate() {
                let bone = clip.track_to_bone.get(k).copied().unwrap_or(-1);
                if bone < 0 {
                    continue;
                }
                let nm = skel[bone as usize].name.to_ascii_lowercase();
                if nm.contains("globalsrt") || nm.contains("bone_root") || nm.contains("hips") {
                    continue;
                }
                let bt = skel[bone as usize].t;
                let d = ((q.t[0] - bt[0]).powi(2) + (q.t[1] - bt[1]).powi(2) + (q.t[2] - bt[2]).powi(2)).sqrt();
                if d > worst {
                    worst = d;
                    worst_bone = (bone, skel[bone as usize].name.clone());
                }
            }
            for q in &pose {
                for c in 0..3 {
                    worst_scale = worst_scale.max((q.s[c] - 1.0).abs());
                }
                if !q.s.iter().chain(&q.t).all(|v| v.is_finite()) {
                    nonfinite += 1;
                }
                let qn = (q.r[0] * q.r[0] + q.r[1] * q.r[1] + q.r[2] * q.r[2] + q.r[3] * q.r[3]).sqrt();
                if (qn - 1.0).abs() > 1e-3 {
                    nonunit += 1;
                }
            }
        }
        let e = by_blocks.entry(nb).or_insert((0, 0));
        e.0 += 1;
        let mut ctrls: Vec<u8> = spline.map(|(a, blob)| a.track_masks(blob, 0)).unwrap_or_default().iter().map(|m| m[0]).collect();
        ctrls.sort_unstable();
        ctrls.dedup();
        let ctrl_str = ctrls.iter().map(|c| format!("{c:#04x}")).collect::<Vec<_>>().join(",");
        let te = ctrl_tally.entry(ctrl_str).or_insert((0, 0));
        te.0 += 1;
        if worst_scale > max_scale_dev.1 {
            max_scale_dev = (clip.name.clone(), worst_scale);
        }
        tot_nonfinite += nonfinite;
        tot_nonunit += nonunit;
        if worst > 0.75 {
            bad += 1;
            e.1 += 1;
            te.1 += 1;
            if bad <= 8 {
                println!("  BAD {:<38} frames={} worst={:.2} m on bone {} '{}'", clip.name, clip.frame_count, worst, worst_bone.0, worst_bone.1);
            }
        }
    }
    println!("sweep: {bad}/{n} bad, {failed} failed to load");
    println!("sweep: worst |scale-1| = {:.4} in '{}'", max_scale_dev.1, max_scale_dev.0);
    println!("sweep: non-finite components: {tot_nonfinite}   non-unit quaternions: {tot_nonunit}");
    println!("sweep: by ctrl-byte set -> (checked, bad):");
    for (c, (tot, b)) in &ctrl_tally {
        println!("   ctrl={{{c}}}  checked={tot:<5} bad={b}");
    }
    println!("sweep: by num_blocks -> (checked, bad):");
    for (nb, (tot, b)) in by_blocks {
        println!("   blocks={nb:<3} checked={tot:<5} bad={b}");
    }
    if failed == 0 && bad == 0 { 0 } else { 1 }
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
/// Deliberately lazy in two ways: a thumbnail is only decoded when its tile is actually drawn
/// (egui's ScrollArea culls the rest), and it decodes a small mip rather than the finest one.
///
/// A record that fails to decode is remembered as a failure, with its reason, or we would retry the
/// same broken record every frame for as long as the page is open.
#[derive(Default)]
struct Thumbs {
    cache: std::collections::HashMap<usize, Result<egui::TextureHandle, String>>,
}

impl Thumbs {
    fn get(&mut self, i: usize, ctx: &egui::Context, assets: &[TexAsset]) -> Result<egui::TextureHandle, String> {
        if let Some(hit) = self.cache.get(&i) {
            return hit.clone();
        }
        let made = match assets.get(i) {
            None => Err(format!("texture {i} is not in the pool")),
            Some(a) => a.decode_preview(96).map_err(|e| format!("{}: {e}", a.name)).map(|t| {
                let img = egui::ColorImage::from_rgba_unmultiplied([t.width as usize, t.height as usize], &t.rgba);
                ctx.load_texture(format!("thumb{i}"), img, egui::TextureOptions::LINEAR)
            }),
        };
        self.cache.insert(i, made.clone());
        made
    }

    /// Drop everything — the asset pool changed, so every index is stale.
    fn clear(&mut self) {
        self.cache.clear();
    }
}

/// Depth of each bone in the hierarchy, so the rig tree can indent by it.
///
/// A bone whose parent is not an earlier bone is treated as a root rather than trusted — a
/// malformed rig should indent oddly, not hang.
fn bone_depths(skel: &[formats::Bone]) -> Vec<usize> {
    let mut d = vec![0usize; skel.len()];
    for i in 0..skel.len() {
        let p = skel[i].parent;
        d[i] = if p >= 0 && (p as usize) < i { d[p as usize] + 1 } else { 0 };
    }
    d
}

/// A rail page. Each one exists because its LAYOUT differs, not merely its subject.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    /// The model/animation viewer — pick a model or clip and look at it (wgpu scene + rig).
    Inspect,
    /// GameText.dlg — everything the player reads, per language.
    Strings,
    /// The template table — what a thing IS: its property pairs.
    Objects,
    /// The texture pool as pictures, and wiring one into an object property.
    Icons,
    /// Where the game is, and the defaults every other page starts from.
    Settings,
}

impl Page {
    const ALL: [Page; 5] = [Page::Inspect, Page::Strings, Page::Objects, Page::Icons, Page::Settings];
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
    /// empty box with no error.
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
    /// The editor page this maps to, or `None` for the pages that are not editor views.
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

/// What the Inspect navigator lists: models, clips, the loaded model's textures, its rig.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NavTab {
    Models,
    Animations,
    Textures,
    Rig,
}

impl NavTab {
    const ALL: [NavTab; 4] = [NavTab::Models, NavTab::Animations, NavTab::Textures, NavTab::Rig];
    fn label(self) -> &'static str {
        match self {
            NavTab::Models => "models",
            NavTab::Animations => "anims",
            NavTab::Textures => "textures",
            NavTab::Rig => "rig",
        }
    }
}

/// The export verb's state for one frame — what the verb bar reads, and what it raises.
struct ExportCtx<'a> {
    /// Raised by the verb-bar button or F10: start an export of what is loaded.
    request: &'a mut bool,
    /// Raised by clicking the destination path: pick a new bundle root.
    pick_dir: &'a mut bool,
    /// The asset an export is already running for, if any — the verb is disabled while set.
    busy: Option<String>,
    /// A model assembled from the build is on screen.
    ready: bool,
    /// The model on screen came from loose files, which a bundle cannot re-assemble.
    from_files: bool,
    /// Textures are still resolving; a bundle written now would be untextured.
    tex_pending: bool,
    /// Where this export would land, shown so it is never a mystery.
    dest: String,
}

/// The Materials panel's out-params and caches.
struct MatsCtx<'a> {
    /// Which submesh the panel is working on.
    selected: &'a mut Option<usize>,
    /// A per-slot picker click: `(submesh, slot 0..=3, asset)`.
    pending_tex: &'a mut Option<(usize, u8, Option<usize>)>,
    /// Set by the stamp: re-write the sidecar as it currently stands.
    pending_save: &'a mut bool,
    thumbs: &'a mut Thumbs,
}

/// The navigator's state and click out-params.
struct NavCtx<'a> {
    tab: &'a mut NavTab,
    search: &'a mut String,
    /// The assembled assets the browser lists.
    browse_assets: &'a [Asset],
    /// The selected asset (whose parts the inspector shows).
    sel_asset: &'a mut Option<usize>,
    /// The part (mesh item) whose record the inspector describes.
    sel_part: &'a mut Option<usize>,
    pending_model: &'a mut Option<(String, Assembly)>,
    selected_bone: &'a mut Option<usize>,
    /// Which pool texture the Textures tab is showing.
    selected_tex: &'a mut Option<usize>,
    /// Model-browser groupings (rule-based + user overrides), for the grouped model tree.
    groups: &'a crate::models::Groups,
    /// A right-click "move to group" request: (asset index, new category), applied after the UI.
    pending_group: &'a mut Option<(usize, String)>,
    /// A right-click "Export bundle" on a row that is not the loaded model: export once the model
    /// and its texture resolve have landed.
    export_after_load: &'a mut bool,
}

/// Everything one frame of UI reads and writes.
struct UiCtx<'a> {
    source: Option<&'a Arc<dyn GameSource>>,
    opening: bool,
    errors: &'a [String],
    load_status: &'a Option<(String, bool)>,
    page: &'a mut Page,
    view: &'a View,
    current: &'a Option<LoadedClip>,
    playback: &'a mut Playback,
    show_grid: &'a mut bool,
    show_textures: &'a mut bool,
    /// The damage state drawn (`Renderer::damage_state`).
    damage_state: &'a mut i32,
    lock_root: &'a mut bool,
    /// List clips that are not on the loaded rig too.
    show_all: &'a mut bool,
    pending_clip: &'a mut Option<usize>,
    exp: ExportCtx<'a>,
    mats: MatsCtx<'a>,
    nav: NavCtx<'a>,
    ed: Option<&'a mut crate::editor::Editor>,
    settings: &'a mut SettingsCtx,
}

/// Live state for the Settings page: the values being edited, plus what changing them implies.
pub struct SettingsCtx {
    pub s: crate::settings::Settings,
    /// The install path as typed. Kept separate from `s.game_dir` so a half-typed path does not
    /// re-run the existence checks against nonsense on every keystroke.
    pub dir_edit: String,
    /// The folder last opened (or tried), so the auto-apply fires once per change.
    pub launched_with: String,
    pub note: Option<(String, bool)>, // (message, is_error)
    /// The app came up with no valid install: the Settings page is the forced first-run chooser and
    /// shows a prominent red error until a valid folder is picked.
    pub needs_install: bool,
    /// A validated folder the user just chose, for `run()` to open in place. `None` the rest of the
    /// time.
    pub apply: Option<String>,
}

/// The Settings page. One column of labelled groups; nothing here is a mod edit, so nothing here
/// touches the changelist or the DLC overlay.
fn settings_page(ctx: &egui::Context, sx: &mut SettingsCtx) {
    egui::CentralPanel::default().show(ctx, |ui| {
        // `auto_shrink([false, false])` makes the scroll region fill the whole panel, so the wheel
        // works anywhere over the page and the scrollbar sits at the far right edge.
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(720.0);
            ui.add_space(8.0);
            ui.label(theme::poster_text("SETTINGS", 22.0, theme::TX));
            ui.label(theme::data_text(&format!("stored at {}", crate::settings::settings_path().display()), 10.0, theme::FAINT));
            ui.add_space(14.0);

            if sx.needs_install {
                egui::Frame::none()
                    .fill(theme::RED_SOFT)
                    .stroke(egui::Stroke::new(1.0, theme::RED))
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0))
                    .outer_margin(egui::Margin { bottom: 14.0, ..Default::default() })
                    .show(ui, |ui| {
                        ui.label(theme::poster_text("NO GAME FOLDER — NOTHING CAN LOAD", 15.0, theme::RED));
                        ui.add_space(4.0);
                        ui.label(theme::data_text(
                            "The workshop has no Saboteur folder to read, so every page is empty. Choose the \
                             retail install (it holds Global/, France.materials and Saboteur.exe) or the 2008 \
                             prototype (it holds Bin360/) below — it opens the instant the checklist turns \
                             green. No restart, no Save button.",
                            11.0,
                            theme::TX,
                        ));
                    });
            }

            theme::eyebrow(ui, "Game folder");
            ui.label(theme::data_text(
                "Everything the workshop reads comes from this folder. It is opened READ-ONLY; retail mods publish to a DLC overlay inside it.",
                11.0,
                theme::DIM,
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut sx.dir_edit)
                        .desired_width(430.0)
                        // Describe the folder, do not name one: it lives somewhere different on every
                        // machine, so we never guess — the user points us at it.
                        .hint_text("the retail install, or the 2008 prototype's folder"),
                );
                if ui.button("Browse…").clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title("Select a Saboteur build folder").pick_folder() {
                        sx.dir_edit = d.to_string_lossy().replace('\\', "/");
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
                        ui.label(theme::data_text("(optional — one page has nothing to show)", 10.0, theme::FAINT));
                    }
                });
            }

            ui.add_space(12.0);
            theme::eyebrow(ui, "Defaults");
            egui::Grid::new("settings_defaults").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                ui.label(theme::data_text("Language", 11.0, theme::DIM));
                egui::ComboBox::from_id_source("set_lang").selected_text(crate::settings::LANG_NAMES[sx.s.lang].1).show_ui(ui, |ui| {
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
                    ui.label(theme::data_text("02–04 only: the engine probes four slots and 01 is the game's own", 10.0, theme::FAINT));
                });
                ui.end_row();
            });

            ui.add_space(12.0);
            theme::eyebrow(ui, "Appearance");
            ui.horizontal(|ui| {
                ui.label(theme::data_text("Type scale", 11.0, theme::DIM));
                if ui.add(egui::Slider::new(&mut sx.s.type_scale, 0.8..=2.0).step_by(0.05).show_value(true)).changed() {
                    // Applies live: set the scale, then re-install so egui's cached TextStyle table
                    // picks up the new sizes too.
                    theme::set_type_scale(sx.s.type_scale);
                    theme::install(ctx);
                }
            });
            ui.label(theme::data_text("Scales every voice together, so the hierarchy between them is preserved.", 10.0, theme::FAINT));

            ui.add_space(12.0);
            theme::eyebrow(ui, "Voice audio");
            ui.horizontal(|ui| {
                ui.label(theme::data_text("vgmstream-cli", 11.0, theme::DIM));
                ui.add(egui::TextEdit::singleline(&mut sx.s.vgmstream_path).desired_width(320.0).hint_text("path to vgmstream-cli"));
                if ui.button("Browse…").clicked() {
                    let mut dlg = rfd::FileDialog::new().set_title("Select vgmstream-cli");
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

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);
            let typed = sx.dir_edit.replace('\\', "/");
            let typed = typed.trim_end_matches('/').to_string();
            let typed_valid = crate::settings::Settings { game_dir: typed.clone(), ..sx.s.clone() }.is_valid();

            // AUTO-APPLY the folder: the moment a valid, different folder is in the box, hand it to
            // `run()` to open in place and persist it. Only the folder auto-applies; the other
            // defaults below still commit on Save.
            if typed_valid && typed != sx.launched_with && sx.apply.is_none() {
                sx.apply = Some(typed.clone());
            }

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
            if !typed.is_empty() && typed != sx.launched_with && !typed_valid {
                ui.add_space(8.0);
                ui.label(theme::data_text(
                    "That folder is not a Saboteur build the workshop recognizes (see the checklist).",
                    11.0,
                    theme::EMBER,
                ));
            }
            ui.add_space(20.0);
        });
    });
}

fn build_ui(ctx: &egui::Context, u: &mut UiCtx) {
    let view = u.view;
    let textured = view.submesh_tex.iter().filter(|b| b.is_bound()).count();
    let bound = view.submesh_prov.iter().filter(|p| **p == Prov::Bound).count();

    // 1-5 select a page — but not while a text field has focus.
    if !ctx.wants_keyboard_input() {
        for p in Page::ALL {
            if ctx.input(|i| i.key_pressed(p.key())) {
                *u.page = p;
            }
        }
    }
    if let Some(ed) = u.ed.as_deref_mut() {
        // Land any editor assets that finished loading since the last frame, before anything renders.
        ed.pump(ctx);
    }

    // ── COMMAND BAR ──
    egui::TopBottomPanel::top("cmdbar").exact_height(theme::bar_h()).frame(theme::bar_frame()).show(ctx, |ui| {
        ui.horizontal_centered(|ui| {
            theme::brand_mark(ui);
            ui.add_space(2.0);
            ui.label(theme::poster_text("SAB WORKSHOP", 18.0, theme::TX));
            ui.label(theme::data_text("ASSET WORKBENCH", 9.0, theme::FAINT));
            if let Some(src) = u.source {
                theme::chip(ui, src.version().label(), false, None);
            }
            ui.separator();
            match (u.page.editor(), u.ed.as_deref()) {
                (Some(_), Some(ed)) => {
                    let (name, out) = ed.mod_chip();
                    ui.label(theme::disp_text("MOD", 10.0, theme::FAINT));
                    ui.label(theme::data_text(name, 11.5, theme::RED));
                    ui.label(theme::data_text(out, 10.0, theme::COLD));
                }
                _ => {
                    ui.label(theme::data_text(&view.model_name, 11.5, theme::RED));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match (u.page.editor(), u.ed.as_deref()) {
                (Some(_), Some(ed)) => {
                    let n = ed.total_pending();
                    theme::chip(ui, &format!("{n} staged"), n > 0, None);
                }
                _ => {
                    theme::chip(ui, "orbit", true, None);
                    theme::chip(ui, if *u.show_textures { "textured" } else { "untextured" }, *u.show_textures, None);
                }
            });
        });
    });

    if !u.errors.is_empty() {
        egui::TopBottomPanel::top("errors").show(ctx, |ui| {
            for e in u.errors {
                ui.colored_label(theme::RED, format!("load error: {e}"));
            }
        });
    }

    // ── STATUS BAR ──
    egui::TopBottomPanel::bottom("status").exact_height(theme::status_h()).frame(theme::bar_frame()).show(ctx, |ui| {
        ui.horizontal_centered(|ui| {
            let (col, msg) = match (u.source, u.opening) {
                (_, true) => (theme::EMBER, "opening…"),
                (Some(_), false) => (theme::EMBER, "ready"),
                (None, false) => (theme::RED, "no build"),
            };
            theme::status_dot(ui, msg, col);
            ui.separator();
            ui.label(theme::data_text(format!("{} verts · {} tris · {} bones", view.mesh_stats.0, view.mesh_stats.1, view.mesh_stats.2), 10.0, theme::DIM));
            ui.separator();
            let fact = match (u.page.editor(), u.ed.as_deref()) {
                (Some(ep), Some(ed)) => ed.status(ep),
                _ => format!("{textured}/{} submeshes textured", view.submeshes.len()),
            };
            ui.label(theme::data_text(fact, 10.0, theme::DIM));
            if let Some((msg, is_err)) = u.load_status {
                ui.separator();
                ui.label(theme::data_text(msg, 10.0, if *is_err { theme::RED } else { theme::EMBER }));
            }
        });
    });

    // ── VERB BAR ──
    if *u.page == Page::Inspect {
        let exp = &mut u.exp;
        egui::TopBottomPanel::bottom("verbbar").exact_height(theme::verb_h()).frame(theme::bar_frame()).show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                let busy = exp.busy.is_some();
                let label = match &exp.busy {
                    Some(l) => format!("exporting {l}…"),
                    None => "Export  F10".into(),
                };
                let r = theme::action_button(ui, &label, exp.ready && !busy);
                if r.clicked() {
                    *exp.request = true;
                }
                r.on_hover_text(
                    "Export bundle — writes model.glb (skinned, one object per level of detail, one primitive per \
                     submesh), textures/*.png, the verbatim raw/ source records, and manifest.json (the reassembly map).",
                );
                ui.separator();
                ui.label(theme::data_text("→", 11.0, theme::FAINT));
                if ui
                    .add(egui::Label::new(theme::data_text(short_path(&exp.dest), 10.0, theme::COLD)).sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(format!("{}\n\nClick to choose where bundles are written", exp.dest))
                    .clicked()
                {
                    *exp.pick_dir = true;
                }
                if exp.ready && exp.tex_pending {
                    ui.separator();
                    ui.label(theme::data_text("textures still resolving — exporting now writes an untextured bundle", 10.0, theme::EMBER));
                } else if exp.from_files {
                    ui.separator();
                    ui.label(theme::data_text("a model from loose files has no build parts to export", 10.0, theme::FAINT));
                } else if !exp.ready {
                    ui.separator();
                    ui.label(theme::data_text("load a model to export", 10.0, theme::FAINT));
                }
            });
        });
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::F10)) && exp.ready && exp.busy.is_none() {
            *exp.request = true;
        }
    }

    // ── EXPORT PROGRESS ── (not modal: the export works on its own copy of the data)
    if let Some(l) = &u.exp.busy {
        ctx.request_repaint();
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
                ui.label(theme::data_text("Decoding a model's skins takes a few seconds. The viewport stays live.", 10.0, theme::FAINT));
            });
    }

    // ── RAIL ──
    egui::SidePanel::left("rail").exact_width(theme::RAIL_W).resizable(false).frame(egui::Frame::none().fill(theme::G0)).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        ui.add_space(4.0);
        for (i, p) in Page::ALL.iter().enumerate() {
            let (done, total) = match p {
                Page::Inspect => (textured, view.submeshes.len()),
                _ => (0, 0),
            };
            let r = theme::rail_button(ui, &format!("{}", i + 1), p.glyph(), p.label(), *u.page == *p, done, total);
            if let (Some(ep), Some(ed)) = (p.editor(), u.ed.as_deref()) {
                let n = ed.pending(ep);
                if n > 0 {
                    let b = egui::Rect::from_min_size(r.rect.right_top() + egui::vec2(-19.0, 5.0), egui::vec2(14.0, 14.0));
                    ui.painter().rect_filled(b, egui::Rounding::ZERO, theme::EMBER);
                    ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, format!("{n}"), egui::FontId::new(9.0, theme::data()), theme::G0);
                }
            }
            if r.clicked() {
                *u.page = *p;
            }
            let note = match (p.editor(), u.ed.as_deref()) {
                (Some(ep), Some(ed)) => ed.status(ep),
                (Some(_), None) => "no build open".into(),
                (None, _) => format!("{textured}/{} submeshes textured", view.submeshes.len()),
            };
            r.on_hover_text(format!("{}  ({})", p.label(), note));
        }
    });

    if *u.page == Page::Settings {
        settings_page(ctx, u.settings);
        return;
    }

    // ── EDITOR PAGES ──
    if let Some(ed_page) = u.page.editor() {
        match u.ed.as_deref_mut() {
            Some(ed) => {
                egui::SidePanel::left("ed_nav").resizable(true).default_width(320.0).width_range(260.0..=480.0).show(ctx, |ui| ed.nav(ui, ed_page));
                egui::SidePanel::right("ed_side").resizable(true).default_width(350.0).width_range(290.0..=480.0).show(ctx, |ui| ed.side(ui, ed_page));
                ed.central(ctx, ed_page);
            }
            None => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(28.0);
                    ui.label(theme::poster_text("NO BUILD OPEN", 22.0, theme::TX));
                    ui.label(theme::data_text("This page reads the open build's text, templates and textures.", 11.0, theme::DIM));
                });
            }
        }
        return;
    }

    // ── TRANSPORT ──
    egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
        ui.add_space(4.0);
        match u.current {
            None => {
                ui.label(egui::RichText::new("No clip loaded — pick one from the navigator. Showing bind pose.").color(theme::DIM));
            }
            Some(c) => {
                ui.horizontal(|ui| {
                    ui.label(theme::data_text(&c.name, 11.5, theme::RED));
                    ui.separator();
                    let frame = if c.frame_duration > 0.0 { (u.playback.time / c.frame_duration).round() as i64 } else { 0 };
                    ui.label(
                        egui::RichText::new(format!("{:.3}s · {} frames · frame {}/{}", c.duration, c.frame_count, frame.max(0), c.frame_count.saturating_sub(1)))
                            .monospace()
                            .size(11.0)
                            .color(theme::DIM),
                    );
                });
                ui.horizontal(|ui| {
                    let transport = if u.playback.playing { format!("{} Pause", theme::sym::PAUSE) } else { format!("{} Play", theme::sym::PLAY) };
                    if theme::primary_button(ui, &transport, true).clicked() {
                        u.playback.playing = !u.playback.playing;
                    }
                    if theme::pill(ui, "loop", u.playback.looping).clicked() {
                        u.playback.looping = !u.playback.looping;
                    }
                    if theme::pill(ui, "grid", *u.show_grid).clicked() {
                        *u.show_grid = !*u.show_grid;
                    }
                    if theme::pill(ui, "textures", *u.show_textures).clicked() {
                        *u.show_textures = !*u.show_textures;
                    }
                    if theme::pill(ui, "lock root", *u.lock_root).clicked() {
                        *u.lock_root = !*u.lock_root;
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("speed").color(theme::DIM).size(11.0));
                    ui.add(egui::Slider::new(&mut u.playback.speed, 0.0..=3.0).fixed_decimals(2));
                    if ui.button("Reset").clicked() {
                        u.playback.time = 0.0;
                    }
                });
                let dur = c.duration.max(0.0001);
                let mut t = u.playback.time.clamp(0.0, dur);
                if ui.add(egui::Slider::new(&mut t, 0.0..=dur).text("time (s)").fixed_decimals(3)).changed() {
                    u.playback.time = t;
                    u.playback.playing = false;
                }
            }
        }
        ui.add_space(4.0);
    });

    navigator(ctx, u);
    inspector(ctx, u, bound);
}

/// The Inspect page's left panel: models, clips, the loaded model's textures, its rig.
fn navigator(ctx: &egui::Context, u: &mut UiCtx) {
    let view = u.view;
    egui::SidePanel::left("navigator")
        .resizable(true)
        .default_width(300.0)
        // Hard clamp: a resizable panel sizes to its content's minimum, so a row whose width is
        // derived from `available_width()` could walk the panel across the screen.
        .width_range(240.0..=460.0)
        .show(ctx, |ui| {
            theme::eyebrow(ui, "Browser");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for t in NavTab::ALL {
                    if theme::pill(ui, t.label(), *u.nav.tab == t).clicked() {
                        *u.nav.tab = t;
                    }
                }
            });
            ui.horizontal(|ui| {
                // Right-to-left: the clear button is placed first, then the field fills EXACTLY the
                // remainder — so total content == available.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(theme::sym::NO).clicked() {
                        u.nav.search.clear();
                    }
                    ui.add_sized([ui.available_width(), 20.0], egui::TextEdit::singleline(u.nav.search).hint_text("filter by name"));
                });
            });
            let needle = u.nav.search.to_ascii_lowercase();
            ui.add_space(2.0);

            match *u.nav.tab {
                NavTab::Models if u.opening => {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(theme::data_text("opening the build…", 11.0, theme::DIM));
                    });
                }
                NavTab::Models => {
                    let mut by_cat: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
                    let mut shown = 0usize;
                    for (i, a) in u.nav.browse_assets.iter().enumerate() {
                        if !needle.is_empty() && !a.name.to_ascii_lowercase().contains(&needle) {
                            continue;
                        }
                        by_cat.entry(u.nav.groups.category(&a.name, a.category)).or_default().push(i);
                        shown += 1;
                    }
                    ui.label(theme::data_text(format!("{shown} of {} asset(s)", u.nav.browse_assets.len()), 10.0, theme::FAINT));
                    ui.label(theme::data_text("click to load · parts on the right · right-click to regroup", 9.0, theme::FAINT));
                    ui.separator();
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for cat in crate::models::CATEGORIES {
                            let Some(idxs) = by_cat.get(*cat) else { continue };
                            let open = !needle.is_empty() || *cat == "Characters";
                            egui::CollapsingHeader::new(egui::RichText::new(format!("{cat}  ({})", idxs.len())).color(theme::TX).size(11.0))
                                .id_source(("modelcat", cat))
                                .default_open(open)
                                .show(ui, |ui| {
                                    for &i in idxs {
                                        let a = &u.nav.browse_assets[i];
                                        let sel = *u.nav.sel_asset == Some(i);
                                        let label = if a.parts.len() > 1 { format!("{}  ({} parts)", a.name, a.parts.len()) } else { a.name.clone() };
                                        let r = ui.selectable_label(sel, label);
                                        if r.clicked() {
                                            *u.nav.sel_asset = Some(i);
                                            *u.nav.sel_part = None;
                                            *u.nav.pending_model = Some((a.name.clone(), a.assembly()));
                                        }
                                        r.context_menu(|ui| {
                                            // Load-then-export: the bundle carries the texture bindings,
                                            // which exist once this asset's resolve lands.
                                            if ui.button("Export bundle").clicked() {
                                                *u.nav.sel_asset = Some(i);
                                                *u.nav.pending_model = Some((a.name.clone(), a.assembly()));
                                                *u.nav.export_after_load = true;
                                                ui.close_menu();
                                            }
                                            ui.separator();
                                            ui.label(theme::data_text("Move to group", 10.0, theme::FAINT));
                                            for c in crate::models::CATEGORIES {
                                                if ui.button(*c).clicked() {
                                                    *u.nav.pending_group = Some((i, (*c).to_string()));
                                                    ui.close_menu();
                                                }
                                            }
                                        });
                                    }
                                });
                        }
                    });
                }
                NavTab::Animations => {
                    let clips: &[ClipRow] = u.source.map(|s| s.clips()).unwrap_or(&[]);
                    if theme::pill(ui, "show all (incl. not on this rig)", *u.show_all).clicked() {
                        *u.show_all = !*u.show_all;
                    }
                    let on_rig: HashSet<usize> = view.playable.iter().copied().collect();
                    let rows: Vec<usize> = if *u.show_all { (0..clips.len()).collect() } else { view.playable.clone() };
                    let matches: Vec<usize> = rows.into_iter().filter(|&i| needle.is_empty() || clips[i].name.to_ascii_lowercase().contains(&needle)).collect();
                    ui.label(theme::data_text(format!("{} clip(s) · {} of {} on this rig", matches.len(), view.playable.len(), clips.len()), 10.0, theme::FAINT));
                    ui.separator();
                    ui.add_space(4.0);
                    let cur_name = u.current.as_ref().map(|c| c.name.clone());
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, matches.len(), |ui, range| {
                        for r in range {
                            let i = matches[r];
                            let c = &clips[i];
                            let selected = cur_name.as_deref() == Some(c.name.as_str());
                            let label = if on_rig.contains(&i) {
                                format!("{}  {:.2}s", c.name, c.duration)
                            } else {
                                format!("{}  {:.2}s  (not on this rig)", c.name, c.duration)
                            };
                            if ui.selectable_label(selected, label).clicked() {
                                *u.pending_clip = Some(i);
                            }
                        }
                    });
                }
                NavTab::Textures => {
                    let rows: Vec<usize> = (0..view.assets.len()).filter(|&i| needle.is_empty() || view.assets[i].name.to_ascii_lowercase().contains(&needle)).collect();
                    ui.label(theme::data_text(format!("{} of {} texture(s) in {}'s pool", rows.len(), view.assets.len(), view.model_name), 10.0, theme::FAINT));
                    ui.separator();
                    ui.add_space(4.0);
                    let tile = 88.0;
                    let cols = ((ui.available_width() / (tile + 6.0)).floor() as usize).max(1);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for chunk in rows.chunks(cols) {
                            ui.horizontal(|ui| {
                                for &i in chunk {
                                    let sel = *u.nav.selected_tex == Some(i);
                                    let resp = ui
                                        .vertical(|ui| {
                                            ui.set_width(tile);
                                            match u.mats.thumbs.get(i, ui.ctx(), &view.assets) {
                                                Ok(t) => {
                                                    ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(tile, tile)));
                                                }
                                                Err(e) => {
                                                    let (rr, _) = ui.allocate_exact_size(egui::vec2(tile, tile), egui::Sense::hover());
                                                    ui.painter().rect_filled(rr, egui::Rounding::ZERO, theme::RED_SOFT);
                                                    ui.painter().text(rr.center(), egui::Align2::CENTER_CENTER, "!", egui::FontId::new(14.0, theme::data()), theme::RED);
                                                    ui.label(theme::data_text(e, 8.5, theme::RED));
                                                }
                                            }
                                            ui.label(theme::data_text(&view.assets[i].name, 8.5, if sel { theme::RED } else { theme::DIM }));
                                        })
                                        .response
                                        .interact(egui::Sense::click());
                                    if resp.clicked() {
                                        *u.nav.selected_tex = Some(i);
                                    }
                                }
                            });
                        }
                    });
                }
                NavTab::Rig => {
                    let rows: Vec<usize> = (0..view.skel.len()).filter(|&i| needle.is_empty() || view.skel[i].name.to_ascii_lowercase().contains(&needle)).collect();
                    ui.label(theme::data_text(format!("{} of {} bone(s)", rows.len(), view.skel.len()), 10.0, theme::FAINT));
                    ui.separator();
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, rows.len(), |ui, range| {
                        for r in range {
                            let i = rows[r];
                            let indent = if needle.is_empty() { view.bone_depth.get(i).copied().unwrap_or(0) as f32 * 10.0 } else { 0.0 };
                            ui.horizontal(|ui| {
                                ui.add_space(indent);
                                let sel = *u.nav.selected_bone == Some(i);
                                if ui.selectable_label(sel, egui::RichText::new(format!("{i:3}  {}", view.skel[i].name)).monospace().size(10.5)).clicked() {
                                    *u.nav.selected_bone = Some(i);
                                }
                            });
                        }
                    });
                }
            }
        });
}

/// The Inspect page's right panel: the model, its parts, the clip, the selected texture or bone, and
/// the Materials panel.
fn inspector(ctx: &egui::Context, u: &mut UiCtx, bound: usize) {
    let view = u.view;
    egui::SidePanel::right("inspector")
        .resizable(true)
        .default_width(340.0)
        .width_range(280.0..=460.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                theme::card(ui, "Model", Some(&view.model_name), |ui| {
                    if let Some(src) = u.source {
                        theme::kv(ui, "build", egui::RichText::new(src.version().label()));
                    }
                    theme::kv(ui, "vertices", egui::RichText::new(view.mesh_stats.0.to_string()));
                    theme::kv(ui, "triangles", egui::RichText::new(view.mesh_stats.1.to_string()));
                    theme::kv(ui, "bones", egui::RichText::new(view.mesh_stats.2.to_string()));
                    for (l, p, t) in &view.levels {
                        theme::kv(ui, &format!("LOD{l}"), egui::RichText::new(format!("{p} parts · {t} tris")));
                    }
                    let clips = u.source.map(|s| s.clips().len()).unwrap_or(0);
                    theme::kv(ui, "clips on rig", egui::RichText::new(format!("{} of {clips}", view.playable.len())));
                    if !view.states.is_empty() {
                        // The template's damage parts split the geometry by group state; one state is
                        // drawn at a time, 0 (intact) first.
                        ui.horizontal(|ui| {
                            ui.label(theme::data_text("damage state", 10.0, theme::DIM));
                            for &s in &view.states {
                                if theme::pill(ui, &if s == 0 { "0 intact".to_string() } else { s.to_string() }, *u.damage_state == s).clicked() {
                                    *u.damage_state = s;
                                }
                            }
                        });
                    }
                    for n in &view.notes {
                        ui.label(egui::RichText::new(n).color(theme::RED).size(10.0));
                    }
                });

                // Parts of the selected asset — each loadable on its own, each record describable.
                if let Some(a) = u.nav.sel_asset.and_then(|ai| u.nav.browse_assets.get(ai)) {
                    let badge = format!("{}", a.parts.len());
                    theme::card(ui, "Parts", Some(&badge), |ui| {
                        ui.label(theme::data_text(&a.name, 10.5, theme::RED));
                        ui.add_space(3.0);
                        for p in &a.parts {
                            let sel = *u.nav.sel_part == Some(p.mesh_index);
                            let r = ui.selectable_label(sel, egui::RichText::new(format!("{}   {}", p.label, p.name)).monospace().size(10.5));
                            if r.clicked() {
                                *u.nav.sel_part = Some(p.mesh_index);
                                // One part on its own: its geometry is shown as-is (level 0).
                                *u.nav.pending_model = Some((p.name.clone(), Assembly { picks: vec![(p.mesh_index, 0)], notes: Vec::new(), damage: a.damage.clone() }));
                            }
                        }
                        if let (Some(m), Some(src)) = (*u.nav.sel_part, u.source) {
                            ui.add_space(4.0);
                            for (k, v) in src.describe_mesh(m) {
                                theme::kv(ui, &k, egui::RichText::new(v).monospace());
                            }
                        }
                    });
                }

                if let Some(c) = u.current {
                    let (b, w) = c.bound();
                    theme::card(ui, "Clip", None, |ui| {
                        theme::kv(ui, "name", egui::RichText::new(&c.name).color(theme::RED));
                        theme::kv(ui, "duration", egui::RichText::new(format!("{:.3} s", c.duration)));
                        theme::kv(ui, "frames", egui::RichText::new(c.frame_count.to_string()));
                        theme::kv(ui, "tracks", egui::RichText::new(c.clip.num_transform_tracks().to_string()));
                        theme::kv(ui, "bound on rig", egui::RichText::new(format!("{b} of {w}")).color(if b < w { theme::RED } else { theme::TX }));
                    });
                }

                match *u.nav.tab {
                    NavTab::Textures => {
                        if let Some(a) = u.nav.selected_tex.and_then(|i| view.assets.get(i).map(|a| (i, a))) {
                            let (i, a) = a;
                            theme::card(ui, "Texture", Some(&a.name), |ui| {
                                match u.mats.thumbs.get(i, ui.ctx(), &view.assets) {
                                    Ok(t) => {
                                        ui.add(egui::Image::new(&t).fit_to_exact_size(egui::vec2(256.0, 256.0)).maintain_aspect_ratio(true));
                                    }
                                    Err(e) => {
                                        ui.label(egui::RichText::new(e).color(theme::RED).size(10.0));
                                    }
                                }
                                theme::kv(ui, "size", egui::RichText::new(format!("{}×{}", a.width, a.height)));
                                theme::kv(ui, "format", egui::RichText::new(&a.format).monospace());
                                theme::kv(ui, "role", egui::RichText::new(a.role.label()));
                                let users: Vec<String> = view
                                    .submesh_tex
                                    .iter()
                                    .enumerate()
                                    .flat_map(|(k, b)| b.iter_bound().filter(|(_, ai)| *ai == i).map(move |(s, _)| format!("{k:02} {}", SubmeshBinding::SLOT_NAMES[s])))
                                    .collect();
                                theme::kv(ui, "bound to", egui::RichText::new(if users.is_empty() { "nothing".into() } else { users.join(", ") }));
                            });
                        }
                    }
                    NavTab::Rig => {
                        if let Some(b) = u.nav.selected_bone.and_then(|i| view.skel.get(i).map(|b| (i, b))) {
                            let (i, bone) = b;
                            theme::card(ui, "Bone", Some(&bone.name), |ui| {
                                let h = view.rig_hashes.get(i).copied().unwrap_or(0);
                                theme::kv(ui, "index", egui::RichText::new(i.to_string()));
                                theme::kv(ui, "hash", egui::RichText::new(format!("{h:08X}")).monospace());
                                let parent = if bone.parent >= 0 { view.skel.get(bone.parent as usize).map(|p| p.name.clone()).unwrap_or_default() } else { "(root)".into() };
                                theme::kv(ui, "parent", egui::RichText::new(parent));
                                theme::kv(ui, "translation", egui::RichText::new(format!("{:.3}, {:.3}, {:.3}", bone.t[0], bone.t[1], bone.t[2])).monospace());
                                theme::kv(ui, "rotation", egui::RichText::new(format!("{:.3}, {:.3}, {:.3}, {:.3}", bone.r[0], bone.r[1], bone.r[2], bone.r[3])).monospace());
                                theme::kv(ui, "scale", egui::RichText::new(format!("{:.3}, {:.3}, {:.3}", bone.s[0], bone.s[1], bone.s[2])).monospace());
                                if let Some(c) = u.current {
                                    let tracks: Vec<String> = c.track_to_bone.iter().enumerate().filter(|(_, b)| **b == i as i32).map(|(t, _)| t.to_string()).collect();
                                    theme::kv(ui, "clip tracks", egui::RichText::new(if tracks.is_empty() { "none — holds its bind pose".into() } else { tracks.join(", ") }));
                                }
                            });
                        }
                    }
                    _ => {}
                }

                materials(ui, view, &mut u.mats, bound);
            });
        });
}

/// The Materials panel: every submesh's binding, per slot, with a picker over the model's pool.
fn materials(ui: &mut egui::Ui, view: &View, mats: &mut MatsCtx, bound: usize) {
    let badge = format!("{bound}/{} bound", view.submeshes.len());
    theme::section(ui, "Materials", Some(&badge), true, |ui| {
        if !view.loaded {
            ui.label(egui::RichText::new("No model loaded.").color(theme::FAINT).size(10.5));
            return;
        }
        if view.assets.is_empty() {
            ui.colored_label(theme::RED, "The build's material data bound no texture to this model.");
            return;
        }
        ui.label(
            egui::RichText::new("Bound from the build's own material data. Change any slot here — your picks are saved for this model and win over the build's.")
                .color(theme::FAINT)
                .size(10.5),
        );
        ui.horizontal(|ui| {
            if theme::stamp_button(ui, "Save bindings", true).on_hover_text(resolve::sidecar_path(&view.tex_profile)).clicked() {
                *mats.pending_save = true;
            }
        });
        ui.add_space(6.0);
        for (i, sm) in view.submeshes.iter().enumerate() {
            let binding = view.submesh_tex[i];
            let (fill, border) = match (binding.is_bound(), *mats.selected == Some(i)) {
                (_, true) => (theme::RED_SOFT, theme::RED),
                (true, false) => (theme::EMBER_SOFT, theme::EMBER_DK),
                (false, false) => (theme::G0, theme::LINE),
            };
            theme::row_chip(ui, fill, border, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let ix = if binding.is_bound() { theme::EMBER } else { theme::FAINT };
                        if ui.add(egui::Label::new(theme::data_text(format!("{i:02}"), 10.0, ix)).sense(egui::Sense::click())).clicked() {
                            *mats.selected = Some(i);
                        }
                        ui.label(theme::data_text(format!("LOD{} · {} tris", sm.lod, sm.index_count / 3), 10.0, theme::DIM));
                        let hashes: Vec<String> = sm.materials.iter().map(|m| format!("{m:08X}")).collect();
                        ui.label(theme::data_text(hashes.join(" "), 9.0, theme::FAINT)).on_hover_text("the drawcalls' material hashes");
                        // A binding never pretends to be a decision: say how we got it.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let (label, kind) = match view.submesh_prov[i] {
                                Prov::Bound => ("bound", theme::Badge::Lit),
                                Prov::Seeded => ("seeded", theme::Badge::Outline),
                                Prov::Unresolved => ("unresolved", theme::Badge::Muted),
                            };
                            theme::badge(ui, label, kind);
                        });
                    });
                    // One combo per slot — 0=diffuse, 1=spec, 2=normal, 3=mask. Role-matching pool
                    // entries first; the rest stay reachable under a separator.
                    for slot in 0..SubmeshBinding::NUM_SLOTS {
                        let want_role = SubmeshBinding::SLOT_ROLES[slot];
                        let slot_name = SubmeshBinding::SLOT_NAMES[slot];
                        let assigned = binding.slot(slot);
                        let cur = assigned.and_then(|ai| view.assets.get(ai)).map(|a| a.name.as_str()).unwrap_or("(none)");
                        ui.horizontal(|ui| {
                            let thumb_size = egui::vec2(48.0, 48.0);
                            match assigned.map(|ai| mats.thumbs.get(ai, ui.ctx(), &view.assets)) {
                                Some(Ok(handle)) => {
                                    ui.add(egui::Image::new(&handle).fit_to_exact_size(thumb_size).maintain_aspect_ratio(true));
                                }
                                other => {
                                    let (rect, r) = ui.allocate_exact_size(thumb_size, egui::Sense::hover());
                                    ui.painter().rect_filled(rect, 2.0, if matches!(other, Some(Err(_))) { theme::RED_SOFT } else { theme::G0 });
                                    ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::LINE));
                                    if let Some(Err(e)) = other {
                                        r.on_hover_text(e);
                                    }
                                }
                            }
                            let tag_color = if assigned.is_some() { theme::EMBER } else { theme::FAINT };
                            ui.add_sized([40.0, 18.0], egui::Label::new(theme::data_text(slot_name, 10.0, tag_color)));
                            egui::ComboBox::from_id_source(("texpick", i, slot))
                                .selected_text(egui::RichText::new(cur).size(11.0))
                                .width((ui.available_width() - 8.0).max(0.0))
                                .show_ui(ui, |ui| {
                                    if ui.selectable_label(assigned.is_none(), "(none)").clicked() {
                                        *mats.pending_tex = Some((i, slot as u8, None));
                                    }
                                    let mut any_role = false;
                                    for (ai, a) in view.assets.iter().enumerate().filter(|(_, a)| a.role == want_role) {
                                        any_role = true;
                                        if ui.selectable_label(assigned == Some(ai), format!("{}  · {}", a.name, a.role.label())).clicked() {
                                            *mats.pending_tex = Some((i, slot as u8, Some(ai)));
                                        }
                                    }
                                    if any_role {
                                        ui.separator();
                                    }
                                    for (ai, a) in view.assets.iter().enumerate().filter(|(_, a)| a.role != want_role) {
                                        if ui.selectable_label(assigned == Some(ai), format!("{}  · {}", a.name, a.role.label())).clicked() {
                                            *mats.pending_tex = Some((i, slot as u8, Some(ai)));
                                        }
                                    }
                                });
                        });
                        if let Some(a) = assigned.and_then(|ai| view.assets.get(ai)) {
                            ui.label(egui::RichText::new(format!("    {}x{} · {} · {}", a.width, a.height, a.format, a.role.label())).monospace().size(9.5).color(theme::FAINT));
                        }
                    }
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verb bar shows the destination's tail, not a 100-character absolute path.
    #[test]
    fn short_path_keeps_the_last_two_components() {
        assert_eq!(short_path(r"C:\Users\Shadow\Desktop\repo\tools\sab_workshop\workshop_export\Sean"), "…/workshop_export/Sean");
        assert_eq!(short_path("/home/x/workshop_export/Sean"), "…/workshop_export/Sean");
        // Too short to trim: hand it back rather than inventing an ellipsis.
        assert_eq!(short_path("Sean"), "Sean");
    }
}
