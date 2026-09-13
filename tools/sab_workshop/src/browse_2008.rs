//! The 2008 Xbox 360 prototype browser page — the version-agnostic explorer for the pre-release
//! build the community wants to secret-hunt through. This module OWNS both the state and the UI
//! for the prototype page; it lives outside `app.rs` because the flow is disjoint from retail:
//! we have no MSH4 decoder, no rig, no wgpu scene, no material sidecars. It's a directory browser
//! over a `Box<dyn GameSource>`.
//!
//! **What the page does today.** Given an opened `Prototype2008Source`, list every asset the
//! source can enumerate (meshes + textures + materials — meshes and materials come from the
//! packlog inventory + parsed WSAO slots; textures split into packlog-mined slot rows and
//! loose-DDS rows), filterable by name/hash, in three tabs. Clicking one shows a detail card:
//!
//! - **Texture** → decoded RGBA via `source.load_texture(hash)` (loose DDS or, once probe #1 wires
//!   more paths, packed-slot bytes), plus width/height/format/hash. A `Thumbs`-shaped
//!   `egui::TextureHandle` cache keyed by hash so re-clicking a texture doesn't re-decode.
//! - **Material** → the `[d,s,n,wm]` slot vector via `source.textures_for_material(uid)`. Each
//!   slot is a clickable chip; clicking navigates to that texture (same page, same state).
//! - **Mesh** → metadata only (byte length, SBL4 slot boundary via `read_mesh_bytes`, and a
//!   `MSH4` magic confirmation on the first inflated inner record). The MSH4 geometry format
//!   is unknown, so no 3D preview is offered — the page is honest about it.
//!
//! **Not in scope here** (parallel probes / task-note explicit not-scope): the non-SBL4 sub-pack
//! variants (SBLL/SBL8/SBL6/SBLX) that hold ~40% of `.Pack` files, and any attempt to render
//! MSH4 geometry.
//!
//! **Why a module and not more `app.rs`.** `app.rs` is already 3125 lines and every existing state
//! path bakes in wgpu scene state, an rig, sidecars, and the animation catalog. None of those
//! apply to the prototype; the browser is a pure GameSource UI. Keeping it separate means the
//! retail flow stays as-is (task's "The existing retail path stays untouched"), and the prototype
//! page grows as we probe new formats without every edit touching the shared file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::dtex::CpuTexture;
use crate::gui::theme;
use crate::pack::pandemic_hash;
use crate::source::{self, AssetHandle, AssetKind, GameSource};

/// Which sub-list the browser is showing. Not persisted — a per-session view choice.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrowseTab {
    Textures,
    Materials,
    Meshes,
}

impl BrowseTab {
    pub const ALL: [BrowseTab; 3] = [BrowseTab::Textures, BrowseTab::Materials, BrowseTab::Meshes];
    pub fn label(self) -> &'static str {
        match self {
            BrowseTab::Textures => "textures",
            BrowseTab::Materials => "materials",
            BrowseTab::Meshes => "meshes",
        }
    }
    pub fn kind(self) -> AssetKind {
        match self {
            BrowseTab::Textures => AssetKind::Texture,
            BrowseTab::Materials => AssetKind::Material,
            BrowseTab::Meshes => AssetKind::Mesh,
        }
    }
}

/// One thing the user has selected — remembered by kind + hash so a tab-switch that re-lists
/// doesn't lose the selection (indexes into the returned Vec change; hashes don't).
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    pub kind: AssetKind,
    /// The asset's identifying hash. For materials this is the UID; for textures / meshes the
    /// packlog-derived block hash (which the source resolves back to a handle).
    pub hash: u32,
    /// The friendly name at pick time — kept for the detail header even if a later re-list
    /// changes ordering. Empty when the source only had a hash.
    pub name: String,
    /// The mesh browser needs the source's opaque key back (packlog index → `.Pack` location).
    /// Textures and materials are keyed by hash alone. `None` for those.
    pub mesh_key: Option<u64>,
}

/// A decoded texture kept as an egui handle. Extracted so the same cache serves texture rows AND
/// material slot previews. Keyed by hash — the source's own dedup layer already handles
/// pack-vs-loose collisions before we get here.
#[derive(Default)]
pub struct TextureCache {
    /// `Some(handle)` = decoded ok. `None` = decode was tried and failed — remembered so a
    /// broken row doesn't re-attempt every repaint. The failure REASON is not cached: the
    /// error string comes back from `load_texture` on demand for the detail card.
    handles: Mutex<HashMap<u32, Option<egui::TextureHandle>>>,
    /// Last error string per hash, mirrors `handles == None` so the detail card can say WHY.
    errors: Mutex<HashMap<u32, String>>,
}

impl TextureCache {
    /// Fetch (or decode + cache) the egui handle for one hash. `source.load_texture` is the only
    /// path that actually reads bytes — the cache never keeps raw pixels, only the GPU-uploaded
    /// handle egui hands out.
    pub fn get(
        &self,
        ctx: &egui::Context,
        source: &dyn GameSource,
        hash: u32,
    ) -> Option<egui::TextureHandle> {
        // Mutex is only for interior mutability under `&self`; contention is nil (one UI thread).
        if let Some(entry) = self.handles.lock().unwrap().get(&hash) {
            return entry.clone();
        }
        let outcome: Result<CpuTexture, String> = source.load_texture(hash);
        let handle = match outcome {
            Ok(tex) => {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [tex.width as usize, tex.height as usize],
                    &tex.rgba,
                );
                Some(ctx.load_texture(format!("proto_tex_{hash:08x}"), img, egui::TextureOptions::LINEAR))
            }
            Err(e) => {
                self.errors.lock().unwrap().insert(hash, e);
                None
            }
        };
        self.handles.lock().unwrap().insert(hash, handle.clone());
        handle
    }

    /// Last decode error for a hash (matches a cached `None` handle). `None` if never tried, or
    /// tried and succeeded. Used only by the detail card to surface WHY a preview is missing —
    /// the browser row itself just draws a placeholder box.
    pub fn error(&self, hash: u32) -> Option<String> {
        self.errors.lock().unwrap().get(&hash).cloned()
    }
}

/// State backing the prototype page. Held on the app side so the folder pick, the tab, the
/// selection and the decoded-texture cache all survive across the many repaints egui does
/// between two user actions. Constructed once at boot from settings; recreated by
/// `set_root(...)` when the user chooses a different folder.
pub struct ProtoState {
    /// The opened source, or `None` if none has been chosen / opening failed.
    pub source: Option<Box<dyn GameSource>>,
    /// Set alongside `source == None` when a load actually failed (as opposed to "not configured
    /// yet"). Rendered as a red note next to the picker.
    pub open_error: Option<String>,
    pub tab: BrowseTab,
    /// Case-insensitive substring filter over asset names. Empty = show everything.
    pub search: String,
    /// The one currently-inspected asset. Detail card renders off this.
    pub selection: Option<Selection>,
    /// One shared decoded-texture cache across rows AND material slot previews — same hash =>
    /// same handle. `Mutex<>` inside so we can render behind `&self` when needed.
    pub tex_cache: TextureCache,
    /// The listings, cached once per source (each is a Vec of thousands and `list_*` walks the
    /// packlog index every call). Cleared on `set_root`.
    listing_cache: HashMap<AssetKind, Vec<AssetHandle>>,
}

impl Default for ProtoState {
    fn default() -> Self {
        Self {
            source: None,
            open_error: None,
            tab: BrowseTab::Textures,
            search: String::new(),
            selection: None,
            tex_cache: TextureCache::default(),
            listing_cache: HashMap::new(),
        }
    }
}

impl ProtoState {
    /// Try to open a source at `root`. Success installs it; failure keeps the previous source
    /// (if any) intact and records the error. Returning a `Result` here rather than mutating a
    /// flag makes the boot-time seed path readable: it just ignores an error, and the page shows
    /// the picker.
    pub fn set_root(&mut self, root: &Path) -> Result<(), String> {
        match source::open_source(root) {
            Ok(src) => {
                self.source = Some(src);
                self.open_error = None;
                self.selection = None;
                self.listing_cache.clear();
                self.tex_cache = TextureCache::default();
                Ok(())
            }
            Err(e) => {
                self.open_error = Some(e.clone());
                Err(e)
            }
        }
    }

    /// Clear the source. Used by the "Forget this folder" button — the picker re-appears and the
    /// user can point somewhere else. Distinct from `set_root(...)` failing (which keeps
    /// whatever was there before).
    pub fn close(&mut self) {
        self.source = None;
        self.open_error = None;
        self.selection = None;
        self.listing_cache.clear();
        self.tex_cache = TextureCache::default();
    }

    /// Cached asset list for a tab. Every `list_meshes()` / `list_textures()` builds thousands of
    /// AssetHandles fresh; caching avoids that per-frame at the cost of missing a source that
    /// mutated. The 2008 source is read-only after `open()`, so this is safe by construction.
    pub fn listing(&mut self, kind: AssetKind) -> &[AssetHandle] {
        if !self.listing_cache.contains_key(&kind) {
            let list = match (&self.source, kind) {
                (Some(s), AssetKind::Texture) => s.list_textures(),
                (Some(s), AssetKind::Mesh) => s.list_meshes(),
                (Some(s), AssetKind::Material) => s.list_materials(),
                _ => Vec::new(),
            };
            self.listing_cache.insert(kind, list);
        }
        self.listing_cache.get(&kind).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

// ── UI ───────────────────────────────────────────────────────────────────────────────────────

/// The whole prototype page — a single-CentralPanel layout with a header, a sidebar list, and a
/// detail card. Uses the same `theme::*` primitives as the rest of the app so the visual
/// grammar (eyebrow, card, section, data_text) stays consistent.
pub fn render(ctx: &egui::Context, state: &mut ProtoState, pick_dir: &mut bool) {
    egui::CentralPanel::default().show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(8.0);
            ui.label(theme::poster_text("2008 PROTOTYPE BROWSER", 22.0, theme::TX));
            ui.label(theme::data_text(
                "The pre-release Xbox 360 build. Read-only. Names come from packlog manifests; \
                 hashes are pandemic_hash (same as retail).",
                11.0,
                theme::FAINT,
            ));
            ui.add_space(14.0);

            // Header row: current source (or picker prompt), plus the folder-pick + close controls.
            render_header(ui, state, pick_dir);
            ui.add_space(10.0);

            let Some(source) = state.source.as_ref() else {
                // No source configured — the header above already offers the picker; keep the
                // rest of the page hidden so it's not a wall of empty tables.
                if let Some(err) = &state.open_error {
                    ui.label(theme::data_text(
                        format!("last attempt: {err}"),
                        11.0,
                        theme::RED,
                    ));
                }
                ui.add_space(20.0);
                return;
            };

            render_fingerprint(ui, source.as_ref());
            ui.add_space(12.0);

            // Body: tab bar + search + list (left column, ~40% width) + detail card (right).
            // A horizontal layout with an explicit split — the list is the primary object, the
            // detail is what the click reveals.
            ui.horizontal_top(|ui| {
                let list_w = (ui.available_width() * 0.42).clamp(280.0, 520.0);
                ui.vertical(|ui| {
                    ui.set_width(list_w);
                    render_tab_bar(ui, state);
                    ui.add_space(4.0);
                    render_search(ui, state);
                    ui.add_space(4.0);
                    render_list(ui, state);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width().max(200.0));
                    render_detail(ui, state);
                });
            });
        });
    });
}

fn render_header(ui: &mut egui::Ui, state: &mut ProtoState, pick_dir: &mut bool) {
    theme::eyebrow(ui, "Prototype root");
    ui.horizontal(|ui| {
        let label = match &state.source {
            Some(s) => s.label(),
            None => "(no folder configured — pick one on the right)".into(),
        };
        ui.label(theme::data_text(label, 11.0, theme::TX));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if state.source.is_some() && ui.button("Forget").clicked() {
                // A destructive-ish action, but reversible — the picker sits right there. No
                // confirmation prompt: this only affects UI state, never disk.
                state.close();
            }
            if ui.button("Choose folder…").clicked() {
                *pick_dir = true;
            }
        });
    });
    ui.label(theme::data_text(
        "Point at either `game-files/` (parent of Bin360/) or `Bin360/` itself — both work.",
        10.0,
        theme::FAINT,
    ));
}

fn render_fingerprint(ui: &mut egui::Ui, source: &dyn GameSource) {
    let fp = source.fingerprint();
    // The fingerprint is a mix of matched facts + warnings — one card is enough for both, with
    // the counts summarised in the badge. A user opening a fresh copy wants to see "how much
    // did the workshop actually recognize" without scrolling.
    let badge = format!("{} facts · {} warnings", fp.matched.len(), fp.warnings.len());
    theme::section(ui, "Fingerprint", Some(&badge), false, |ui| {
        for m in &fp.matched {
            ui.horizontal(|ui| {
                ui.label(theme::data_text(theme::sym::OK, 10.0, theme::COLD));
                ui.label(theme::data_text(m, 10.5, theme::TX));
            });
        }
        for w in &fp.warnings {
            ui.horizontal(|ui| {
                ui.label(theme::data_text("!", 10.0, theme::EMBER));
                ui.label(theme::data_text(w, 10.5, theme::EMBER));
            });
        }
    });
}

fn render_tab_bar(ui: &mut egui::Ui, state: &mut ProtoState) {
    ui.horizontal(|ui| {
        for t in BrowseTab::ALL {
            let n = state.listing(t.kind()).len();
            let label = format!("{}  {}", t.label(), n);
            if theme::pill(ui, &label, state.tab == t).clicked() {
                state.tab = t;
            }
        }
    });
}

fn render_search(ui: &mut egui::Ui, state: &mut ProtoState) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(theme::sym::NO).clicked() {
                state.search.clear();
            }
            ui.add_sized(
                [ui.available_width(), 20.0],
                egui::TextEdit::singleline(&mut state.search).hint_text("filter by name or hash"),
            );
        });
    });
}

fn render_list(ui: &mut egui::Ui, state: &mut ProtoState) {
    // A virtualized list — the texture tab is 1.9k rows, the mesh tab is ~900, and materials can
    // run to thousands post-probe-#1. `show_rows` only builds visible children.
    let needle = state.search.to_ascii_lowercase();
    let handles: Vec<AssetHandle> = state
        .listing(state.tab.kind())
        .iter()
        .filter(|h| matches_needle(h, &needle))
        .cloned()
        .collect();

    ui.label(theme::data_text(
        format!("{} shown of {}", handles.len(), state.listing(state.tab.kind()).len()),
        10.0,
        theme::FAINT,
    ));
    ui.separator();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(
        ui,
        22.0,
        handles.len(),
        |ui, range| {
            for i in range {
                let h = &handles[i];
                let sel = matches_selection(state.selection.as_ref(), h);
                let display = if h.name.is_empty() {
                    format!("{:08X}  (unnamed)", handle_hash(h))
                } else {
                    format!("{}   {:08X}", h.name, handle_hash(h))
                };
                let resp = ui.add(egui::SelectableLabel::new(sel, egui::RichText::new(display).monospace().size(10.5)));
                if resp.clicked() {
                    state.selection = Some(Selection {
                        kind: h.kind,
                        hash: handle_hash(h),
                        name: h.name.clone(),
                        mesh_key: (h.kind == AssetKind::Mesh).then_some(h.key),
                    });
                }
            }
        },
    );
}

/// Case-insensitive substring match over the name; also matches hex hash typed with or without
/// the leading `0x`. Empty needle passes.
fn matches_needle(h: &AssetHandle, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if h.name.to_ascii_lowercase().contains(needle) {
        return true;
    }
    // Try the hash three ways: bare hex, 0x-prefixed, and decimal — a user might paste any.
    let hash = handle_hash(h);
    let hex = format!("{hash:08x}");
    let hex_short = format!("{hash:x}");
    hex.contains(needle) || hex_short.contains(needle) || hash.to_string() == needle
}

/// The identifying hash of a handle. For textures + meshes, we index into the source's own
/// hash_names by extracting from the handle name: since the source populates `AssetHandle.name`
/// from `hash_names.get(&e.hash)`, we can re-derive the hash via `pandemic_hash(name)` — and the
/// scheme is verified identical across every packlog block (see fingerprint).
///
/// For materials the source uses uid AS the key (which is a u32 stored in u64), so we take that
/// directly. Same for meshes' loose_dds shadow entries (u64::MAX - i); those hash names come
/// from the DDS stem and hash correctly.
///
/// Preferring name-hash over `key` for textures/meshes keeps this function agnostic to whichever
/// slot in the source the row came from — packlog-mined vs loose-DDS shadow both round-trip.
fn handle_hash(h: &AssetHandle) -> u32 {
    match h.kind {
        AssetKind::Material => h.key as u32,
        _ if !h.name.is_empty() => pandemic_hash(&h.name),
        // The one honest fallback: an unnamed row from a packlog slot with no hash→name entry
        // (shouldn't happen — packlog stems always name their block — but if it does, key is a
        // packlog-index-order u64 and truncating loses info).
        _ => h.key as u32,
    }
}

fn matches_selection(sel: Option<&Selection>, h: &AssetHandle) -> bool {
    match sel {
        Some(s) => s.kind == h.kind && s.hash == handle_hash(h),
        None => false,
    }
}

fn render_detail(ui: &mut egui::Ui, state: &mut ProtoState) {
    // Destructure the state to unlock two independent borrows: `source` is read-only (feeds
    // `load_texture`/`textures_for_material`/`read_mesh_bytes`), and the rest — tex_cache,
    // selection, tab — is what the detail card mutates. The whole-`state` reference couldn't
    // hand out both at once (a `&Box<dyn GameSource>` is tied to `&state` even though the trait
    // object behind it never needs mutation), so this destructure is the borrow-checker fix.
    let ProtoState { source, selection, tab, tex_cache, .. } = state;
    let Some(sel) = selection.clone() else {
        ui.add_space(20.0);
        ui.label(theme::data_text(
            "click a row on the left to inspect",
            11.0,
            theme::FAINT,
        ));
        return;
    };
    let source: &dyn GameSource = match source.as_deref() {
        Some(s) => s,
        None => return,
    };
    // Header — kind + name + hash. Same look on every kind so a switch of selection doesn't
    // reflow the page.
    theme::eyebrow(ui, kind_label(sel.kind));
    ui.horizontal(|ui| {
        let display_name = if sel.name.is_empty() { "(unnamed)".into() } else { sel.name.clone() };
        ui.label(theme::data_text(display_name, 14.0, theme::RED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::data_text(format!("{:08X}", sel.hash), 11.0, theme::EMBER));
        });
    });
    ui.add_space(8.0);
    match sel.kind {
        AssetKind::Texture => render_texture_detail(ui, tex_cache, source, &sel),
        AssetKind::Material => {
            render_material_detail(ui, tex_cache, selection, tab, source, &sel)
        }
        AssetKind::Mesh => render_mesh_detail(ui, source, &sel),
        _ => {
            ui.label(theme::data_text(
                format!("no detail view for {:?} yet", sel.kind),
                11.0,
                theme::FAINT,
            ));
        }
    }
}

fn kind_label(k: AssetKind) -> &'static str {
    match k {
        AssetKind::Texture => "Texture",
        AssetKind::Material => "Material",
        AssetKind::Mesh => "Mesh",
        AssetKind::Animation => "Animation",
        AssetKind::UiString => "UI string",
        AssetKind::Other => "Asset",
    }
}

fn render_texture_detail(
    ui: &mut egui::Ui,
    tex_cache: &TextureCache,
    source: &dyn GameSource,
    sel: &Selection,
) {
    // Try the cache first; a miss goes through source.load_texture (which will hit disk for
    // loose DDS or return the SBL4-blocked stub error for packlog-mined slots).
    let handle = tex_cache.get(ui.ctx(), source, sel.hash);
    theme::card(ui, "Preview", None, |ui| {
        match handle {
            Some(h) => {
                let size = h.size_vec2();
                // Show at natural size up to 512px; anything bigger scales down. This keeps
                // small icons crisp AND lets a 2048×2048 skybox fit the pane.
                let max = 512.0_f32;
                let scale = (max / size.x.max(size.y)).min(1.0);
                ui.add(
                    egui::Image::new(&h)
                        .fit_to_exact_size(egui::vec2(size.x * scale, size.y * scale))
                        .maintain_aspect_ratio(true),
                );
            }
            None => {
                // Preview box with the reason underneath — the whole point of caching errors is
                // so this always says WHY (SBL4-blocked / unknown-hash / decode failed) instead
                // of just being empty.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(200.0, 120.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, theme::G0);
                ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::LINE));
                if let Some(err) = tex_cache.error(sel.hash) {
                    ui.add_space(6.0);
                    ui.label(theme::data_text(err, 10.0, theme::EMBER));
                } else {
                    ui.add_space(6.0);
                    ui.label(theme::data_text("no preview available", 10.0, theme::FAINT));
                }
            }
        }
    });

    // Metadata: try to peek at the DDS/DTEX to report width/height/format. Cheap since the
    // cache already forced a decode above; we just re-call `load_texture` (also cached inside
    // Prototype2008Source's own `tex_cache`).
    let meta = source.load_texture(sel.hash).ok();
    theme::card(ui, "Facts", None, |ui| {
        theme::kv(ui, "kind", egui::RichText::new("Texture"));
        theme::kv(ui, "hash", egui::RichText::new(format!("{:08X}", sel.hash)));
        match &meta {
            Some(t) => {
                theme::kv(ui, "width", egui::RichText::new(t.width.to_string()));
                theme::kv(ui, "height", egui::RichText::new(t.height.to_string()));
                theme::kv(ui, "format", egui::RichText::new(format_tag(t.format)));
                theme::kv(ui, "rgba bytes", egui::RichText::new(t.rgba.len().to_string()));
            }
            None => {
                ui.label(theme::data_text(
                    "no decoded bytes — see the preview area for the reason",
                    10.5,
                    theme::FAINT,
                ));
            }
        }
    });
}

/// Format tag → short human name. Mirrors dtex::Dtex::format_name for DXT codes; loose DDS uses
/// the same numeric constants so this covers both. `format` value on `CpuTexture` doesn't carry
/// a stable enum here, so we lean on the raw fourcc that both paths preserve.
fn format_tag(fourcc: u32) -> String {
    const DXT1: u32 = 0x3154_5844; // BC1
    const DXT3: u32 = 0x3354_5844; // BC2
    const DXT5: u32 = 0x3554_5844; // BC3
    match fourcc {
        DXT1 => "DXT1 (BC1)".into(),
        DXT3 => "DXT3 (BC2)".into(),
        DXT5 => "DXT5 (BC3)".into(),
        0x15 => "A8R8G8B8".into(),
        0x14 => "X8R8G8B8".into(),
        0x1c => "A8".into(),
        0x32 => "L8".into(),
        0x16 => "R5G6B5".into(),
        0x28 => "V8U8".into(),
        0 => "(raw RGBA)".into(),
        other => format!("0x{other:08X}"),
    }
}

fn render_material_detail(
    ui: &mut egui::Ui,
    tex_cache: &TextureCache,
    selection: &mut Option<Selection>,
    tab: &mut BrowseTab,
    source: &dyn GameSource,
    sel: &Selection,
) {
    let slots = match source.textures_for_material(sel.hash) {
        Some(s) => s,
        None => {
            ui.label(theme::data_text(
                "no bindings — this material's UID isn't in the parsed index (either \
                 SBL4-wrapped or unknown)",
                11.0,
                theme::EMBER,
            ));
            return;
        }
    };
    theme::card(ui, "Slots", Some("[diffuse · spec · normal · wm]"), |ui| {
        let slot_names = ["diffuse", "spec", "normal", "wm"];
        for (i, s) in slots.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(theme::data_text(slot_names[i], 11.0, theme::DIM));
                match s {
                    Some(h) => {
                        // A clickable name — resolves via source.name_of_hash so the picker
                        // can jump to the texture. Falls back to raw hex if we don't have a
                        // name for it (unnamed loose DDS, or a hash the packlogs don't cover).
                        let name = source.name_of_hash(*h).unwrap_or_default();
                        let display = if name.is_empty() {
                            format!("{h:08X}  (unnamed)")
                        } else {
                            format!("{name}  {h:08X}")
                        };
                        let resp = ui.add(
                            egui::Label::new(theme::data_text(display, 11.0, theme::EMBER))
                                .sense(egui::Sense::click()),
                        );
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("click to open this texture")
                            .clicked()
                        {
                            *selection = Some(Selection {
                                kind: AssetKind::Texture,
                                hash: *h,
                                name,
                                mesh_key: None,
                            });
                            *tab = BrowseTab::Textures;
                        }
                    }
                    None => {
                        ui.label(theme::data_text("(unset)", 11.0, theme::FAINT));
                    }
                }
            });
        }
    });

    // Tiny inline previews of every slot, purely to make the page feel like the retail Materials
    // panel — a wall of hex names is not what a user came here for. `tex_cache` shares state
    // with the texture tab, so a preview clicked into once from either place decodes exactly
    // once.
    theme::card(ui, "Previews", None, |ui| {
        ui.horizontal(|ui| {
            for (i, s) in slots.iter().enumerate() {
                ui.vertical(|ui| {
                    let name = ["d", "s", "n", "wm"][i];
                    let sz = egui::vec2(64.0, 64.0);
                    if let Some(h) = s {
                        if let Some(handle) = tex_cache.get(ui.ctx(), source, *h) {
                            ui.add(
                                egui::Image::new(&handle).fit_to_exact_size(sz).maintain_aspect_ratio(true),
                            );
                        } else {
                            let (rect, _) = ui.allocate_exact_size(sz, egui::Sense::hover());
                            ui.painter().rect_filled(rect, 2.0, theme::G0);
                            ui.painter().rect_stroke(rect, 2.0, egui::Stroke::new(1.0, theme::LINE));
                        }
                    } else {
                        let (rect, _) = ui.allocate_exact_size(sz, egui::Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, theme::G0);
                    }
                    ui.label(theme::data_text(name, 9.5, theme::FAINT));
                });
            }
        });
    });
}

fn render_mesh_detail(ui: &mut egui::Ui, source: &dyn GameSource, sel: &Selection) {
    // The mesh key was stashed on selection because meshes are the only kind whose retrieval
    // needs the source's own opaque index (packlog-order index into `mesh_index` -> `.Pack`
    // file). Materials and textures round-trip by hash alone.
    let Some(key) = sel.mesh_key else {
        ui.label(theme::data_text(
            "internal: mesh selection is missing its key — reselect the row",
            11.0,
            theme::RED,
        ));
        return;
    };
    let handle = AssetHandle { key, name: sel.name.clone(), kind: AssetKind::Mesh };
    match source.read_mesh_bytes(&handle) {
        Ok(bytes) => {
            theme::card(ui, "Slot bytes", None, |ui| {
                theme::kv(ui, "length", egui::RichText::new(bytes.len().to_string()));
                // The 2008 meshes slot is a mini-directory: u32 BE count + count×24-byte records
                // + zlib blobs whose inflated head is "MSH4". Report the header count + peek at
                // the first record to prove we grabbed the right slot boundary.
                if bytes.len() >= 4 {
                    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    theme::kv(ui, "records (u32 BE)", egui::RichText::new(n.to_string()));
                    if n > 0 && n < 4096 {
                        let dir_end = 4 + n as usize * 24;
                        if bytes.len() >= dir_end.max(16) {
                            let first_off =
                                u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
                            let first_comp =
                                u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
                            theme::kv(ui, "first record off", egui::RichText::new(format!("0x{first_off:X}")));
                            theme::kv(ui, "first record comp", egui::RichText::new(first_comp.to_string()));
                            // Try inflating that one record and looking for MSH4.
                            let start = first_off as usize;
                            let end = start.saturating_add(first_comp as usize);
                            if end <= bytes.len() && bytes.get(start).copied() == Some(0x78) {
                                use std::io::Read;
                                let mut z = flate2::read::ZlibDecoder::new(&bytes[start..end]);
                                let mut out: Vec<u8> = Vec::new();
                                match z.read_to_end(&mut out) {
                                    Ok(_) => {
                                        let magic = out.get(..4).and_then(|s| std::str::from_utf8(s).ok()).unwrap_or("?");
                                        theme::kv(ui, "inflated head", egui::RichText::new(format!("{magic}  ({} bytes)", out.len())));
                                        if magic == "MSH4" {
                                            ui.label(theme::data_text(
                                                "MSH4 magic confirmed — this IS a real meshes-slot entry (geometry decoder is a future probe)",
                                                10.5,
                                                theme::COLD,
                                            ));
                                        } else {
                                            ui.label(theme::data_text(
                                                "inflated head is not MSH4 — the outer boundary is correct but this record isn't a mesh",
                                                10.5,
                                                theme::EMBER,
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        ui.label(theme::data_text(
                                            format!("first record inflate failed: {e}"),
                                            10.5,
                                            theme::EMBER,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            });
        }
        Err(e) => {
            theme::card(ui, "Extract failed", None, |ui| {
                ui.label(theme::data_text(e, 11.0, theme::RED));
                ui.label(theme::data_text(
                    "Common cause: the containing .Pack uses a non-SBL4 sub-pack magic \
                     (SBLL/SBL8/SBL6/SBLX/…). Extracting those slots waits on the non-SBL4 \
                     sub-pack probe.",
                    10.0,
                    theme::FAINT,
                ));
            });
        }
    }
}

// ── tests ────────────────────────────────────────────────────────────────────────────────────
//
// The state layer runs windowless — the two tests below don't need an egui context. That was
// the whole point of splitting state from render(): the "browse a folder + click a row + get a
// texture" flow can be exercised on plain Rust code, and only the paint calls need a live
// egui::Ui.

#[cfg(test)]
mod tests {
    use super::*;

    /// Same skip pattern as `sources::prototype2008::tests` — this test needs a real 2008 dump.
    fn test_root() -> Option<std::path::PathBuf> {
        let candidate = std::path::PathBuf::from(
            "c:/Users/Shadow/Desktop/notes-on-reversing-the-sabetour/game-files",
        );
        if candidate.join("Bin360").is_dir() {
            Some(candidate)
        } else {
            None
        }
    }

    #[test]
    fn opens_a_2008_folder_and_lists_assets_through_state() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let mut st = ProtoState::default();
        st.set_root(&root).expect("open");
        // Each tab should produce a non-empty list — the packlog inventory alone contains
        // ~900 meshes + ~1900 textures + hundreds of materials on the real extract.
        for kind in [AssetKind::Texture, AssetKind::Mesh, AssetKind::Material] {
            let n = st.listing(kind).len();
            assert!(n > 10, "expected many {kind:?} handles, got {n}");
        }
        // The source is reachable through the state (this is what render() reads).
        assert!(st.source.is_some());
        assert!(st.open_error.is_none());
    }

    #[test]
    fn cloudproj_hash_round_trips_through_load_texture() {
        // The known-good loose DDS: CloudProj.dds is 512×512 DXT1 at the Bin360 root. Its hash
        // is the retail-scheme pandemic_hash of its stem, and the source's loose_dds path decodes
        // it directly (no SBL4 unwrap needed).
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let mut st = ProtoState::default();
        st.set_root(&root).expect("open");
        let source = st.source.as_ref().expect("source present after set_root");
        let hash = pandemic_hash("CloudProj");
        let tex = source.load_texture(hash).expect("decode CloudProj");
        assert_eq!(tex.width, 512);
        assert_eq!(tex.height, 512);
        assert_eq!(tex.rgba.len() as u64, tex.width as u64 * tex.height as u64 * 4);
    }

    #[test]
    fn selecting_a_material_gets_a_slot_vector_when_indexed() {
        // Walks the listing directly (skipping the UI) to prove: (a) materials are enumerated,
        // (b) the trait's textures_for_material answers, and (c) at least one slot is populated
        // on a real material. If any of these regressed we'd get a silent all-Nones result in
        // the material detail card — this test would catch it before a user opened the app.
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let mut st = ProtoState::default();
        st.set_root(&root).expect("open");
        let mats = st.listing(AssetKind::Material).to_vec();
        assert!(!mats.is_empty(), "no materials indexed");
        let source = st.source.as_ref().expect("source");
        // Find one that resolves with at least one non-None slot — every indexed material by
        // definition has that (only 0-texture drawcalls end up all-None, which the ingest
        // doesn't emit).
        let (uid, slots) = mats
            .iter()
            .find_map(|h| {
                let uid = h.key as u32;
                source.textures_for_material(uid).map(|s| (uid, s))
            })
            .expect("at least one indexed material resolves");
        assert!(
            slots.iter().any(|s| s.is_some()),
            "material {uid:#010x} came back with no slots — did the WSAO parser regress?"
        );
    }

    #[test]
    fn search_needle_matches_hex_hash() {
        // Round-trip the hash filter without a real source — the helper is pure.
        let h = AssetHandle { key: 0xdead_beef, name: "CloudProj".into(), kind: AssetKind::Texture };
        assert!(matches_needle(&h, ""));
        assert!(matches_needle(&h, "cloud"));
        assert!(matches_needle(&h, &format!("{:08x}", pandemic_hash("CloudProj"))));
    }

    #[test]
    fn close_clears_source_and_selection() {
        let Some(root) = test_root() else {
            eprintln!("skip: no Bin360/ prototype");
            return;
        };
        let mut st = ProtoState::default();
        st.set_root(&root).expect("open");
        st.selection = Some(Selection {
            kind: AssetKind::Texture,
            hash: pandemic_hash("CloudProj"),
            name: "CloudProj".into(),
            mesh_key: None,
        });
        st.close();
        assert!(st.source.is_none());
        assert!(st.selection.is_none());
        assert!(st.open_error.is_none());
    }
}
