//! The mod-editor pages: **Strings**, **Objects**, **Icons**.
//!
//! These are three views onto ONE document — the mod — not three file editors. That distinction is
//! the whole design:
//!
//! * **The mod is the document.** It has a name and one output slot (`DLC/NN/`). The three pages
//!   edit different parts of the same thing, and they cross-reference each other (an icon added on
//!   the Icons page is what an Objects property points at).
//! * **Sources are read-only until publish, and every write is reversible.** `DLC/01/` mirrors the
//!   whole tree (megapacks, Cinematics, GameTemplates), so a mod is naturally the next slot, and
//!   uninstalling it is deleting a folder.
//!
//!   One thing cannot go there. An edit to an EXISTING string has to patch the base
//!   `GameText.dlg`, because the engine's string map is first-write-wins and the base file is
//!   loaded first — a DLC slot can only ADD ids. Those patches keep retail beside them as
//!   `.sabbak`, which is both what publish reads back (so it never compounds) and what Unpublish
//!   restores.
//!
//!   It is NOT additive at runtime, which is the thing to know before publishing anything: the
//!   engine mounts exactly ONE DLC slot — the highest `dlclevel` that has a `dlcinfo.ini`, ties
//!   going to the lowest slot — and a slot with no manifest is not even a candidate. See
//!   [`mirror_dlc`] and [`winning_slot`] for what publish therefore has to write.
//! * **The changelist is the spine.** Every edit records what the value WAS, so revert is free and
//!   "what have I actually changed" is answerable at a glance. It spans all three pages because a
//!   real mod does.
//!
//! Two facts about the formats shape the UI more than any style choice:
//!
//! 1. **A UI string has no name on disk.** `GameText` stores `asset_id = pandemic_hash("File_Text.Key")`
//!    and an EMPTY key; the dotted id itself is never written. So strings cannot be grouped by
//!    namespace — you find them by searching their text, which is what a modder actually has. Where
//!    a template's `Name`/`Description` property points at a text hash, that template's name IS the
//!    string's name, and the cross-reference index recovers it.
//! 2. **A template property is four raw bytes.** The same word is a plausible int, float, or hash.
//!    Rather than make the modder pick a type before seeing anything, every reading is shown at once
//!    and the likeliest is lit.

use sab_formats::gametemplates::{Entry, GameTemplates};
use sab_formats::gametext::GameText;
use sab_formats::pandemic_hash;

use crate::dtex;
use crate::gui::theme;
use crate::pack::Megapack;

/// Which editor page is active.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdPage {
    Strings,
    Objects,
    Icons,
}

/// The language folders under `Cinematics/Dialog/`. A string edited in one is still retail in the
/// others — the likeliest way a mod ships half-broken, so coverage is always on screen.
const LANGS: [(&str, &str); 7] = [
    ("EN", "English"),
    ("FR", "French"),
    ("DE", "German"),
    ("IT", "Italian"),
    ("PL", "Polish"),
    ("RU", "Russian"),
    ("RND", "Random"),
];

// ---------------------------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------------------------

/// The mod being built: a name and one output overlay. Sources live in `game_dir` and are read-only.
struct Mod {
    name: String,
    game_dir: String,
    /// DLC slot to publish into. `01` is the game's own, so a mod takes the next free one.
    slot: String,
}

impl Mod {
    fn out_dir(&self) -> String {
        format!("{}/DLC/{}", self.game_dir, self.slot)
    }
    /// Where the game reads its base UI text from — and the only place an edit to an EXISTING string
    /// can go (see the destinations note on [`Editor::publish_inner`]).
    fn gametext_base(&self, lang: usize) -> String {
        format!("{}/Cinematics/Dialog/{}/GameText.dlg", self.game_dir, LANGS[lang].1)
    }
    /// Retail, kept beside the patched file. Publishing restores nothing and reverts nothing on its
    /// own; this is what makes both possible.
    fn gametext_bak(&self, lang: usize) -> String {
        format!("{}.sabbak", self.gametext_base(lang))
    }
    /// What to read: retail if we have already patched over it. Keeping the pristine copy as the
    /// source is what keeps publish a pure function of (sources, changelist) — re-publishing applies
    /// the changelist to retail again rather than stacking edits on yesterday's output.
    fn gametext_src(&self, lang: usize) -> String {
        let bak = self.gametext_bak(lang);
        if std::path::Path::new(&bak).exists() { bak } else { self.gametext_base(lang) }
    }
    fn gametext_out(&self, lang: usize) -> String {
        format!("{}/Cinematics/Dialog/{}/GameText.dlg", self.out_dir(), LANGS[lang].1)
    }
    /// The retail DLC — source for templates, and the payload the mod's slot has to carry once it
    /// wins the mount (see [`mirror_dlc`]).
    fn stock_dlc(&self) -> String {
        format!("{}/DLC/01", self.game_dir)
    }
    /// Templates ship inside the DLC overlay, so the game's own `DLC/01` copy is the live source.
    fn templates_src(&self) -> String {
        format!("{}/GameTemplates.wsd", self.stock_dlc())
    }
    fn templates_out(&self) -> String {
        format!("{}/GameTemplates.wsd", self.out_dir())
    }
    fn palettes(&self) -> String {
        format!("{}/Global/Palettes0.megapack", self.game_dir)
    }
}

// ---------------------------------------------------------------------------------------------
// The changelist
// ---------------------------------------------------------------------------------------------

/// What a change does when published. Each carries enough to re-apply it to a freshly parsed source.
///
/// "Re-apply to a freshly parsed source" is not only publish's contract — it is also how the window
/// shows your work. Sources are re-read on every load (a language switch, or a new session), and the
/// edit lives here rather than in the loaded model, so anything that lands has to be re-applied.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum Op {
    /// Overwrite an existing string's text. `scene` is `None` for a base record and `Some(sceneHash)`
    /// for a DNEC cinematic subtitle (a DNEC asset_id can collide with a base one, so it disambiguates
    /// the lookup). Old documents predate `scene` and deserialize it as `None` — i.e. a base record —
    /// which is what they always were, so no migration is needed.
    SetString {
        lang: usize,
        asset_id: u32,
        #[serde(default)]
        scene: Option<u32>,
        text: String,
    },
    AddString { lang: usize, dotted: String, text: String },
    SetPair { entry: usize, pair: usize, bytes: Vec<u8> },
    /// Reserve a texture NAME so template properties can point at its hash before the DTEX exists.
    ReserveTexture { name: String },
}

impl Op {
    fn page(&self) -> EdPage {
        match self {
            Op::SetString { .. } | Op::AddString { .. } => EdPage::Strings,
            Op::SetPair { .. } => EdPage::Objects,
            Op::ReserveTexture { .. } => EdPage::Icons,
        }
    }
}

/// One reversible edit. `before` is kept so the changelist can always say what it was.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Change {
    target: String,
    before: String,
    after: String,
    op: Op,
}

/// The mod document — the changelist, on disk.
///
/// The changelist IS the mod. Sources are read-only and the published overlay is derived from it, so
/// a changelist that lives only in the window means closing the window is losing the work, and a new
/// session opens on retail text with no sign that anything was ever modded. It is authoring state
/// rather than game content, so it is stored beside the app's settings — where it also survives
/// deleting the published `DLC/NN/` folder.
///
/// Keyed by output slot, not by mod name: the slot is what identifies this document to the app (it
/// is the mod's output), and keying by name would silently start an empty document the moment anyone
/// renamed their mod. `name` rides along so the file says what it belongs to.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ModDoc {
    /// Bumped when a stored op stops meaning what it meant. See [`ModDoc::CURRENT`].
    #[serde(default)]
    version: u32,
    #[serde(default)]
    name: String,
    #[serde(default)]
    changes: Vec<Change>,
}

impl ModDoc {
    /// `%APPDATA%/sab_workshop/mods/dlc-<slot>.json`, beside `settings.json`.
    fn path(slot: &str) -> std::path::PathBuf {
        let slug: String =
            slot.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect::<String>();
        crate::settings::settings_path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("mods")
            .join(format!("dlc-{}.json", if slug.is_empty() { "unset".into() } else { slug }))
    }

    /// 1 — the original. `SetPair.entry` indexed `DLC/01/GameTemplates.wsd` (5 entries).
    /// 2 — the Objects page reads the real DB from `France/loosefiles_BinPC.pack` (10,761
    ///     templates), so the same index now means a different template. A v1 document's object
    ///     edits cannot be carried forward and are dropped on load rather than silently re-pointed
    ///     at whatever now sits at that index.
    const CURRENT: u32 = 2;

    fn load(slot: &str) -> ModDoc {
        let mut doc = ModDoc::load_from(&ModDoc::path(slot));
        if doc.version < ModDoc::CURRENT {
            doc.changes.retain(|c| !matches!(c.op, Op::SetPair { .. }));
            doc.version = ModDoc::CURRENT;
        }
        doc
    }

    /// A missing or unreadable document is an empty one — first run, not an error. A CORRUPT one is
    /// also empty, which is the honest outcome: the alternative is refusing to open the app at all.
    fn load_from(path: &std::path::Path) -> ModDoc {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(slot: &str, name: &str, changes: &[Change]) -> Result<(), String> {
        ModDoc::save_to(&ModDoc::path(slot), name, changes)
    }

    fn save_to(path: &std::path::Path, name: &str, changes: &[Change]) -> Result<(), String> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
        }
        let doc =
            ModDoc { version: ModDoc::CURRENT, name: name.to_string(), changes: changes.to_vec() };
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

// ---------------------------------------------------------------------------------------------
// Per-page state
// ---------------------------------------------------------------------------------------------

/// An asset that loads ITSELF, off the UI thread.
///
/// There are no Load buttons in this editor. A page knowing which file it needs and then demanding
/// a click before fetching it is busywork — and doing that fetch on the UI thread is worse: the
/// megapack sweep is a ~700 MB read that froze the window outright ("Not Responding"). So every
/// page kicks its own load in the background and renders whatever state it is in.
enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
    fn ready_mut(&mut self) -> Option<&mut T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
}

/// One texture record found in the pack: name, its hash, and where to slice the bytes from.
type IconRec = (String, u32, usize, usize, usize);

/// Results streaming back from the loader threads.
enum EdMsg {
    Strings(usize, Result<GameText, String>),
    /// The table AND its value-hash cross-reference. The xref is built on the worker: it walks every
    /// pair of every template (~800k over the real DB), which is a visible hitch on the UI thread.
    Objects(Result<(GameTemplates, std::collections::HashMap<u32, Vec<usize>>), String>),
    IconProgress(usize, usize),
    Icons(Result<Vec<IconRec>, String>),
    /// hash, width, height, RGBA — decoded off-thread; the UI only uploads it.
    Thumb(u32, usize, usize, Vec<u8>),
}

/// Identity of a string across the base records AND the DNEC subtitle sub-tables. `scene == None` is a
/// base record (keyed by asset_id); `Some(hash)` is a record inside that DNEC scene group. Used for
/// selection and for keying staged edits, because a DNEC asset_id can collide with a base one.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct StrSel {
    asset_id: u32,
    scene: Option<u32>,
}

/// A row in the strings list, resolvable to a record in O(1). `Base` indexes `gt.records`; `Dnec`
/// indexes `gt.dnec_groups()[g].records[r]`.
#[derive(Clone, Copy)]
enum RowRef {
    Base(usize),
    Dnec(usize, usize),
}

/// A rendered row in the grouped Strings nav: a collapsible section header (index into the group
/// list) or one record under an expanded section.
#[derive(Clone, Copy)]
enum NavRow {
    Section(usize),
    Entry(RowRef),
}

#[derive(Default)]
struct StringsState {
    lang: usize,
    search: String,
    filter_ui: bool,
    filter_vo: bool,
    /// Show the DNEC cinematic subtitles (the per-scene VO overlay records).
    filter_dnec: bool,
    filter_edited: bool,
    selected: Option<StrSel>,
    edit_buf: String,
    new_id: String,
    new_text: String,
    /// Which speaker/UI sections are expanded in the grouped nav.
    expanded: std::collections::HashSet<String>,
    /// Cached grouping: `(section name, its rows)` sorted by size. Rebuilt only when the inputs
    /// below change — grouping 12k records (a `speaker()` parse each) every frame would be wasteful.
    groups: Vec<(String, Vec<RowRef>)>,
    /// The `(lang, search, filters, edit-count)` the cache was built for; `None` forces a rebuild.
    groups_key: Option<String>,
}

/// One line of the template list: a type heading, a template, or the "narrowed" footer.
///
/// Flat, and every variant one row tall, because that is what `show_rows` needs to virtualise —
/// grouping is expressed by position rather than by nesting.
enum ObjRow {
    Head(String, usize),
    Item(usize, String),
    More(usize),
}

#[derive(Default)]
struct ObjectsState {
    search: String,
    /// The list, grouped by type and flattened. Rebuilt only when the search or the loaded table
    /// changes: rebuilding it per frame meant lowercasing and cloning 10,761 names EVERY frame.
    rows: Vec<ObjRow>,
    /// The search this cache was built for. `None` forces a rebuild (a fresh table landed).
    rows_key: Option<String>,
    /// Template count, cached with the rows — `templates().count()` walks all 11,072 entries.
    total: usize,
    selected: Option<usize>,
    /// pair index -> edit buffer, for the pair currently being typed into
    edit_pair: Option<usize>,
    edit_buf: String,
    /// set when a texture-valued property asked the Icons page for a value
    wiring: Option<(usize, usize)>,
}

#[derive(Default)]
struct IconsState {
    search: String,
    selected: Option<usize>,
    only_used: bool,
    new_name: String,
    /// entries swept / total, while the pack is being walked
    progress: (usize, usize),
}

/// Thumbnails, decoded on a worker and kept.
///
/// Decoding is NOT cheap even for a small picture: a DTEX stores its mips as zlib streams, so
/// reaching the 128px mip inflates the whole ~700 KB chain first — about 11 ms per texture. Doing
/// that for a screenful on the UI thread is a visible stall, and for the whole pool it is the freeze
/// this page used to have. So the UI never decodes: it asks, and uploads what comes back.
#[derive(Default)]
struct Thumbs {
    cache: std::collections::HashMap<u32, Option<egui::TextureHandle>>,
    /// asked for, still in flight — so a tile on screen for many frames is requested once
    pending: std::collections::HashSet<u32>,
    req: Option<std::sync::mpsc::Sender<ThumbReq>>,
}

/// (hash, megapack entry, offset, len) — where to find the record's bytes.
type ThumbReq = (u32, usize, usize, usize);

// ---------------------------------------------------------------------------------------------

pub struct Editor {
    md: Mod,
    changes: Vec<Change>,
    strings: StringsState,
    objects: ObjectsState,
    icons: IconsState,
    thumbs: Thumbs,
    /// value-hash -> template entry indices that reference it. Built from the parsed templates in one
    /// pass; serves BOTH pages — "which templates use this texture" and "which template names this
    /// string" are the same question asked of different hashes.
    xref: std::collections::HashMap<u32, Vec<usize>>,
    publish_note: String,
    /// Whether that note is a success. Kept explicitly rather than sniffed from the note's text: the
    /// note now also carries the "which slot will the engine actually mount" verdict, and a publish
    /// that wrote every byte correctly into a slot that loses the mount is not a success.
    publish_ok: bool,

    // --- self-loading assets ---
    str_load: Load<GameText>,
    obj_load: Load<GameTemplates>,
    icon_load: Load<Vec<IconRec>>,
    tx: std::sync::mpsc::Sender<EdMsg>,
    rx: std::sync::mpsc::Receiver<EdMsg>,

    /// The Wwise sound pack for one language, cached (opened lazily on first Play). VO/subtitle
    /// audio resolution; see `sab_formats::wwise`.
    sound: Option<(usize, sab_formats::wwise::SoundPack)>,
    /// Status/error shown next to the Play button.
    audio_note: String,
    /// Status/error for file exports (textures, meshes).
    io_note: String,
}

impl Editor {
    /// `lang`, `slot` and `mod_name` come from the user's persisted settings — the Strings page opens
    /// on the configured language, and publish targets the chosen DLC slot. (The vgmstream path for
    /// voice playback is read live from settings at Play time, not cached here.)
    pub fn new(game_dir: &str, lang: usize, slot: &str, mod_name: &str) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut e = Editor {
            md: Mod {
                name: mod_name.to_string(),
                game_dir: game_dir.trim_end_matches(['/', '\\']).to_string(),
                slot: slot.to_string(),
            },
            changes: Vec::new(),
            strings: StringsState {
                filter_ui: true,
                filter_vo: true,
                lang: lang.min(LANGS.len() - 1),
                ..Default::default()
            },
            objects: ObjectsState::default(),
            icons: IconsState::default(),
            thumbs: Thumbs::default(),
            xref: Default::default(),
            publish_note: String::new(),
            publish_ok: false,
            str_load: Load::Idle,
            obj_load: Load::Idle,
            icon_load: Load::Idle,
            tx,
            rx,
            sound: None,
            audio_note: String::new(),
            io_note: String::new(),
        };
        // Whatever was staged last session. Loaded before the assets are even asked for, so the
        // first frame that has text in it already has the edits applied (see `pump`).
        e.changes = ModDoc::load(slot).changes;
        // Start everything now. Templates and one language are small and land almost immediately;
        // the pack sweep is the slow one and it runs alongside, so by the time anyone opens Icons it
        // is usually done — and if not, they see progress rather than a button.
        e.kick_strings();
        e.kick_objects();
        e.kick_icons();
        e
    }

    fn kick_strings(&mut self) {
        let (tx, lang, path) = (self.tx.clone(), self.strings.lang, self.md.gametext_src(self.strings.lang));
        self.str_load = Load::Loading;
        std::thread::spawn(move || {
            let r = std::fs::read(&path)
                .map_err(|e| format!("{path}: {e}"))
                .and_then(|b| GameText::parse(&b));
            let _ = tx.send(EdMsg::Strings(lang, r));
        });
    }

    /// Load the game's object DB — all of it.
    ///
    /// This used to read `DLC/01/GameTemplates.wsd` and show **five** templates, which is the whole
    /// of the Midnight Show DLC's patch table and none of the game. The real DB is an `AULB` blob
    /// embedded in `France/loosefiles_BinPC.pack` (10,761 templates), which `assets` already knows
    /// how to find — the page was simply still pointed at the file it was prototyped against.
    fn kick_objects(&mut self) {
        let (tx, game_dir) = (self.tx.clone(), self.md.game_dir.clone());
        self.obj_load = Load::Loading;
        std::thread::spawn(move || {
            let r = crate::assets::load_gametemplates(&game_dir)
                .ok_or_else(|| {
                    format!("no GameTemplates found in {game_dir}/France/loosefiles_BinPC.pack")
                })
                .map(|gt| {
                    let xref = build_xref(&gt);
                    (gt, xref)
                });
            let _ = tx.send(EdMsg::Objects(r));
        });
    }

    fn kick_icons(&mut self) {
        let (tx, path) = (self.tx.clone(), self.md.palettes());
        self.icon_load = Load::Loading;
        std::thread::spawn(move || {
            let r = (|| -> Result<Vec<IconRec>, String> {
                let mp = Megapack::open(&path)?;
                let total = mp.entries().len();
                let mut out = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for (ei, e) in mp.entries().iter().enumerate() {
                    for (off, len, name) in dtex::find_records(mp.slice(e)) {
                        let h = pandemic_hash(&name);
                        if seen.insert(h) {
                            out.push((name, h, ei, off, len));
                        }
                    }
                    if ei % 16 == 0 {
                        let _ = tx.send(EdMsg::IconProgress(ei, total));
                    }
                }
                out.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(out)
            })();
            let _ = tx.send(EdMsg::Icons(r));
        });
    }

    /// Drain the loader channel. Called once per frame before anything renders.
    pub fn pump(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                EdMsg::Strings(lang, r) => {
                    // A stale result from a language the user already switched away from.
                    if lang != self.strings.lang {
                        continue;
                    }
                    self.str_load = match r {
                        Ok(g) => Load::Ready(g),
                        Err(e) => Load::Failed(e),
                    };
                    // What just arrived is retail. Without this the staged edits are still in the
                    // changelist but invisible in the text — which is what a language switch, and
                    // every new session, used to look like.
                    self.reapply_strings(lang);
                }
                EdMsg::Objects(r) => {
                    self.obj_load = match r {
                        Ok((g, xref)) => {
                            self.xref = xref;
                            Load::Ready(g)
                        }
                        Err(e) => Load::Failed(e),
                    };
                    self.reapply_objects();
                    // A different table means the cached list describes nothing.
                    self.objects.rows_key = None;
                }
                EdMsg::IconProgress(d, t) => self.icons.progress = (d, t),
                EdMsg::Icons(r) => {
                    self.icon_load = match r {
                        Ok(v) => {
                            self.start_thumb_worker();
                            Load::Ready(v)
                        }
                        Err(e) => Load::Failed(e),
                    };
                }
                EdMsg::Thumb(hash, w, h, rgba) => {
                    self.thumbs.pending.remove(&hash);
                    let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                    let t = ctx.load_texture(
                        format!("ic{hash:08x}"),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    self.thumbs.cache.insert(hash, Some(t));
                }
            }
        }
    }

    /// A persistent decoder thread. It owns its own mmap of the pack and answers requests; the UI
    /// thread never touches zlib or BC again, it only uploads finished RGBA.
    fn start_thumb_worker(&mut self) {
        if self.thumbs.req.is_some() {
            return;
        }
        let (rq_tx, rq_rx) = std::sync::mpsc::channel::<ThumbReq>();
        let out = self.tx.clone();
        let path = self.md.palettes();
        std::thread::spawn(move || {
            let Ok(mp) = Megapack::open(&path) else { return };
            while let Ok((hash, ei, off, len)) = rq_rx.recv() {
                let Some(e) = mp.entries().get(ei) else { continue };
                let sub = mp.slice(e);
                if off + len > sub.len() {
                    continue;
                }
                if let Ok(t) = dtex::decode_preview(&sub[off..off + len], 96) {
                    let _ = out.send(EdMsg::Thumb(
                        hash,
                        t.width as usize,
                        t.height as usize,
                        t.rgba,
                    ));
                }
            }
        });
        self.thumbs.req = Some(rq_tx);
    }

    /// The scanned texture records, or an empty slice while the sweep is still running.
    fn icon_recs(&self) -> &[IconRec] {
        match &self.icon_load {
            Load::Ready(v) => v,
            _ => &[],
        }
    }

    /// Render a not-yet-ready asset. Never a button — the load is already happening.
    fn load_state(&self, ui: &mut egui::Ui, what: &str, l: &Load<impl Sized>, prog: Option<(usize, usize)>) -> bool {
        match l {
            Load::Ready(_) => false,
            Load::Idle | Load::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(theme::data_text(format!("loading {what}…"), 10.5, theme::DIM));
                });
                if let Some((d, t)) = prog {
                    if t > 0 {
                        let (r, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width().min(200.0), 3.0),
                            egui::Sense::hover(),
                        );
                        theme::meter_in(ui.painter(), r, d, t);
                        ui.label(theme::data_text(
                            format!("{d} of {t} bundles swept"),
                            9.5,
                            theme::FAINT,
                        ));
                    }
                }
                true
            }
            Load::Failed(e) => {
                ui.label(theme::data_text(format!("could not load {what}"), 11.0, theme::RED));
                ui.label(theme::data_text(e, 9.5, theme::FAINT));
                true
            }
        }
    }

    /// Pending edits on a page — the rail badge.
    pub fn pending(&self, page: EdPage) -> usize {
        self.changes.iter().filter(|c| c.op.page() == page).count()
    }

    pub fn total_pending(&self) -> usize {
        self.changes.len()
    }

    /// The command bar's mod chip: (name, output).
    pub fn mod_chip(&self) -> (String, String) {
        (self.md.name.clone(), format!("→ DLC/{}/", self.md.slot))
    }

    /// The status-bar line for a page.
    pub fn status(&self, page: EdPage) -> String {
        match page {
            EdPage::Strings => {
                let n = self.str_load.ready().map(|g| g.records.len()).unwrap_or(0);
                format!(
                    "{n} strings · {} · {} staged",
                    LANGS[self.strings.lang].0,
                    self.pending(EdPage::Strings)
                )
            }
            EdPage::Objects => {
                let n = self.obj_load.ready().map(|g| g.templates().count()).unwrap_or(0);
                format!("{n} templates · {} staged", self.pending(EdPage::Objects))
            }
            EdPage::Icons => format!(
                "{} records · {} referenced · {} staged",
                self.icon_recs().len(),
                self.icon_recs().iter().filter(|(_, h, _, _, _)| self.xref.contains_key(h)).count(),
                self.pending(EdPage::Icons)
            ),
        }
    }

    // ------------------------------------------------------------------ panels

    /// Left navigator for the active page.
    pub fn nav(&mut self, ui: &mut egui::Ui, page: EdPage) {
        match page {
            EdPage::Strings => self.strings_nav(ui),
            EdPage::Objects => self.objects_nav(ui),
            EdPage::Icons => self.icons_nav(ui),
        }
    }

    /// The work surface.
    pub fn central(&mut self, ctx: &egui::Context, page: EdPage) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::G0)
                    .inner_margin(egui::Margin::symmetric(16.0, 14.0)),
            )
            // No ScrollArea here: a page that needs one adds its own. Icons in particular must
            // virtualize (see `icons_central`), and a wrapping ScrollArea would defeat that by
            // making every tile lay out anyway.
            .show(ctx, |ui| match page {
                EdPage::Strings => {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.strings_central(ui));
                }
                EdPage::Objects => {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.objects_central(ui));
                }
                EdPage::Icons => self.icons_central(ui),
            });
    }

    /// Right rail: the page's context card, then the changelist — the spine, on every page.
    pub fn side(&mut self, ui: &mut egui::Ui, page: EdPage) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match page {
                EdPage::Strings => self.strings_context(ui),
                EdPage::Objects => self.objects_context(ui),
                EdPage::Icons => self.icons_context(ui),
            }
            ui.add_space(6.0);
            self.changelist(ui);
        });
    }

    // ------------------------------------------------------------------ changelist

    fn changelist(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "Changelist");
        ui.add_space(2.0);
        if self.changes.is_empty() {
            ui.label(theme::data_text(
                "Nothing staged yet. Every edit lands here with what it was, so it can always be undone.",
                10.5,
                theme::FAINT,
            ));
            return;
        }
        ui.label(theme::data_text(
            format!("{} change(s) across the mod", self.changes.len()),
            10.0,
            theme::FAINT,
        ));
        ui.add_space(4.0);

        let mut revert: Option<usize> = None;
        for (i, c) in self.changes.iter().enumerate() {
            let (tag, col) = match c.op.page() {
                EdPage::Strings => ("TEXT", theme::EMBER),
                EdPage::Objects => ("OBJ", theme::COLD),
                EdPage::Icons => ("ICON", theme::DIM),
            };
            egui::Frame::none()
                .fill(theme::G2)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                .outer_margin(egui::Margin { bottom: 5.0, ..Default::default() })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(theme::disp_text(tag, 9.5, col));
                        ui.label(theme::data_text(&c.target, 10.5, theme::TX));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(theme::disp_text(theme::sym::UNDO, 11.0, theme::FAINT))
                                        .frame(false),
                                )
                                .on_hover_text("revert this change")
                                .clicked()
                            {
                                revert = Some(i);
                            }
                        });
                    });
                    // before → after, in the two colours the theme already uses for
                    // "the game's value" and "yours".
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if !c.before.is_empty() {
                            ui.label(theme::data_text(trunc(&c.before, 26), 9.5, theme::COLD));
                            ui.label(theme::data_text("→", 9.5, theme::FAINT));
                        }
                        ui.label(theme::data_text(trunc(&c.after, 26), 9.5, theme::EMBER));
                    });
                });
        }
        if let Some(i) = revert {
            let c = self.changes.remove(i);
            self.unapply(&c);
            self.save_doc();
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(theme::disp_text("Revert all", 11.0, theme::TX))).clicked() {
                let all: Vec<Change> = self.changes.drain(..).collect();
                for c in all.iter().rev() {
                    self.unapply(c);
                }
                self.save_doc();
            }
            // Offered whenever anything is installed, not only when the changelist is dirty: the
            // thing worth undoing is what is on the game's disk, which outlives this session.
            let installed = (0..LANGS.len()).any(|l| std::path::Path::new(&self.md.gametext_bak(l)).exists())
                || std::path::Path::new(&format!("{}/dlcinfo.ini", self.md.out_dir())).exists();
            if installed
                && ui
                    .add(egui::Button::new(theme::disp_text("Unpublish", 11.0, theme::TX)))
                    .on_hover_text(
                        "Restore every patched retail GameText.dlg from its .sabbak and delete the \
                         mod's dlcinfo.ini, which un-mounts the slot. The mirrored retail payload \
                         stays; delete the DLC folder to reclaim the disk.",
                    )
                    .clicked()
            {
                self.unpublish();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::stamp_button(ui, "Publish", !self.changes.is_empty())
                    .on_hover_text(format!(
                        "Edits of retail strings patch {}/Cinematics/Dialog/<Lang>/GameText.dlg — the \
                         only place they can work — keeping retail as GameText.dlg.sabbak. Everything \
                         else goes to {}, with a dlcinfo.ini that outranks every other slot and a copy \
                         of DLC/01's payload (~174 MB, first publish only), because the engine mounts \
                         one slot and the loser does not load.",
                        self.md.game_dir,
                        self.md.out_dir()
                    ))
                    .clicked()
                {
                    self.publish();
                }
            });
        });
        if !self.publish_note.is_empty() {
            ui.add_space(4.0);
            let col = if self.publish_ok { theme::EMBER } else { theme::RED };
            ui.label(theme::data_text(&self.publish_note, 10.0, col));
        }
        ui.add_space(3.0);
        // Publish has TWO destinations and they are not interchangeable: an edit of a retail string
        // cannot work from the slot (the engine drops the duplicate hash), and an addition has no
        // business touching retail. Naming only the slot here read as "everything goes to DLC/NN",
        // which is how a base patch could look like a file written to the wrong place.
        let (mut edits, mut extras) = (false, false);
        for c in &self.changes {
            match &c.op {
                Op::SetString { .. } => edits = true,
                _ => extras = true,
            }
        }
        if self.changes.is_empty() {
            // Nothing staged yet — describe both routes rather than neither.
            edits = true;
            extras = true;
        }
        if edits {
            ui.label(theme::data_text(
                format!("edits of retail strings → {}/Cinematics/Dialog/<Lang>/GameText.dlg", self.md.game_dir),
                9.5,
                theme::FAINT,
            ));
            ui.label(theme::data_text(
                "  retail is kept beside it as GameText.dlg.sabbak — Unpublish puts it back",
                9.0,
                theme::FAINT,
            ));
        }
        if extras {
            ui.label(theme::data_text(
                format!("new strings, objects, icons → {}", self.md.out_dir()),
                9.5,
                theme::FAINT,
            ));
        }
    }

    /// Undo a change's effect on the in-memory model (the changelist entry itself is already gone).
    fn unapply(&mut self, c: &Change) {
        match &c.op {
            Op::SetString { asset_id, scene, .. } => {
                if let Some(gt) = self.str_load.ready_mut() {
                    if let Some(r) = gt.find_any_mut(*asset_id, *scene) {
                        r.set_text(&c.before);
                    }
                }
                self.strings.edit_buf = c.before.clone();
            }
            Op::AddString { dotted, .. } => {
                let id = pandemic_hash(dotted);
                if let Some(gt) = self.str_load.ready_mut() {
                    gt.records.retain(|r| r.asset_id != id);
                }
            }
            Op::SetPair { entry, pair, .. } => {
                // `before` is the display form; re-encode from the raw we stored alongside it.
                if let Some(gt) = self.obj_load.ready_mut() {
                    if let Some(Entry::Template(t)) = gt.entries.get_mut(*entry) {
                        if let Some(p) = t.pairs.get_mut(*pair) {
                            if let Ok(b) = parse_u32_any(&c.before) {
                                p.data = b.to_le_bytes().to_vec();
                            }
                        }
                    }
                }
            }
            Op::ReserveTexture { .. } => {}
        }
    }

    fn stage(&mut self, target: String, before: String, after: String, op: Op) {
        // One entry per target: re-editing the same thing updates it and keeps the ORIGINAL before,
        // so the changelist always compares against retail, not against your last keystroke.
        if let Some(existing) = self.changes.iter_mut().find(|c| c.target == target) {
            existing.after = after;
            existing.op = op;
        } else {
            self.changes.push(Change { target, before, after, op });
        }
        self.save_doc();
    }

    /// Persist the changelist. Called on every mutation rather than on exit: the window can be
    /// closed by anything, and "save your mod" is not a step a modder should have to remember.
    fn save_doc(&mut self) {
        if let Err(e) = ModDoc::save(&self.md.slot, &self.md.name, &self.changes) {
            self.publish_ok = false;
            self.publish_note = format!("WARNING: could not save the mod document: {e}");
        }
    }

    /// Put the staged string edits back onto a freshly loaded language.
    fn reapply_strings(&mut self, lang: usize) {
        let Some(gt) = self.str_load.ready_mut() else { return };
        for c in &self.changes {
            match &c.op {
                Op::SetString { lang: l, asset_id, scene, text } if *l == lang => {
                    if let Some(r) = gt.find_any_mut(*asset_id, *scene) {
                        r.set_text(text);
                    }
                }
                Op::AddString { lang: l, dotted, text } if *l == lang => {
                    let _ = gt.add_ui(dotted, text);
                }
                _ => {}
            }
        }
    }

    /// The same, for a freshly loaded template file.
    fn reapply_objects(&mut self) {
        let Some(gt) = self.obj_load.ready_mut() else { return };
        for c in &self.changes {
            if let Op::SetPair { entry, pair, bytes } = &c.op {
                if let Some(Entry::Template(t)) = gt.entries.get_mut(*entry) {
                    if let Some(p) = t.pairs.get_mut(*pair) {
                        p.data = bytes.clone();
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ publish

    /// Write the overlay, then check the engine would actually mount it.
    ///
    /// The write half is a pure function of (sources, changelist): every source is re-read fresh and
    /// every change re-applied, so publishing twice never compounds. The check half exists because
    /// this used to report success for an overlay the game could not see — a slot with no
    /// `dlcinfo.ini` is not a mount candidate at all, and only one slot mounts.
    fn publish(&mut self) {
        match self.publish_inner() {
            Ok(note) => {
                self.publish_ok = !note.contains("WARNING");
                self.publish_note = note;
            }
            Err(e) => {
                self.publish_ok = false;
                self.publish_note = e;
            }
        }
    }

    fn publish_inner(&self) -> Result<String, String> {
        let out = self.md.out_dir();
        // Display names for the note, and the slot-relative paths the mirror must not overwrite.
        let mut wrote: Vec<String> = Vec::new();
        let mut mine: Vec<String> = Vec::new();

        // ---- strings, per language: two destinations, because the engine forces it ----
        //
        // A DLC `GameText.dlg` can only ADD ids. Both text layers insert into the one map the lookup
        // searches, `FUN_009603f0` @0x009603f0 refuses a hash the map already holds, and the base file
        // is loaded during init while the DLC mount runs later off the per-frame state machine. So an
        // edit of a retail string published into the slot is loaded, rejected, and silently ignored —
        // which is exactly how a byte-perfect overlay changed nothing in game.
        //
        // Edits therefore patch the base file, with retail preserved beside it as `.sabbak` (that copy
        // is also what publish reads back, so this never compounds, and `unpublish` can undo it).
        // Additions keep going to the mod's slot, where they work and where uninstalling is deleting a
        // folder.
        for lang in 0..LANGS.len() {
            let mut sets: Vec<(u32, Option<u32>, &str)> = Vec::new();
            let mut adds: Vec<(&str, &str)> = Vec::new();
            for c in &self.changes {
                match &c.op {
                    Op::SetString { lang: l, asset_id, scene, text } if *l == lang => {
                        sets.push((*asset_id, *scene, text))
                    }
                    Op::AddString { lang: l, dotted, text } if *l == lang => {
                        adds.push((dotted, text))
                    }
                    _ => {}
                }
            }
            if sets.is_empty() && adds.is_empty() {
                continue;
            }
            let src = self.md.gametext_src(lang);
            let bytes = std::fs::read(&src).map_err(|_| format!("cannot read {src}"))?;

            let rel = format!("Cinematics/Dialog/{}/GameText.dlg", LANGS[lang].1);
            if !adds.is_empty() {
                let mut gt = GameText::parse(&bytes).map_err(|_| format!("cannot parse {src}"))?;
                for (dotted, text) in &adds {
                    let _ = gt.add_ui(dotted, text);
                }
                write_file(&self.md.gametext_out(lang), &gt.write())?;
                wrote.push(format!("{} +{} new string(s)", LANGS[lang].1, adds.len()));
                mine.push(rel);
            } else {
                // An earlier publish may have put a file here — including one from before edits were
                // known to belong in the base file. Left behind it is inert (the engine rejects its
                // duplicate hashes) and misleading: it reads as a live override that does nothing.
                // Only removed when retail could not have supplied it, so the mirror is never eaten.
                let out_file = self.md.gametext_out(lang);
                if std::path::Path::new(&out_file).exists()
                    && !std::path::Path::new(&format!("{}/{rel}", self.md.stock_dlc())).exists()
                {
                    std::fs::remove_file(&out_file).map_err(|e| format!("remove {out_file}: {e}"))?;
                    wrote.push(format!("{} slot copy removed", LANGS[lang].1));
                }
            }

            if !sets.is_empty() {
                let mut gt = GameText::parse(&bytes).map_err(|_| format!("cannot parse {src}"))?;
                for (id, scene, text) in &sets {
                    if let Some(r) = gt.find_any_mut(*id, *scene) {
                        r.set_text(text);
                    }
                }
                // Back up BEFORE the first patch, never after: the copy has to be retail.
                let (base, bak) = (self.md.gametext_base(lang), self.md.gametext_bak(lang));
                if !std::path::Path::new(&bak).exists() {
                    std::fs::copy(&base, &bak).map_err(|e| format!("back up {base}: {e}"))?;
                }
                write_file(&base, &gt.write())?;
                wrote.push(format!("{} base ×{} (retail → .sabbak)", LANGS[lang].1, sets.len()));
            }
        }

        // ---- templates ----
        let pair_ops: Vec<(usize, usize, Vec<u8>)> = self
            .changes
            .iter()
            .filter_map(|c| match &c.op {
                Op::SetPair { entry, pair, bytes } => Some((*entry, *pair, bytes.clone())),
                _ => None,
            })
            .collect();
        // ---- templates: readable, not yet publishable ----
        //
        // The Objects page now reads the real DB out of `France/loosefiles_BinPC.pack`, so a
        // `SetPair` indexes THAT table — while this used to write `DLC/01`'s five-entry file, where
        // the same index means an unrelated template. Publishing across that mismatch would edit
        // something nobody asked for, silently, which is the exact failure this tool keeps hitting.
        //
        // Refusing is the honest state until the write path is built. Two things have to be settled
        // first: publish the touched entries as their own small `AULB` (retail's DLC ships 5 that
        // way) rather than an 8 MB copy of the whole table, and establish whether a slot can
        // override a BASE template at all — `FUN_00461590` updates a template in place when the
        // name already exists, which makes it last-writer-wins, and retail's own DLC only ever ADDS
        // names. If the base table loads after the mount, an override would be undone.
        // Skipped rather than fatal: the rest of the mod still ships, and the object edits stay in
        // the changelist for the session that builds this.
        let deferred = if pair_ops.is_empty() {
            String::new()
        } else {
            format!(
                "\nWARNING: {} object edit(s) staged but NOT published — object publishing is not \
                 wired up yet; they stay in the changelist",
                pair_ops.len()
            )
        };

        // ---- reserved texture names ----
        // A reservation is a NAME, not pixels: the hash is what a template points at, and packing the
        // DTEX itself belongs to sab_dtex / sab_sbla. Record it so the chain is documented rather
        // than silently missing.
        let reserved: Vec<&str> = self
            .changes
            .iter()
            .filter_map(|c| match &c.op {
                Op::ReserveTexture { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        if !reserved.is_empty() {
            let mut s = String::from("# textures this mod expects, and the hash a template uses\n");
            for n in &reserved {
                s.push_str(&format!("{n}\t0x{:08X}\n", pandemic_hash(n)));
            }
            write_file(&format!("{out}/textures.wanted.tsv"), s.as_bytes())?;
            wrote.push("textures.wanted.tsv".into());
            mine.push("textures.wanted.tsv".into());
        }

        if wrote.is_empty() {
            return Ok(if deferred.is_empty() {
                "nothing to write".into()
            } else {
                deferred.trim_start().to_string()
            });
        }

        // Nothing went into the slot — a mod of nothing but string edits is now exactly two patched
        // base files, and claiming the mount for it would cost 174 MB and unmount DLC/01 for no gain.
        // An old manifest must go with it: left in place it would keep the mount pointed at a slot
        // with no mod content, so DLC/01 stays unmounted for nothing.
        if mine.is_empty() {
            let ini = format!("{out}/dlcinfo.ini");
            if std::path::Path::new(&ini).exists() {
                std::fs::remove_file(&ini).map_err(|e| format!("remove {ini}: {e}"))?;
                wrote.push(format!("DLC/{} un-mounted (nothing left in it)", self.md.slot));
            }
            return Ok(format!("published ({}){}", wrote.join(", "), deferred));
        }

        // ---- the retail payload this slot has to carry ----
        // Winner takes all: once the mod's slot mounts, DLC/01 does not, and everything it provided
        // stops loading. Skipped when publishing into 01 itself, which would be a self-copy.
        if self.md.slot != "01" {
            let (n, bytes) = mirror_dlc(&self.md.stock_dlc(), &out, &mine)?;
            if n > 0 {
                wrote.push(format!("+{n} retail file(s), {:.0} MB", bytes as f64 / 1_048_576.0));
            }
        }

        // ---- dlcinfo.ini — the manifest that makes the slot exist at all ----
        // One more than the best of the OTHER slots, because ties keep the lowest slot: matching
        // retail's `dlclevel=1` from slot 02 loses to DLC/01 and mounts nothing.
        let rival = DLC_SLOTS
            .iter()
            .filter(|s| **s != self.md.slot)
            .filter_map(|s| read_dlclevel(&format!("{}/DLC/{s}", self.md.game_dir)))
            .max()
            .unwrap_or(0);
        write_file(&format!("{out}/dlcinfo.ini"), dlcinfo(&self.md.name, rival + 1).as_bytes())?;
        wrote.push("dlcinfo.ini".into());

        // ---- and would the engine actually take it? ----
        let verdict = if !DLC_SLOTS.contains(&self.md.slot.as_str()) {
            format!(
                "WARNING: the engine only probes DLC/{}..{} — nothing in DLC/{} is ever read",
                DLC_SLOTS[0],
                DLC_SLOTS[DLC_SLOTS.len() - 1],
                self.md.slot
            )
        } else {
            match winning_slot(&self.md.game_dir) {
                Some((s, l)) if s == self.md.slot => {
                    format!("engine mounts DLC/{s} (dlclevel {l}) — this mod")
                }
                Some((s, l)) => format!("WARNING: engine mounts DLC/{s} (dlclevel {l}), not this mod"),
                None => "WARNING: no slot has a dlcinfo.ini — no DLC will mount".into(),
            }
        };

        Ok(format!("published → {} ({})\n{}{}", out, wrote.join(", "), verdict, deferred))
    }

    /// Put the game back the way it was found.
    ///
    /// Two distinct undos, because publish has two destinations: every patched base `GameText.dlg` is
    /// restored from its `.sabbak` (and the backup removed, so the next publish starts from retail
    /// again), and the mod's `dlcinfo.ini` is deleted — which is all it takes to un-mount the slot,
    /// since a slot with no manifest is not a candidate and `DLC/01` wins again.
    ///
    /// The mirrored retail payload is left alone: it is 174 MB of copies, harmless once nothing points
    /// at it, and deleting `DLC/<slot>/` reclaims it whenever the modder wants.
    fn unpublish(&mut self) {
        let mut done: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();

        for lang in 0..LANGS.len() {
            let bak = self.md.gametext_bak(lang);
            if !std::path::Path::new(&bak).exists() {
                continue;
            }
            let base = self.md.gametext_base(lang);
            match std::fs::copy(&bak, &base).and_then(|_| std::fs::remove_file(&bak)) {
                Ok(_) => done.push(format!("{} restored", LANGS[lang].1)),
                Err(e) => failed.push(format!("{}: {e}", LANGS[lang].1)),
            }
        }

        let ini = format!("{}/dlcinfo.ini", self.md.out_dir());
        if std::path::Path::new(&ini).exists() {
            match std::fs::remove_file(&ini) {
                Ok(_) => done.push(format!("DLC/{} un-mounted", self.md.slot)),
                Err(e) => failed.push(format!("dlcinfo.ini: {e}")),
            }
        }

        self.publish_ok = failed.is_empty();
        self.publish_note = if !failed.is_empty() {
            format!("WARNING: {}", failed.join("; "))
        } else if done.is_empty() {
            "nothing published to undo".into()
        } else {
            let now = match winning_slot(&self.md.game_dir) {
                Some((s, l)) => format!("engine mounts DLC/{s} (dlclevel {l})"),
                None => "no DLC will mount".into(),
            };
            format!("unpublished ({})\n{}", done.join(", "), now)
        };
    }

    // ============================================================== STRINGS

    fn strings_nav(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "Language");
        ui.add_space(3.0);
        // Coverage: which languages this mod has touched. A string changed in EN is still retail in
        // the other six, and that was previously invisible.
        // Painted rather than laid out: seven fixed cells that always read as one strip, whatever
        // width the panel is dragged to. (A Frame per language stretches to fill the row.)
        let mut switch_to: Option<usize> = None;
        let cell_w = 40.0;
        let cell_h = 36.0;
        let (strip, _) = ui.allocate_exact_size(
            egui::vec2(cell_w * LANGS.len() as f32 + 6.0 * (LANGS.len() - 1) as f32, cell_h),
            egui::Sense::hover(),
        );
        for (i, (short, full)) in LANGS.iter().enumerate() {
            let edits = self
                .changes
                .iter()
                .filter(|c| {
                    matches!(&c.op, Op::SetString { lang, .. } | Op::AddString { lang, .. } if *lang == i)
                })
                .count();
            let r = egui::Rect::from_min_size(
                strip.min + egui::vec2((cell_w + 6.0) * i as f32, 0.0),
                egui::vec2(cell_w, cell_h),
            );
            let resp = ui.interact(r, ui.id().with(("lang", i)), egui::Sense::click());
            let on = self.strings.lang == i;
            let (fill, stroke, fg) = if on {
                (theme::RED_SOFT, theme::RED, theme::RED)
            } else if edits > 0 {
                (theme::EMBER_SOFT, theme::EMBER_DK, theme::EMBER)
            } else {
                (theme::G2, theme::LINE, theme::FAINT)
            };
            let p = ui.painter();
            p.rect_filled(r, egui::Rounding::ZERO, fill);
            p.rect_stroke(r, egui::Rounding::ZERO, egui::Stroke::new(1.0, stroke));
            p.text(
                r.center() - egui::vec2(0.0, 6.0),
                egui::Align2::CENTER_CENTER,
                short,
                egui::FontId::new(10.0, theme::disp()),
                fg,
            );
            // a filled bar = this language carries edits; empty = still retail
            let bar = egui::Rect::from_min_size(
                egui::pos2(r.left() + 6.0, r.bottom() - 9.0),
                egui::vec2(cell_w - 12.0, 3.0),
            );
            p.rect_filled(bar, egui::Rounding::ZERO, theme::G3);
            if edits > 0 {
                p.rect_filled(bar, egui::Rounding::ZERO, theme::EMBER);
            }
            if resp.clicked() {
                switch_to = Some(i);
            }
            resp.on_hover_text(format!("{full} — {edits} edit(s) in this mod"));
        }
        if let Some(i) = switch_to {
            if self.strings.lang != i {
                self.strings.lang = i;
                self.strings.selected = None;
                self.kick_strings(); // switching language just loads it
            }
        }

        ui.add_space(8.0);
        theme::eyebrow(ui, "Strings");
        ui.add_space(3.0);

        if self.load_state(ui, LANGS[self.strings.lang].1, &self.str_load, None) {
            ui.add_space(4.0);
            ui.label(theme::data_text(
                "nothing is written until you publish; retail is backed up before it is patched",
                9.5,
                theme::FAINT,
            ));
            return;
        }

        // Own the model for the rest of the frame so it does not alias the `&mut self` calls below
        // (staging, selection). Restored before returning — the guard above already proved it Ready.
        let loaded = std::mem::replace(&mut self.str_load, Load::Loading);
        let Load::Ready(gt) = &loaded else { self.str_load = loaded; return; };

        // filters
        ui.horizontal(|ui| {
            if theme::pill(ui, "ui", self.strings.filter_ui).clicked() {
                self.strings.filter_ui = !self.strings.filter_ui;
            }
            if theme::pill(ui, "vo", self.strings.filter_vo).clicked() {
                self.strings.filter_vo = !self.strings.filter_vo;
            }
            // The DNEC per-scene cinematic subtitles — off by default so the page opens on the base
            // records, on to surface the ~1300 overlay VO lines.
            if theme::pill(ui, "subs", self.strings.filter_dnec).clicked() {
                self.strings.filter_dnec = !self.strings.filter_dnec;
            }
            if theme::pill(ui, "edited", self.strings.filter_edited).clicked() {
                self.strings.filter_edited = !self.strings.filter_edited;
            }
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(theme::sym::NO).clicked() {
                    self.strings.search.clear();
                }
                ui.add_sized(
                    [ui.available_width(), 20.0],
                    egui::TextEdit::singleline(&mut self.strings.search)
                        .hint_text("search the text itself"),
                );
            });
        });

        
        let needle = self.strings.search.to_ascii_lowercase();
        let edited: std::collections::HashSet<StrSel> = self
            .changes
            .iter()
            .filter_map(|c| match &c.op {
                Op::SetString { asset_id, scene, lang, .. } if *lang == self.strings.lang => {
                    Some(StrSel { asset_id: *asset_id, scene: *scene })
                }
                _ => None,
            })
            .collect();

        // ---- group the records into collapsible sections: one per speaker (VO/subs), plus
        // "UI Text". Rebuilt only when its inputs change — grouping is O(records) with a speaker()
        // key parse each, so expand/collapse and selection must NOT invalidate it. ----
        let (fui, fvo, fdnec, fed) = (
            self.strings.filter_ui,
            self.strings.filter_vo,
            self.strings.filter_dnec,
            self.strings.filter_edited,
        );
        let cache_key = format!(
            "{}|{needle}|{}{}{}{}|{}",
            self.strings.lang, fui as u8, fvo as u8, fdnec as u8, fed as u8, self.changes.len()
        );
        if self.strings.groups_key.as_deref() != Some(cache_key.as_str()) {
            let keep = |r: &sab_formats::gametext::Record, sel: StrSel| -> bool {
                let is_ui = r.is_ui();
                if (is_ui && !fui) || (!is_ui && !fvo) {
                    return false;
                }
                if fed && !edited.contains(&sel) {
                    return false;
                }
                needle.is_empty()
                    || r.text_string().to_ascii_lowercase().contains(&needle)
                    || format!("{:08x}", r.asset_id).contains(&needle)
                    || r.key_str().to_ascii_lowercase().contains(&needle)
            };
            let section = |r: &sab_formats::gametext::Record| -> String {
                if r.is_ui() {
                    "UI Text".to_string()
                } else {
                    r.speaker().unwrap_or_else(|| "Other VO".into())
                }
            };
            let mut map: std::collections::HashMap<String, Vec<RowRef>> = Default::default();
            for (i, r) in gt.records.iter().enumerate() {
                if keep(r, StrSel { asset_id: r.asset_id, scene: None }) {
                    map.entry(section(r)).or_default().push(RowRef::Base(i));
                }
            }
            if fdnec {
                for (g, grp) in gt.dnec_groups().iter().enumerate() {
                    for (j, r) in grp.records.iter().enumerate() {
                        if keep(r, StrSel { asset_id: r.asset_id, scene: Some(grp.scene_hash) }) {
                            map.entry(section(r)).or_default().push(RowRef::Dnec(g, j));
                        }
                    }
                }
            }
            let mut groups: Vec<(String, Vec<RowRef>)> = map.into_iter().collect();
            // Biggest sections first (main cast + the UI surface); ties broken by name.
            groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
            self.strings.groups = groups;
            self.strings.groups_key = Some(cache_key);
        }

        let searching = !needle.is_empty();
        let shown: usize = self.strings.groups.iter().map(|g| g.1.len()).sum();
        let total = gt.records.len() + gt.dnec_record_count();
        ui.label(theme::data_text(
            format!("{shown} in {} groups · {total} total", self.strings.groups.len()),
            10.0,
            theme::FAINT,
        ));
        ui.add_space(3.0);

        // Flatten to a virtualised row list: a header per section, its entries when expanded (or
        // always, while a search is active).
        let mut nav: Vec<NavRow> = Vec::new();
        for (gi, (name, rows)) in self.strings.groups.iter().enumerate() {
            nav.push(NavRow::Section(gi));
            if searching || self.strings.expanded.contains(name) {
                for &rr in rows {
                    nav.push(NavRow::Entry(rr));
                }
            }
        }

        let mut pick: Option<StrSel> = None;
        let mut toggle: Option<String> = None;
        egui::ScrollArea::vertical().id_source("str_rows").auto_shrink([false, false]).show_rows(
            ui,
            20.0,
            nav.len(),
            |ui, range| {
                for k in range {
                    match nav[k] {
                        NavRow::Section(gi) => {
                            let (name, rows) = &self.strings.groups[gi];
                            let open = searching || self.strings.expanded.contains(name);
                            // The WHOLE row is the click target (caret + name + empty space), with a
                            // pointing-hand cursor — not just the name label. Interact with an
                            // explicit unique id per section, else every row clashes on one id.
                            let inner = ui.horizontal(|ui| {
                                ui.set_min_width(ui.available_width());
                                // caret, painter-drawn so it never depends on a font glyph
                                let (tri, _) = ui.allocate_exact_size(egui::vec2(11.0, 12.0), egui::Sense::hover());
                                let c = tri.center();
                                let pts = if open {
                                    vec![egui::pos2(c.x - 3.5, c.y - 2.0), egui::pos2(c.x + 3.5, c.y - 2.0), egui::pos2(c.x, c.y + 3.0)]
                                } else {
                                    vec![egui::pos2(c.x - 2.0, c.y - 3.5), egui::pos2(c.x - 2.0, c.y + 3.5), egui::pos2(c.x + 3.0, c.y)]
                                };
                                ui.painter().add(egui::Shape::convex_polygon(pts, theme::DIM, egui::Stroke::NONE));
                                ui.label(theme::disp_text(name.as_str(), 12.0, theme::TX));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(theme::data_text(rows.len().to_string(), 10.0, theme::FAINT));
                                });
                            });
                            let resp = ui
                                .interact(inner.response.rect, ui.id().with(("sec_row", name.as_str())), egui::Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                toggle = Some(name.clone());
                            }
                        }
                        NavRow::Entry(rr) => {
                            let (r, rsel, kind) = match rr {
                                RowRef::Base(i) => {
                                    let r = &gt.records[i];
                                    (r, StrSel { asset_id: r.asset_id, scene: None }, if r.is_ui() { "ui" } else { "vo" })
                                }
                                RowRef::Dnec(g, j) => {
                                    let grp = &gt.dnec_groups()[g];
                                    let r = &grp.records[j];
                                    (r, StrSel { asset_id: r.asset_id, scene: Some(grp.scene_hash) }, "sub")
                                }
                            };
                            let sel = self.strings.selected == Some(rsel);
                            let is_edited = edited.contains(&rsel);
                            let inner = ui.horizontal(|ui| {
                                ui.set_min_width(ui.available_width());
                                ui.add_space(12.0);
                                let (d, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                                ui.painter().rect_filled(d, egui::Rounding::ZERO, if is_edited { theme::EMBER } else { theme::G3 });
                                let fg = if sel { theme::RED } else if is_edited { theme::TX } else { theme::DIM };
                                // combat/ambient barks carry no subtitle text; fall back to the key
                                // so the row isn't blank.
                                let text = r.text_string();
                                let label = if text.is_empty() { trunc(&r.key_str(), 36) } else { trunc(&text, 36) };
                                ui.label(theme::data_text(label, 11.0, fg));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(theme::disp_text(kind, 9.0, theme::FAINT));
                                });
                            });
                            let resp = ui
                                .interact(
                                    inner.response.rect,
                                    ui.id().with(("str_row", rsel.asset_id, rsel.scene)),
                                    egui::Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                pick = Some(rsel);
                            }
                        }
                    }
                }
            },
        );
        if let Some(name) = toggle {
            if !self.strings.expanded.remove(&name) {
                self.strings.expanded.insert(name);
            }
        }
        if let Some(rsel) = pick {
            self.strings.selected = Some(rsel);
            if let Some(r) = gt.find_any(rsel.asset_id, rsel.scene) {
                self.strings.edit_buf = r.text_string();
            }
        }
        self.str_load = loaded; // hand the model back
    }

    fn strings_central(&mut self, ui: &mut egui::Ui) {
        if self.str_load.ready().is_none() {
            self.empty_state(
                ui,
                "Strings",
                "Everything the player reads — objectives, mission names, shop items, subtitles.",
                &[
                    "A UI string has NO name on disk: the file stores pandemic_hash(\"File_Text.Key\") and an empty key, so the dotted id is gone. You find a string by searching the text you can see in-game.",
                    "Nothing to press: the language on the left is fetched in the background the moment you pick it.",
                ],
            );
            return;
        }

        // Own the model for the rest of the frame so it does not alias the `&mut self` calls below
        // (staging, selection). Restored before returning — the guard above already proved it Ready.
        let loaded = std::mem::replace(&mut self.str_load, Load::Loading);
        let Load::Ready(gt) = &loaded else { self.str_load = loaded; return; };
        
        let sel = self.strings.selected;
        let mut play_key: Option<String> = None;
        let mut export_key: Option<String> = None;

        if let Some((rsel, r)) = sel.and_then(|s| gt.find_any(s.asset_id, s.scene).map(|r| (s, r))) {
            let orig = self.original_text(rsel).unwrap_or_else(|| r.text_string());
            let is_ui = r.is_ui();
            // The changelist key: a DNEC subtitle's asset_id can collide with a base one, so the scene
            // is part of the identity for staging and revert.
            let target = match rsel.scene {
                Some(sc) => format!("{} · 0x{:08X} @{:08X}", LANGS[self.strings.lang].0, rsel.asset_id, sc),
                None => format!("{} · 0x{:08X}", LANGS[self.strings.lang].0, rsel.asset_id),
            };

            ui.label(theme::data_text(
                match rsel.scene {
                    Some(sc) => format!("0x{:08X} · subtitle · scene 0x{:08X}", r.asset_id, sc),
                    None => format!("0x{:08X} · {}", r.asset_id, if is_ui { "UI string" } else { "VO subtitle" }),
                },
                9.5,
                theme::RED_DK,
            ));
            // A template that points at this hash NAMES it — the only way an anonymous UI string
            // gets a human label back.
            let named = self.xref.get(&r.asset_id).cloned().unwrap_or_default();
            let title = if let Some(n) = named.first().and_then(|e| self.template_name(*e)) {
                n
            } else if !r.key_str().is_empty() {
                r.key_str()
            } else {
                format!("string 0x{:08X}", r.asset_id)
            };
            ui.label(theme::poster_text(title, 21.0, theme::TX));
            ui.add_space(10.0);

            theme::eyebrow(ui, "The string");
            ui.add_space(4.0);
            // was / now — the core widget.
            egui::Frame::none()
                .fill(theme::COLD_SOFT)
                .stroke(egui::Stroke::new(1.0, theme::LINE))
                .inner_margin(egui::Margin::symmetric(11.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(theme::disp_text("Retail", 9.5, theme::COLD));
                        ui.add_space(6.0);
                        ui.label(theme::data_text(&orig, 12.0, theme::DIM));
                    });
                });
            egui::Frame::none()
                .fill(theme::EMBER_SOFT)
                .stroke(egui::Stroke::new(1.0, theme::EMBER_DK))
                .inner_margin(egui::Margin::symmetric(11.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.label(theme::disp_text("Yours", 9.5, theme::EMBER));
                        ui.add_space(6.0);
                        ui.add(
                            egui::TextEdit::multiline(&mut self.strings.edit_buf)
                                .desired_width(f32::INFINITY)
                                .desired_rows(3)
                                .font(egui::FontId::new(12.0, theme::data())),
                        );
                    });
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                theme::chip(ui, &format!("{} chars", self.strings.edit_buf.chars().count()), false, None);
                let changed = self.strings.edit_buf != orig;
                if changed {
                    theme::chip(ui, "unsaved", true, None);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::primary_button(ui, "Stage", changed).clicked() {
                        let after = self.strings.edit_buf.clone();
                        self.stage(
                            target.clone(),
                            orig.clone(),
                            after.clone(),
                            Op::SetString {
                                lang: self.strings.lang,
                                asset_id: rsel.asset_id,
                                scene: rsel.scene,
                                text: after,
                            },
                        );
                    }
                    if ui
                        .add_enabled(changed, egui::Button::new(theme::disp_text("Revert", 11.5, theme::TX)))
                        .clicked()
                    {
                        self.strings.edit_buf = orig.clone();
                        if let Some(p) = self.changes.iter().position(|c| c.target == target) {
                            let c = self.changes.remove(p);
                            self.unapply(&c);
                        }
                    }
                });
            });

            // Voice playback — VO lines and cinematic subtitles carry a Wwise voice; UI strings do not.
            // Understated (COLD utility hue), not the red STAMP reserved for the primary Stage action.
            if !r.is_ui() {
                ui.add_space(12.0);
                theme::eyebrow(ui, "Voice");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let play = ui
                        .add(
                            egui::Button::new(theme::disp_text(
                                format!("{}  Play line", theme::sym::PLAY),
                                11.0,
                                theme::COLD,
                            ))
                            .fill(theme::COLD_SOFT)
                            .stroke(egui::Stroke::new(1.0, theme::COLD_DK))
                            .rounding(egui::Rounding::ZERO),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if play.clicked() {
                        play_key = Some(r.key_str());
                    }
                    if ui
                        .add(egui::Button::new(theme::disp_text("Export…", 11.0, theme::DIM)).rounding(egui::Rounding::ZERO))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        export_key = Some(r.key_str());
                    }
                    if !self.audio_note.is_empty() {
                        ui.add_space(10.0);
                        ui.label(theme::data_text(&self.audio_note, 10.5, theme::FAINT));
                    }
                });
            }
        } else {
            self.empty_state(
                ui,
                "Pick a string",
                "Search the text on the left — that's the handle you have, because the id itself isn't stored.",
                &[],
            );
        }

        // add-new, always available
        ui.add_space(16.0);
        theme::card(ui, "Add a string", None, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::disp_text("Id", 11.0, theme::DIM));
                ui.add_sized(
                    [260.0, 20.0],
                    egui::TextEdit::singleline(&mut self.strings.new_id)
                        .hint_text("MyMod_Text.MyKey"),
                );
            });
            ui.horizontal(|ui| {
                ui.label(theme::disp_text("Text", 11.0, theme::DIM));
                ui.add_sized(
                    [340.0, 20.0],
                    egui::TextEdit::singleline(&mut self.strings.new_text),
                );
            });
            if !self.strings.new_id.is_empty() {
                let h = pandemic_hash(&self.strings.new_id);
                let clash = gt.find(h).is_some();
                ui.label(theme::data_text(
                    format!("→ 0x{h:08X}{}", if clash { "  ·  ALREADY EXISTS" } else { "" }),
                    10.5,
                    if clash { theme::RED } else { theme::COLD },
                ));
                if clash {
                    ui.label(theme::data_text(
                        "that id is taken — staging this would overwrite the existing line",
                        10.0,
                        theme::RED,
                    ));
                }
                if theme::primary_button(ui, "Stage new string", !clash).clicked() {
                    let dotted = self.strings.new_id.clone();
                    let text = self.strings.new_text.clone();
                    self.stage(
                        format!("{} · +{}", LANGS[self.strings.lang].0, dotted),
                        String::new(),
                        text.clone(),
                        Op::AddString { lang: self.strings.lang, dotted, text },
                    );
                    self.strings.new_id.clear();
                    self.strings.new_text.clear();
                }
            }
        });

        
        self.str_load = loaded; // hand the model back
        // Done after handing the model back so these can borrow `self` freely.
        if let Some(k) = play_key {
            self.play_line(&k);
        }
        if let Some(k) = export_key {
            self.export_line(&k);
        }
    }

    fn strings_context(&mut self, ui: &mut egui::Ui) {
        // Resolve the selection to (identity, on-disk key) once. `key` is owned so nothing keeps
        // borrowing the loaded model; it drives the Voice card below.
        let sel_rec: Option<(StrSel, String)> = self.strings.selected.and_then(|s| {
            self.str_load.ready().and_then(|g| g.find_any(s.asset_id, s.scene)).map(|r| (s, r.key_str()))
        });
        let sel = sel_rec.as_ref().map(|(s, _)| *s);
        theme::card(ui, "Coverage", None, |ui| {
            match sel {
                None => {
                    ui.label(theme::data_text("No string selected.", 10.5, theme::FAINT));
                }
                Some(sel) => {
                    let done: Vec<&str> = LANGS
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            self.changes.iter().any(|c| {
                                matches!(&c.op, Op::SetString { lang, asset_id, scene, .. }
                                    if *lang == *i && *asset_id == sel.asset_id && *scene == sel.scene)
                            })
                        })
                        .map(|(_, l)| l.0)
                        .collect();
                    ui.label(theme::data_text(
                        format!("{} of 7 languages", done.len()),
                        11.0,
                        if done.len() == LANGS.len() { theme::EMBER } else { theme::TX },
                    ));
                    ui.label(theme::data_text(
                        if done.is_empty() { "none yet".into() } else { done.join(" ") },
                        10.0,
                        theme::EMBER,
                    ));
                    if done.len() < LANGS.len() && !done.is_empty() {
                        ui.label(theme::data_text(
                            "players in the rest still see the retail line",
                            10.0,
                            theme::FAINT,
                        ));
                    }
                }
            }
        });

        // Voice: only VO lines and cinematic subtitles carry audio (a non-empty vo key). Gives the
        // rail per-line content — speaker, Wwise event, and the resolved .wem once a pack is loaded.
        if let Some((_, key)) = sel_rec.as_ref().filter(|(_, k)| !k.is_empty()) {
            theme::card(ui, "Voice", None, |ui| {
                if let Some(spk) = sab_formats::gametext::speaker(key) {
                    ui.label(theme::data_text(format!("speaker · {spk}"), 11.0, theme::TX));
                }
                ui.label(theme::data_text(format!("event · Play_{}", trunc(key, 32)), 10.0, theme::FAINT));
                match self.sound.as_ref().filter(|(l, _)| *l == self.strings.lang) {
                    Some((_, pack)) => match pack.wem_for_key(key) {
                        Some(w) => ui.label(theme::data_text(format!("audio · 0x{w:08x}.wem"), 10.5, theme::COLD)),
                        None => ui.label(theme::data_text("no audio resolved for this line", 10.0, theme::FAINT)),
                    },
                    None => ui.label(theme::data_text("press Play line to load the voice pack", 10.0, theme::FAINT)),
                };
            });
        }

        if let Some(id) = sel.map(|s| s.asset_id) {
            let refs = self.xref.get(&id).cloned().unwrap_or_default();
            theme::card(ui, "Named by", Some(&refs.len().to_string()), |ui| {
                if refs.is_empty() {
                    ui.label(theme::data_text(
                        "No template points at this hash, so there is no name to recover — the dotted id was never written to disk.",
                        10.0,
                        theme::FAINT,
                    ));
                } else {
                    for e in refs.iter().take(6) {
                        if let Some(n) = self.template_name(*e) {
                            ui.label(theme::data_text(n, 10.5, theme::TX));
                        }
                    }
                }
            });
        }
    }

    /// The `vgmstream-cli` to use: the user's configured path if present, else a `vgmstream-cli` on
    /// PATH (autodiscovery). Read LIVE from settings so a path chosen in Settings this session takes
    /// effect immediately — no restart, no cached copy. No hard-coded install locations.
    fn vgmstream_exe(&self) -> Option<std::path::PathBuf> {
        if let Some(p) = crate::settings::Settings::load().vgmstream() {
            return Some(std::path::PathBuf::from(p));
        }
        if std::process::Command::new("vgmstream-cli")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
        {
            return Some("vgmstream-cli".into());
        }
        None
    }

    /// The Wwise sound-pack path for a language — only EN/FR/DE/IT ship localized VO.
    fn sound_pck(&self, lang: usize) -> Option<String> {
        let name = match LANGS.get(lang).map(|l| l.0) {
            Some("EN") => "English(US)",
            Some("FR") => "French(Canada)",
            Some("DE") => "German",
            Some("IT") => "Italian",
            _ => return None,
        };
        Some(format!("{}/Sound/{name}.pck", self.md.game_dir))
    }

    /// Shared prep for Play/Export: open the current language's sound pack (cached), resolve the key
    /// to a `.wem`, extract its bytes, and locate `vgmstream`. Sets `audio_note` and returns `None`
    /// on any failure so the caller can just `?`-style bail.
    fn prepare_voice(&mut self, key: &str) -> Option<(std::path::PathBuf, u32, Vec<u8>)> {
        let lang = self.strings.lang;
        let Some(pck) = self.sound_pck(lang) else {
            self.audio_note = format!("no localized VO for {}", LANGS[lang].1);
            return None;
        };
        if self.sound.as_ref().map(|(l, _)| *l) != Some(lang) {
            match sab_formats::wwise::SoundPack::open(&pck) {
                Ok(p) => self.sound = Some((lang, p)),
                Err(e) => {
                    self.audio_note = format!("open sound pack: {e}");
                    return None;
                }
            }
        }
        let pack = &self.sound.as_ref().unwrap().1;
        let Some(wem) = pack.wem_for_key(key) else {
            self.audio_note = "no audio resolved for this line".into();
            return None;
        };
        let Some(bytes) = pack.wem_bytes(wem) else {
            self.audio_note = format!("wem {wem:08x} missing from pack");
            return None;
        };
        let Some(vgm) = self.vgmstream_exe() else {
            self.audio_note = "set vgmstream-cli in Settings → Voice audio (or put it on PATH)".into();
            return None;
        };
        Some((vgm, wem, bytes))
    }

    /// Resolve a VO/subtitle line, decode with vgmstream, and play it. The slow decode + playback run
    /// on a background thread so the UI never blocks.
    fn play_line(&mut self, key: &str) {
        let Some((vgm, wem, bytes)) = self.prepare_voice(key) else { return };
        let dir = std::env::temp_dir();
        let wem_path = dir.join(format!("sab_vo_{wem:08x}.wem"));
        let wav_path = dir.join(format!("sab_vo_{wem:08x}.wav"));
        if let Err(e) = std::fs::write(&wem_path, &bytes) {
            self.audio_note = format!("temp write: {e}");
            return;
        }
        self.audio_note = format!("playing 0x{wem:08x}");
        std::thread::spawn(move || {
            let decoded = std::process::Command::new(&vgm)
                .arg("-o")
                .arg(&wav_path)
                .arg(&wem_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if decoded {
                // PowerShell's SoundPlayer plays a PCM WAV with no extra dependencies.
                let _ = std::process::Command::new("powershell")
                    .args(["-NoProfile", "-Command"])
                    .arg(format!("(New-Object Media.SoundPlayer '{}').PlaySync()", wav_path.display()))
                    .status();
            }
            let _ = std::fs::remove_file(&wem_path);
            let _ = std::fs::remove_file(&wav_path);
        });
    }

    /// Resolve a VO/subtitle line and export it as a `.wav` to a user-chosen path. Runs
    /// synchronously (the user asked for a file and a native Save dialog is already blocking), so the
    /// outcome can be reported directly.
    fn export_line(&mut self, key: &str) {
        let Some((vgm, wem, bytes)) = self.prepare_voice(key) else { return };
        let default = format!("{}.wav", sanitize_filename(key));
        let Some(out) = rfd::FileDialog::new()
            .set_title("Export voice line as WAV")
            .set_file_name(default)
            .add_filter("WAV audio", &["wav"])
            .save_file()
        else {
            return;
        };
        let wem_path = std::env::temp_dir().join(format!("sab_vo_{wem:08x}.wem"));
        if let Err(e) = std::fs::write(&wem_path, &bytes) {
            self.audio_note = format!("temp write: {e}");
            return;
        }
        let ok = std::process::Command::new(&vgm)
            .arg("-o")
            .arg(&out)
            .arg(&wem_path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = std::fs::remove_file(&wem_path);
        self.audio_note = if ok {
            format!("exported → {}", out.display())
        } else {
            "vgmstream decode failed".into()
        };
    }

    /// The retail text for a string: the `before` of a staged change, else what's in memory.
    fn original_text(&self, sel: StrSel) -> Option<String> {
        if let Some(c) = self.changes.iter().find(|c| {
            matches!(&c.op, Op::SetString { asset_id: a, scene, lang, .. }
                if *a == sel.asset_id && *scene == sel.scene && *lang == self.strings.lang)
        }) {
            return Some(c.before.clone());
        }
        self.str_load.ready().and_then(|g| g.find_any(sel.asset_id, sel.scene)).map(|r| r.text_string())
    }

    // ============================================================== OBJECTS

    fn objects_nav(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "Templates");
        ui.add_space(3.0);
        // Name the source. It is not a file you can open — the DB is an AULB blob inside
        // `France/loosefiles_BinPC.pack` — and "GameTemplates.wsd" on its own was what let this page
        // show the DLC's five templates for so long without anyone asking which file that was.
        if self.load_state(ui, "GameTemplates (loosefiles_BinPC.pack)", &self.obj_load, None) {
            return;
        }

        let loaded = std::mem::replace(&mut self.obj_load, Load::Loading);
        let Load::Ready(gt) = &loaded else { self.obj_load = loaded; return; };
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(theme::sym::NO).clicked() {
                    self.objects.search.clear();
                }
                ui.add_sized(
                    [ui.available_width(), 20.0],
                    egui::TextEdit::singleline(&mut self.objects.search).hint_text("name or type"),
                );
            });
        });

        
        let needle = self.objects.search.to_ascii_lowercase();
        let edited: std::collections::HashSet<usize> = self
            .changes
            .iter()
            .filter_map(|c| match &c.op {
                Op::SetPair { entry, .. } => Some(*entry),
                _ => None,
            })
            .collect();

        // ---- the list, built once per search rather than once per frame ----
        //
        // This page was written against a five-entry table, where rebuilding the whole grouping
        // every frame cost nothing. Against the real 10,761-template DB the same code allocated two
        // Strings per template per frame just to lowercase a haystack, then cloned every name and
        // type into a map — tens of thousands of allocations at frame rate, which is what made the
        // window feel like it was struggling to load. Nothing about it was the load.
        if self.objects.rows_key.as_deref() != Some(needle.as_str()) {
            let mut by_type: std::collections::BTreeMap<&str, Vec<(usize, &str)>> =
                Default::default();
            let mut total = 0usize;
            for (i, e) in gt.entries.iter().enumerate() {
                if let Entry::Template(t) = e {
                    total += 1;
                    // Match without building a haystack: two case-insensitive passes, no allocation.
                    if !needle.is_empty()
                        && !contains_ci(&t.name, &needle)
                        && !contains_ci(&t.ttype, &needle)
                    {
                        continue;
                    }
                    by_type.entry(t.ttype.as_str()).or_default().push((i, t.name.as_str()));
                }
            }
            let mut rows = Vec::new();
            for (ty, items) in by_type {
                rows.push(ObjRow::Head(ty.to_string(), items.len()));
                for (i, name) in items.iter().take(400) {
                    rows.push(ObjRow::Item(*i, name.to_string()));
                }
                if items.len() > 400 {
                    rows.push(ObjRow::More(items.len() - 400));
                }
            }
            self.objects.rows = rows;
            self.objects.rows_key = Some(needle);
            self.objects.total = total;
        }

        let shown = self
            .objects
            .rows
            .iter()
            .filter(|r| matches!(r, ObjRow::Item(..)))
            .count();
        ui.label(theme::data_text(
            format!("{shown} of {} templates", self.objects.total),
            10.0,
            theme::FAINT,
        ));
        ui.add_space(3.0);

        // Virtualised, for the same reason the Strings and Icons lists are: `show()` builds a widget
        // for every row and merely clips the paint, so a filter matching thousands of templates laid
        // all of them out on every frame.
        let mut pick = None;
        let rows = &self.objects.rows;
        egui::ScrollArea::vertical().id_source("obj_rows").auto_shrink([false, false]).show_rows(
            ui,
            20.0,
            rows.len(),
            |ui, range| {
                for k in range {
                    match &rows[k] {
                        ObjRow::Head(ty, n) => {
                            ui.horizontal(|ui| {
                                ui.label(theme::disp_text(ty, 10.5, theme::FAINT));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(theme::data_text(
                                            format!("{n}"),
                                            9.5,
                                            theme::FAINT,
                                        ));
                                    },
                                );
                            });
                        }
                        ObjRow::Item(i, name) => {
                            let sel = self.objects.selected == Some(*i);
                            let is_ed = edited.contains(i);
                            ui.horizontal(|ui| {
                                let (d, _) = ui
                                    .allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                                ui.painter().rect_filled(
                                    d,
                                    egui::Rounding::ZERO,
                                    if is_ed { theme::EMBER } else { theme::G3 },
                                );
                                let fg = if sel {
                                    theme::RED
                                } else if is_ed {
                                    theme::TX
                                } else {
                                    theme::DIM
                                };
                                if ui
                                    .add(
                                        egui::Label::new(theme::data_text(name, 11.0, fg))
                                            .sense(egui::Sense::click()),
                                    )
                                    .clicked()
                                {
                                    pick = Some(*i);
                                }
                            });
                        }
                        ObjRow::More(n) => {
                            ui.label(theme::data_text(
                                format!("… {n} more — narrow the filter"),
                                9.5,
                                theme::FAINT,
                            ));
                        }
                    }
                }
            },
        );
        if let Some(i) = pick {
            self.objects.selected = Some(i);
            self.objects.edit_pair = None;
        }
        
        self.obj_load = loaded; // hand the model back
    }

    /// One pass over every pair: any 4-byte value is a candidate hash, so index value → templates.
    /// This is what lets the Icons page say "used by" and the Strings page recover a name.
    fn template_name(&self, entry: usize) -> Option<String> {
        match self.obj_load.ready()?.entries.get(entry) {
            Some(Entry::Template(t)) => Some(t.name.clone()),
            _ => None,
        }
    }

    /// What a 4-byte value POINTS AT, if anything.
    ///
    /// A hash is only meaningful once you know what it names, and the workshop already holds every
    /// corpus needed to find out: the texture pool, the template list, and the loaded language's
    /// strings. Resolving turns `1215273707` into `"Brothel Hat"` — which is the difference between
    /// a hex editor and a tool.
    fn resolve_hash(&self, v: u32) -> Option<(&'static str, String)> {
        if let Some((n, _, _, _, _)) = self.icon_recs().iter().find(|(_, h, _, _, _)| *h == v) {
            return Some(("texture", n.clone()));
        }
        if let Some(gt) = self.str_load.ready() {
            if let Some(r) = gt.find(v) {
                return Some(("string", r.text_string()));
            }
        }
        if let Some(gt) = self.obj_load.ready() {
            for e in &gt.entries {
                if let Entry::Template(t) = e {
                    if pandemic_hash(&t.name) == v {
                        return Some(("template", t.name.clone()));
                    }
                }
            }
        }
        None
    }

    /// Known properties whose value is a reference, not a number. Showing `245355.672f` for a Name
    /// is worse than useless — it invites someone to "fix" a float that was never a float.
    fn is_ref_property(name: &str) -> bool {
        matches!(
            name,
            "Name" | "Description" | "Image" | "Texture" | "Model" | "Face" | "Head" | "Skin"
        )
    }

    fn objects_central(&mut self, ui: &mut egui::Ui) {
        if self.obj_load.ready().is_none() {
            self.empty_state(
                ui,
                "Objects",
                "Every thing in the game is a template: a bag of {property → value} pairs.",
                &[
                    "A property name is a hash too, and only fifteen are known, so most show as 0x…. A value is four raw bytes that could be an int, a float, or a hash — every reading is shown at once so you can recognise the real one instead of guessing.",
                    "The game's own DLC/01/GameTemplates.wsd is the source, and it is already being read — nothing to press.",
                ],
            );
            return;
        }

        let loaded = std::mem::replace(&mut self.obj_load, Load::Loading);
        let Load::Ready(gt) = &loaded else { self.obj_load = loaded; return; };
        
        let sel = self.objects.selected;

        match sel.and_then(|i| match gt.entries.get(i) {
            Some(Entry::Template(t)) => Some((i, t)),
            _ => None,
        }) {
            None => self.empty_state(ui, "Pick a template", "Choose one on the left.", &[]),
            Some((idx, t)) => {
                let n_edit = self
                    .changes
                    .iter()
                    .filter(|c| matches!(&c.op, Op::SetPair { entry, .. } if *entry == idx))
                    .count();
                ui.label(theme::data_text(
                    format!("{} · {} properties · {} changed", t.ttype, t.pairs.len(), n_edit),
                    9.5,
                    theme::RED_DK,
                ));
                ui.label(theme::poster_text(&t.name, 21.0, theme::TX));
                ui.add_space(10.0);

                // known first — they get a real label
                let mut known: Vec<usize> = Vec::new();
                let mut unknown: Vec<usize> = Vec::new();
                for (pi, p) in t.pairs.iter().enumerate() {
                    if sab_formats::gametemplates::known_property_name(p.hash).is_some() {
                        known.push(pi)
                    } else {
                        unknown.push(pi)
                    }
                }

                let mut act: Option<(usize, Vec<u8>, String, String)> = None; // (pair, bytes, before, after)
                let mut wire: Option<usize> = None;

                if !known.is_empty() {
                    theme::eyebrow(ui, "Named properties");
                    ui.add_space(4.0);
                    for pi in known {
                        let p = &t.pairs[pi];
                        let name = sab_formats::gametemplates::known_property_name(p.hash).unwrap();
                        self.prop_row(ui, idx, pi, name, p, &mut act, &mut wire);
                    }
                }
                if !unknown.is_empty() {
                    ui.add_space(10.0);
                    theme::eyebrow(ui, "Unnamed properties");
                    ui.add_space(2.0);
                    ui.label(theme::data_text(
                        "the property name is a hash we can't spell yet — every reading of the value is shown so the real one is recognisable",
                        10.0,
                        theme::FAINT,
                    ));
                    ui.add_space(4.0);
                    for pi in unknown.into_iter().take(60) {
                        let p = &t.pairs[pi];
                        let label = format!("0x{:08X}", p.hash);
                        self.prop_row(ui, idx, pi, &label, p, &mut act, &mut wire);
                    }
                }

                if let Some((pi, bytes, before, after)) = act {
                    let tname = t.name.clone();
                    self.stage(
                        format!("{tname} · pair {pi}"),
                        before,
                        after,
                        Op::SetPair { entry: idx, pair: pi, bytes },
                    );
                }
                if let Some(pi) = wire {
                    self.objects.wiring = Some((idx, pi));
                }
            }
        }
        
        self.obj_load = loaded; // hand the model back
    }

    /// One property row. A texture-valued property shows the icon and offers the picker; everything
    /// else is an editable value with its four readings underneath when it is not a known type.
    #[allow(clippy::too_many_arguments)]
    fn prop_row(
        &mut self,
        ui: &mut egui::Ui,
        entry: usize,
        pi: usize,
        name: &str,
        p: &sab_formats::gametemplates::Pair,
        act: &mut Option<(usize, Vec<u8>, String, String)>,
        wire: &mut Option<usize>,
    ) {
        let staged = self
            .changes
            .iter()
            .find(|c| matches!(&c.op, Op::SetPair { entry: e, pair, .. } if *e == entry && *pair == pi));
        let edited = staged.is_some();
        let val = p.as_u32();
        let known = sab_formats::gametemplates::known_property_name(p.hash).is_some();
        let is_ref = known && Self::is_ref_property(name);
        // What this value points at, if we can tell.
        let resolved = val.and_then(|v| self.resolve_hash(v));
        // A texture reference gets the picker; anything else that resolves just gets named.
        let tex = match &resolved {
            Some(("texture", n)) => Some(n.clone()),
            _ => None,
        };

        egui::Frame::none()
            .fill(if edited { theme::EMBER_SOFT } else { theme::G2 })
            .stroke(egui::Stroke::new(1.0, if edited { theme::EMBER_DK } else { theme::LINE }))
            .inner_margin(egui::Margin::symmetric(9.0, 6.0))
            .outer_margin(egui::Margin { bottom: 4.0, ..Default::default() })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::disp_text(
                        name,
                        11.0,
                        if edited { theme::EMBER } else { theme::DIM },
                    ));
                    ui.add_space(4.0);

                    if let Some(tn) = &tex {
                        ui.label(theme::data_text(tn, 11.0, theme::TX));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(theme::disp_text(
                                    format!("Pick {}", theme::sym::ICONS),
                                    9.5,
                                    theme::COLD,
                                )))
                                .on_hover_text("choose a texture on the Icons page")
                                .clicked()
                            {
                                *wire = Some(pi);
                            }
                        });
                    } else {
                        // editable value
                        let is_open = self.objects.edit_pair == Some(pi);
                        if is_open {
                            let r = ui.add_sized(
                                [160.0, 20.0],
                                egui::TextEdit::singleline(&mut self.objects.edit_buf)
                                    .font(egui::FontId::new(11.5, theme::data())),
                            );
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                if let Ok(v) = parse_u32_any(&self.objects.edit_buf) {
                                    let before = val.map(|v| format!("{v}")).unwrap_or_default();
                                    *act = Some((
                                        pi,
                                        v.to_le_bytes().to_vec(),
                                        before,
                                        format!("{v}"),
                                    ));
                                    self.objects.edit_pair = None;
                                }
                            }
                        } else {
                            // A reference shows as a hash, never as a float — and if we can say
                            // what it points at, that comes first.
                            let shown = match (val, is_ref) {
                                (Some(v), true) => format!("0x{v:08X}"),
                                (Some(v), false) => readable(v),
                                (None, _) => format!("<{} bytes>", p.data.len()),
                            };
                            if ui
                                .add(
                                    egui::Label::new(theme::data_text(shown, 11.5, theme::TX))
                                        .sense(egui::Sense::click()),
                                )
                                .on_hover_text("click to edit")
                                .clicked()
                            {
                                self.objects.edit_pair = Some(pi);
                                self.objects.edit_buf =
                                    val.map(|v| v.to_string()).unwrap_or_default();
                            }
                            if let Some((kind, label)) = &resolved {
                                ui.label(theme::disp_text(*kind, 9.0, theme::FAINT));
                                ui.label(theme::data_text(trunc(label, 34), 11.0, theme::COLD));
                            } else if is_ref {
                                ui.label(theme::data_text(
                                    "unresolved",
                                    10.0,
                                    theme::FAINT,
                                ))
                                .on_hover_text(
                                    "nothing loaded names this hash — load Strings and scan Icons to resolve it",
                                );
                            }
                        }
                        if let Some(c) = staged {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(theme::data_text(
                                    format!("was {}", c.before),
                                    9.5,
                                    theme::COLD,
                                ));
                            });
                        }
                    }
                });

                // The four readings are for values whose TYPE is unknown. A named reference already
                // knows what it is, so showing four guesses there would be noise.
                if tex.is_none() && !is_ref {
                    if let Some(v) = val {
                        if !known {
                            four_readings(ui, v);
                        }
                    }
                }
            });
    }

    fn objects_context(&mut self, ui: &mut egui::Ui) {
        let sel = self.objects.selected;
        theme::card(ui, "Referenced by", None, |ui| {
            match sel {
                None => {
                    ui.label(theme::data_text("No template selected.", 10.5, theme::FAINT));
                }
                Some(i) => {
                    // who points at THIS template's name-hash
                    let nh = self.template_name(i).map(|n| pandemic_hash(&n));
                    let refs = nh.and_then(|h| self.xref.get(&h).cloned()).unwrap_or_default();
                    if refs.is_empty() {
                        ui.label(theme::data_text(
                            "Nothing else points at this template — changing it is contained.",
                            10.0,
                            theme::FAINT,
                        ));
                    } else {
                        ui.label(theme::data_text(
                            format!("{} template(s) reference it", refs.len()),
                            10.5,
                            theme::TX,
                        ));
                        for e in refs.iter().take(8) {
                            if let Some(n) = self.template_name(*e) {
                                ui.label(theme::data_text(n, 10.0, theme::DIM));
                            }
                        }
                    }
                }
            }
        });
    }

    // ============================================================== ICONS

    fn icons_nav(&mut self, ui: &mut egui::Ui) {
        theme::eyebrow(ui, "Texture pool");
        ui.add_space(3.0);
        ui.label(theme::data_text(
            "names are plaintext inside the pack; a template points at one by pandemic_hash(name)",
            10.0,
            theme::FAINT,
        ));
        ui.add_space(5.0);
        let prog = Some(self.icons.progress);
        if self.load_state(ui, "Palettes0.megapack", &self.icon_load, prog) {
            return;
        }
        ui.horizontal(|ui| {
            if theme::pill(ui, "referenced only", self.icons.only_used).clicked() {
                self.icons.only_used = !self.icons.only_used;
            }
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(theme::sym::NO).clicked() {
                    self.icons.search.clear();
                }
                ui.add_sized(
                    [ui.available_width(), 20.0],
                    egui::TextEdit::singleline(&mut self.icons.search).hint_text("filter name"),
                );
            });
        });
        let used = self.icon_recs().iter().filter(|(_, h, _, _, _)| self.xref.contains_key(h)).count();
        ui.label(theme::data_text(
            format!("{} records · {} referenced", self.icon_recs().len(), used),
            10.0,
            theme::FAINT,
        ));

        ui.add_space(10.0);
        theme::eyebrow(ui, "Reserve a name");
        ui.add_space(2.0);
        ui.label(theme::data_text(
            "a template can point at a texture before it exists — reserve the name, wire the hash, pack the DTEX with sab_dtex",
            10.0,
            theme::FAINT,
        ));
        ui.add_space(4.0);
        ui.add_sized(
            [ui.available_width(), 20.0],
            egui::TextEdit::singleline(&mut self.icons.new_name).hint_text("UI_Icon_MyThing_D"),
        );
        if !self.icons.new_name.is_empty() {
            let h = pandemic_hash(&self.icons.new_name);
            ui.label(theme::data_text(format!("→ 0x{h:08X}"), 10.5, theme::COLD));
            if theme::primary_button(ui, "Reserve", true).clicked() {
                let n = self.icons.new_name.clone();
                self.stage(
                    format!("+{n}"),
                    String::new(),
                    format!("0x{h:08X}"),
                    Op::ReserveTexture { name: n },
                );
                self.icons.new_name.clear();
            }
        }
    }

    fn icons_central(&mut self, ui: &mut egui::Ui) {
        if self.icon_load.ready().is_none() {
            self.empty_state(
                ui,
                "Icons",
                "Every texture in the pack, as pictures — because you pick an icon by looking at it.",
                &[
                    "A GameTemplate references a texture by pandemic_hash(name), so this page is also where you find the hash to wire into a property.",
                    "The pack is swept in the background as soon as the tool opens. Only the tiles you actually look at get decoded, and each takes the smallest mip that covers the tile.",
                ],
            );
            return;
        }
        if let Some((e, p)) = self.objects.wiring {
            egui::Frame::none()
                .fill(theme::RED_SOFT)
                .stroke(egui::Stroke::new(1.0, theme::RED_DK))
                .inner_margin(egui::Margin::symmetric(11.0, 8.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(theme::disp_text("Wiring", 10.0, theme::RED));
                        let n = self.template_name(e).unwrap_or_default();
                        ui.label(theme::data_text(
                            format!("{n} · pair {p} — click a texture to bind it"),
                            11.0,
                            theme::TX,
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("cancel").clicked() {
                                self.objects.wiring = None;
                            }
                        });
                    });
                });
            ui.add_space(8.0);
        }

        let needle = self.icons.search.to_ascii_lowercase();
        let rows: Vec<usize> = self
            .icon_recs()
            .iter()
            .enumerate()
            .filter(|(_, (n, h, _, _, _))| {
                (!self.icons.only_used || self.xref.contains_key(h))
                    && (needle.is_empty() || n.to_ascii_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();

        ui.horizontal(|ui| {
            theme::chip(ui, &format!("{} shown", rows.len()), false, None);
            let used = rows.iter().filter(|i| self.xref.contains_key(&self.icon_recs()[**i].1)).count();
            theme::chip(ui, &format!("{used} referenced"), used > 0, None);
        });
        ui.add_space(8.0);

        // ---- the sheet ----
        //
        // Two things keep this from locking the window on a pool of a couple of thousand records:
        //
        // 1. VIRTUALISE. `show_rows` only runs the closure for rows actually on screen. A plain
        //    ScrollArea + Grid does not do this — it builds every child and merely clips the paint,
        //    which meant landing on this page decoded ~2,000 BC textures and uploaded them all to
        //    the GPU in ONE frame. That was the freeze.
        // 2. BUDGET. Even a screenful is ~40 decodes; a fast scroll would still hitch. So only a
        //    few new textures are decoded per frame and the rest arrive over the next few, with a
        //    repaint requested so they fill in promptly. Already-cached tiles never count.
        let tile = 152.0;
        let cap_h = 42.0;
        let row_h = 96.0 + cap_h + 8.0;
        let cols = ((ui.available_width() / (tile + 8.0)).floor() as usize).max(1);
        let n_rows = rows.len().div_ceil(cols);
        let mut clicked: Option<usize> = None;
        let mut waiting = false;

        egui::ScrollArea::vertical()
            .id_source("sheet")
            .auto_shrink([false, false])
            .show_rows(ui, row_h, n_rows, |ui, vis| {
                for r in vis {
                    ui.horizontal(|ui| {
                        for c in 0..cols {
                            let Some(&i) = rows.get(r * cols + c) else { break };
                            let (name, hash, ei, off, len) = self.icon_recs()[i].clone();
                            let is_used = self.xref.contains_key(&hash);
                            let sel = self.icons.selected == Some(i);
                            let tex = self.thumb(hash, ei, off, len);
                            if tex.is_none() {
                                waiting = true;
                            }
                            let resp = ui
                                .vertical(|ui| {
                                    ui.set_width(tile);
                                    match tex {
                                        Some(t) => {
                                            let mut img = egui::Image::new(&t)
                                                .fit_to_exact_size(egui::vec2(tile, 96.0));
                                            if !is_used {
                                                img = img.tint(egui::Color32::from_gray(110));
                                            }
                                            ui.add(img);
                                        }
                                        None => {
                                            let (rr, _) = ui.allocate_exact_size(
                                                egui::vec2(tile, 96.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                rr,
                                                egui::Rounding::ZERO,
                                                theme::G2,
                                            );
                                            ui.painter().text(
                                                rr.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "·",
                                                egui::FontId::new(12.0, theme::data()),
                                                theme::FAINT,
                                            );
                                        }
                                    }
                                    ui.label(theme::data_text(
                                        trunc(&name, 18),
                                        10.5,
                                        if is_used { theme::TX } else { theme::DIM },
                                    ));
                                    ui.label(theme::data_text(
                                        format!("0x{hash:08X}"),
                                        10.0,
                                        if is_used { theme::EMBER } else { theme::FAINT },
                                    ));
                                })
                                .response;
                            let stroke = if sel {
                                egui::Stroke::new(1.0, theme::RED)
                            } else if is_used {
                                egui::Stroke::new(1.0, theme::EMBER_DK)
                            } else {
                                egui::Stroke::new(1.0, theme::LINE)
                            };
                            ui.painter().rect_stroke(resp.rect, egui::Rounding::ZERO, stroke);
                            if ui
                                .interact(resp.rect, ui.id().with(("tile", i)), egui::Sense::click())
                                .clicked()
                            {
                                clicked = Some(i);
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
            });
        if waiting {
            // pixels are in flight; come back for them
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
        }

        if let Some(i) = clicked {
            self.icons.selected = Some(i);
            // if a property is waiting on a value, bind it and go back
            if let Some((e, p)) = self.objects.wiring.take() {
                let (name, hash, _, _, _) = self.icon_recs()[i].clone();
                let before = self
                    .obj_load
                    .ready()
                    .and_then(|g| match g.entries.get(e) {
                        Some(Entry::Template(t)) => t.pairs.get(p).and_then(|x| x.as_u32()),
                        _ => None,
                    })
                    .map(|v| {
                        self.icon_recs()
                            .iter()
                            .find(|(_, h, _, _, _)| *h == v)
                            .map(|(n, _, _, _, _)| n.clone())
                            .unwrap_or_else(|| format!("0x{v:08X}"))
                    })
                    .unwrap_or_default();
                let tname = self.template_name(e).unwrap_or_default();
                self.stage(
                    format!("{tname} · pair {p}"),
                    before,
                    name,
                    Op::SetPair { entry: e, pair: p, bytes: hash.to_le_bytes().to_vec() },
                );
            }
        }
    }

    fn icons_context(&mut self, ui: &mut egui::Ui) {
        let sel = self.icons.selected;
        theme::card(ui, "Selected", None, |ui| match sel {
            None => {
                ui.label(theme::data_text("No texture selected.", 10.5, theme::FAINT));
            }
            Some(i) => {
                let (name, hash, _, _, _) = self.icon_recs()[i].clone();
                ui.label(theme::data_text(&name, 11.0, theme::TX));
                ui.label(theme::data_text(format!("0x{hash:08X}"), 10.5, theme::COLD));
                ui.add_space(4.0);
                let refs = self.xref.get(&hash).cloned().unwrap_or_default();
                ui.label(theme::data_text(
                    format!("used by {} template(s)", refs.len()),
                    10.5,
                    if refs.is_empty() { theme::FAINT } else { theme::EMBER },
                ));
                for e in refs.iter().take(8) {
                    if let Some(n) = self.template_name(*e) {
                        ui.label(theme::data_text(n, 10.0, theme::DIM));
                    }
                }
                if refs.is_empty() {
                    ui.label(theme::data_text(
                        "nothing references it — safe to repurpose",
                        10.0,
                        theme::FAINT,
                    ));
                }
            }
        });
        // Export the selected texture.
        if let Some(i) = sel {
            let mut want: Option<bool> = None; // Some(dds?)
            theme::card(ui, "Export", None, |ui| {
                ui.horizontal(|ui| {
                    let png = ui
                        .add(egui::Button::new(theme::disp_text("PNG", 11.0, theme::COLD)).fill(theme::COLD_SOFT).stroke(egui::Stroke::new(1.0, theme::COLD_DK)).rounding(egui::Rounding::ZERO))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    let dds = ui
                        .add(egui::Button::new(theme::disp_text("DDS", 11.0, theme::DIM)).rounding(egui::Rounding::ZERO))
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if png.clicked() {
                        want = Some(false);
                    }
                    if dds.clicked() {
                        want = Some(true);
                    }
                });
                if !self.io_note.is_empty() {
                    ui.add_space(4.0);
                    ui.label(theme::data_text(&self.io_note, 10.0, theme::FAINT));
                }
            });
            if let Some(dds) = want {
                self.export_texture(i, dds);
            }
        }
    }

    /// Decode the selected texture from the palettes pack and export it as PNG or DDS to a chosen
    /// path. Synchronous (a native Save dialog is already blocking); sets `io_note`.
    fn export_texture(&mut self, i: usize, dds: bool) {
        let Some((name, _hash, ei, off, len)) = self.icon_recs().get(i).cloned() else { return };
        let decoded = (|| -> Result<dtex::CpuTexture, String> {
            let mp = Megapack::open(&self.md.palettes())?;
            let e = mp.entries().get(ei).ok_or("texture record no longer present")?;
            let sub = mp.slice(e);
            if off + len > sub.len() {
                return Err("texture record out of range".into());
            }
            dtex::decode(&sub[off..off + len])
        })();
        let tex = match decoded {
            Ok(t) => t,
            Err(e) => {
                self.io_note = format!("decode: {e}");
                return;
            }
        };
        let ext = if dds { "dds" } else { "png" };
        let Some(out) = rfd::FileDialog::new()
            .set_title("Export texture")
            .set_file_name(format!("{}.{ext}", sanitize_filename(&name)))
            .add_filter(&ext.to_uppercase(), &[ext])
            .save_file()
        else {
            return;
        };
        let res = if dds {
            write_dds(&out, tex.width, tex.height, &tex.rgba)
        } else {
            write_png(&out, tex.width, tex.height, &tex.rgba)
        };
        self.io_note = match res {
            Ok(()) => format!("exported → {}", out.display()),
            Err(e) => e,
        };
    }

    /// The texture for a record, or `None` while the worker is still on it. Never blocks: an
    /// un-cached tile fires one request and draws a placeholder until the pixels arrive.
    fn thumb(&mut self, hash: u32, ei: usize, off: usize, len: usize) -> Option<egui::TextureHandle> {
        if let Some(hit) = self.thumbs.cache.get(&hash) {
            return hit.clone();
        }
        if self.thumbs.pending.insert(hash) {
            if let Some(q) = &self.thumbs.req {
                let _ = q.send((hash, ei, off, len));
            }
        }
        None
    }

    // ------------------------------------------------------------------ shared

    /// A page with nothing loaded still has to teach: say what the page is for and what the format
    /// makes awkward, rather than showing an empty pane.
    fn empty_state(&self, ui: &mut egui::Ui, title: &str, sub: &str, notes: &[&str]) {
        ui.add_space(28.0);
        ui.label(theme::poster_text(title, 26.0, theme::TX));
        ui.add_space(4.0);
        ui.label(theme::data_text(sub, 12.0, theme::DIM));
        ui.add_space(14.0);
        // Bounded so long explanations wrap into a readable column instead of running off the panel.
        let wrap = ui.available_width().min(620.0);
        for n in notes {
            ui.horizontal_top(|ui| {
                ui.label(theme::data_text("·", 12.0, theme::RED));
                ui.add_space(4.0);
                ui.allocate_ui(egui::vec2(wrap, 0.0), |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(*n).size(11.5).color(theme::FAINT))
                            .wrap(),
                    );
                });
            });
            ui.add_space(7.0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// One pass over every pair: any 4-byte value is a candidate hash, so index value → templates.
/// This is what lets the Icons page say "used by" and the Strings page recover a name.
fn build_xref(gt: &GameTemplates) -> std::collections::HashMap<u32, Vec<usize>> {
    let mut xref: std::collections::HashMap<u32, Vec<usize>> = Default::default();
    for (i, e) in gt.entries.iter().enumerate() {
        if let Entry::Template(t) = e {
            for p in &t.pairs {
                if let Some(v) = p.as_u32() {
                    let slot = xref.entry(v).or_default();
                    // Entries arrive in ascending `i`, so a repeat can only be the last one pushed.
                    // The `contains` this replaces was a linear scan per pair — fine over the DLC's
                    // five templates, ~800k scans over the real DB.
                    if slot.last() != Some(&i) {
                        slot.push(i);
                    }
                }
            }
        }
    }
    xref
}

/// `haystack.to_lowercase().contains(needle)` without the allocation. `needle` must already be
/// lowercase. Called once per template per rebuild, which is often enough to matter.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

fn trunc(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((b, _)) => format!("{}…", &s[..b]),
        None => s.to_string(),
    }
}

/// Make a string safe as a filename stem (VO keys already are; this guards odd characters).
fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' })
        .collect();
    if cleaned.is_empty() { "export".into() } else { cleaned }
}

/// Write a decoded texture as an 8-bit RGBA PNG.
fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

/// Write a decoded texture as an **uncompressed 32-bit DDS** (`A8R8G8B8`/BGRA) — any tool reads it,
/// and it re-imports cleanly. Not byte-faithful to the original DXT compression (use `sab_dtex` for
/// that); this is a viewable/editable export.
fn write_dds(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let mut out = Vec::with_capacity(128 + rgba.len());
    out.extend_from_slice(b"DDS ");
    let mut hdr = [0u32; 31]; // 124-byte DDS_HEADER
    hdr[0] = 124; // dwSize
    hdr[1] = 0x1 | 0x2 | 0x4 | 0x8 | 0x1000; // CAPS|HEIGHT|WIDTH|PITCH|PIXELFORMAT
    hdr[2] = h; // dwHeight
    hdr[3] = w; // dwWidth
    hdr[4] = w * 4; // dwPitchOrLinearSize
    // ddspf at u32 index 18 (byte offset 72 within the 124-byte header)
    hdr[18] = 32; // pfSize
    hdr[19] = 0x1 | 0x40; // ALPHAPIXELS | RGB
    hdr[20] = 0; // fourCC (0 = uncompressed)
    hdr[21] = 32; // bits per pixel
    hdr[22] = 0x00ff_0000; // R mask
    hdr[23] = 0x0000_ff00; // G mask
    hdr[24] = 0x0000_00ff; // B mask
    hdr[25] = 0xff00_0000; // A mask
    hdr[26] = 0x1000; // dwCaps = TEXTURE
    for v in hdr {
        out.extend_from_slice(&v.to_le_bytes());
    }
    // A8R8G8B8 is BGRA in memory.
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}


/// The most plausible single-line form of a 4-byte value.
fn readable(v: u32) -> String {
    let f = f32::from_bits(v);
    if f.is_finite() && f != 0.0 && f.abs() >= 1e-3 && f.abs() < 1e7 {
        format!("{v}  ·  {f:.3}f")
    } else {
        format!("{v}")
    }
}

/// Show a value read every way at once, with the likeliest lit. Beats making the modder choose a
/// type from a radio before the UI will show them anything.
fn four_readings(ui: &mut egui::Ui, v: u32) {
    let f = f32::from_bits(v);
    let float_ok = f.is_finite() && f != 0.0 && f.abs() >= 1e-3 && f.abs() < 1e7;
    // A small int is a far more likely count than a denormal float.
    let int_ok = v < 100_000;
    let ascii: String = v
        .to_le_bytes()
        .iter()
        .map(|b| if b.is_ascii_graphic() { *b as char } else { '·' })
        .collect();
    let ascii_ok = v.to_le_bytes().iter().filter(|b| b.is_ascii_graphic()).count() >= 3;

    egui::Frame::none()
        .fill(theme::G0)
        .stroke(egui::Stroke::new(1.0, theme::LINE))
        .inner_margin(egui::Margin::symmetric(0.0, 0.0))
        .outer_margin(egui::Margin { top: 5.0, ..Default::default() })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let cell = |ui: &mut egui::Ui, k: &str, val: String, lit: bool| {
                    egui::Frame::none()
                        .fill(if lit { theme::COLD_SOFT } else { egui::Color32::TRANSPARENT })
                        .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.label(theme::disp_text(
                                    k,
                                    9.0,
                                    if lit { theme::COLD } else { theme::FAINT },
                                ));
                                ui.label(theme::data_text(
                                    val,
                                    10.5,
                                    if lit { theme::TX } else { theme::DIM },
                                ));
                            });
                        });
                };
                cell(ui, "as int", format!("{v}"), int_ok && !float_ok);
                cell(
                    ui,
                    "as float",
                    if float_ok { format!("{f:.3}") } else { "—".into() },
                    float_ok,
                );
                cell(ui, "as hash", format!("0x{v:08X}"), false);
                cell(ui, "as ascii", ascii, ascii_ok);
            });
        });
}

// ---------------------------------------------------------------------------------------------
// DLC slots — what the engine actually mounts
// ---------------------------------------------------------------------------------------------
//
// Everything below is read off the retail PC binary's DLC state machine (`FUN_00990d30`
// @0x00990d30) and the mount it drives (`FUN_009906c0` @0x009906c0). Three rules matter, and all
// three are the kind that fail silently:
//
// 1. The slot list is hardcoded to four — `sprintf_s(..., "dlc\\%02d", i + 1)` with a count of 4.
// 2. A slot only becomes a candidate when its `dlcinfo.ini` OPENS (`FUN_00990320` sets the flag);
//    the payload is never looked at otherwise.
// 3. Exactly one slot mounts: the highest `dlclevel`, compared with a strict `<`, so ties keep the
//    lowest slot. The loser is not layered underneath — it simply does not load.

/// The slots the engine probes, in the order it probes them.
const DLC_SLOTS: [&str; 4] = ["01", "02", "03", "04"];

/// Files `FUN_009906c0` opens by hardcoded name inside whichever slot won. Documented here because
/// two of them are traps: `dlc01mega0.megapack` keeps that literal name in EVERY slot, and the
/// `megapack=` key in `dlcinfo.ini` is parsed into the DLC record and then never read — renaming a
/// pack and pointing the key at it silently loads nothing.
#[allow(dead_code)]
const DLC_MOUNTED_NAMES: [&str; 7] = [
    "Animations.pack",
    "dlc01mega0.megapack",
    "dynamic0.megapack",
    "palettes0.megapack",
    "global.map",
    "GameTemplates.wsd",
    "France.map",
];

/// Create the parent directory and write, reporting WHICH step failed. `create_dir_all` used to be
/// discarded here, so a permission problem surfaced one line later as a confusing write error.
fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("create {}: {e}", p.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("write {path}: {e}"))
}

/// A slot's `dlclevel`, or `None` when it has no `dlcinfo.ini` — which is exactly the engine's test
/// for "this slot exists", so `None` means invisible however complete the payload is.
///
/// Key matching mirrors the engine's `__strnicmp(line, "dlclevel", 8)`: the comparison starts at the
/// beginning of the LINE, which is why `//dlclevel=9` is a comment and an indented key is not read.
/// A file with no `dlclevel` at all reads as 0, matching the zeroed DLC record.
fn read_dlclevel(dir: &str) -> Option<i32> {
    let text = std::fs::read_to_string(format!("{dir}/dlcinfo.ini")).ok()?;
    let mut level = 0;
    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else { continue };
        if !key.to_ascii_lowercase().starts_with("dlclevel") {
            continue;
        }
        // `_atol` stops at the first non-digit, so trailing junk and quotes are harmless.
        let digits: String =
            val.trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        if let Ok(n) = digits.parse::<i32>() {
            level = n;
        }
    }
    Some(level)
}

/// Which slot the engine will mount, and at what level. `None` when no slot has a manifest.
fn winning_slot(game_dir: &str) -> Option<(&'static str, i32)> {
    let mut best: Option<(&'static str, i32)> = None;
    for slot in DLC_SLOTS {
        let Some(level) = read_dlclevel(&format!("{game_dir}/DLC/{slot}")) else { continue };
        // Strict `<`: a tie keeps the slot already held, which is the earlier (lower) one.
        if best.is_none_or(|(_, b)| b < level) {
            best = Some((slot, level));
        }
    }
    best
}

/// The manifest. Retail's shape, with only `dlclevel` doing any work.
///
/// `savedlclevel` is inert twice over — the parser wants `saveddlclevel` and nothing in the mount
/// reads it — and is kept only so the file looks like the one it sits beside. `hasnudity` matches
/// `DLC/01` because the mod's slot carries that same mirrored payload.
fn dlcinfo(mod_name: &str, level: i32) -> String {
    format!(
        "// {mod_name} — published by sab_workshop\n\
         // The engine mounts ONE DLC: the highest dlclevel with a dlcinfo.ini, ties to the lowest\n\
         // slot. This level beats every other slot on disk at publish time. Raising a rival slot's\n\
         // level, or adding one, takes the mount away from this mod.\n\
         megapack=\"Files\\dlc01mega0.megapack\"\n\
         dlclevel={level}\n\
         savedlclevel=0\n\
         hasnudity=1\n"
    )
}

/// Copy the retail DLC's payload into the mod's slot, skipping anything the mod publishes itself.
///
/// This is not belt-and-braces, it is the cost of winning the mount. `FUN_00990d30` mounts one slot;
/// when that is the mod's, `DLC/01` is not mounted at all and everything it provided —
/// `dlc01mega0.megapack`, `Animations.pack`, the France packs, `Sound\` — stops loading. The engine
/// looks for those inside the winning slot by hardcoded name, so the mod has to carry them.
///
/// `skip` holds slot-relative paths the mod wrote; they are compared case-insensitively with `/`
/// separators, since the mod's `GameTemplates.wsd` must not be clobbered by retail's. A destination
/// that already matches in size and is no older than its source is left alone, so republishing after
/// the first mirror costs nothing.
///
/// Returns (files copied, bytes copied).
fn mirror_dlc(from: &str, to: &str, skip: &[String]) -> Result<(usize, u64), String> {
    fn walk(
        from: &std::path::Path,
        to: &std::path::Path,
        rel: &str,
        skip: &[String],
        n: &mut usize,
        bytes: &mut u64,
    ) -> Result<(), String> {
        let dir = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
        for entry in dir {
            let entry = entry.map_err(|e| format!("read {}: {e}", from.display()))?;
            let name = entry.file_name().to_string_lossy().to_string();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let src = entry.path();
            let dst = to.join(&name);
            if src.is_dir() {
                walk(&src, &dst, &child_rel, skip, n, bytes)?;
                continue;
            }
            if skip.iter().any(|s| s.eq_ignore_ascii_case(&child_rel)) {
                continue;
            }
            let meta = src.metadata().map_err(|e| format!("stat {}: {e}", src.display()))?;
            if let Ok(d) = dst.metadata() {
                let fresh = match (d.modified(), meta.modified()) {
                    (Ok(a), Ok(b)) => a >= b,
                    _ => false,
                };
                if d.len() == meta.len() && fresh {
                    continue;
                }
            }
            std::fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
            *bytes += std::fs::copy(&src, &dst)
                .map_err(|e| format!("copy {} → {}: {e}", src.display(), dst.display()))?;
            *n += 1;
        }
        Ok(())
    }

    let (mut n, mut bytes) = (0usize, 0u64);
    let src = std::path::Path::new(from);
    if !src.is_dir() {
        // No retail DLC to carry (a stripped install). The mod still publishes; it just cannot
        // preserve content that was never there.
        return Ok((0, 0));
    }
    walk(src, std::path::Path::new(to), "", skip, &mut n, &mut bytes)?;
    Ok((n, bytes))
}

/// Accept decimal, `0x…` hex, or a float — whichever the modder typed.
fn parse_u32_any(t: &str) -> Result<u32, String> {
    let t = t.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u32::from_str_radix(h, 16).map_err(|e| e.to_string());
    }
    if let Ok(i) = t.parse::<i64>() {
        return Ok(i as u32);
    }
    if let Ok(f) = t.parse::<f32>() {
        return Ok(f.to_bits());
    }
    Err("not a number".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_export_round_trips() {
        let (w, h) = (2u32, 2u32);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let p = std::env::temp_dir().join("sab_png_test.png");
        write_png(&p, w, h, &rgba).expect("write");
        let dec = png::Decoder::new(std::fs::File::open(&p).unwrap());
        let mut reader = dec.read_info().unwrap();
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (w, h));
        assert_eq!(&buf[..info.buffer_size()], &rgba[..]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dds_export_header() {
        let p = std::env::temp_dir().join("sab_dds_test.dds");
        write_dds(&p, 4, 2, &vec![0xAAu8; 4 * 2 * 4]).expect("write");
        let b = std::fs::read(&p).unwrap();
        assert_eq!(&b[0..4], b"DDS ");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 124); // dwSize
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 2); // height
        assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 4); // width
        assert_eq!(b.len(), 128 + 4 * 2 * 4); // header + BGRA pixels
        let _ = std::fs::remove_file(&p);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sab_workshop_dlc_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch");
        d
    }

    fn slot(root: &std::path::Path, n: &str, ini: Option<&str>) -> String {
        let dir = root.join("DLC").join(n);
        std::fs::create_dir_all(&dir).expect("slot");
        if let Some(text) = ini {
            std::fs::write(dir.join("dlcinfo.ini"), text).expect("ini");
        }
        dir.to_string_lossy().replace('\\', "/")
    }

    /// Retail's own manifest, verbatim — the shape every check here has to agree with.
    const RETAIL_01: &str = "megapack=\"Files\\dlc01mega0.megapack\"\ndlclevel=1\nsavedlclevel=0\nhasnudity=1\n//autorunlua=\"\"\n";

    #[test]
    fn dlclevel_reads_retail_and_ignores_lookalikes() {
        let root = scratch("read");
        let one = slot(&root, "01", Some(RETAIL_01));
        assert_eq!(read_dlclevel(&one), Some(1));

        // `savedlclevel` must not be mistaken for `dlclevel`, and a commented key is a comment:
        // the engine compares from the start of the line, so neither matches there either.
        let two = slot(&root, "02", Some("savedlclevel=9\n//dlclevel=7\nhasnudity=0\n"));
        assert_eq!(read_dlclevel(&two), Some(0));

        // No manifest is not level 0 — it is "not a slot", which is the whole bug this catches.
        let three = slot(&root, "03", None);
        assert_eq!(read_dlclevel(&three), None);
    }

    /// A mod that merely matches retail's level loses: the engine's compare is strict, so the tie
    /// keeps the lower slot. This is why `publish` writes one MORE than the best rival.
    #[test]
    fn ties_keep_the_lowest_slot_and_the_highest_level_wins() {
        let root = scratch("win");
        let game = root.to_string_lossy().replace('\\', "/");
        slot(&root, "01", Some(RETAIL_01));

        slot(&root, "02", Some("dlclevel=1\n"));
        assert_eq!(winning_slot(&game), Some(("01", 1)), "a tie must not take the mount");

        slot(&root, "02", Some("dlclevel=2\n"));
        assert_eq!(winning_slot(&game), Some(("02", 2)));

        // A payload with no manifest stays invisible however high the other slots are.
        slot(&root, "03", None);
        assert_eq!(winning_slot(&game), Some(("02", 2)));
    }

    /// What `publish` writes must be what `winning_slot` then reads — these two agreeing is the
    /// entire guarantee that the published mod is the one that mounts.
    #[test]
    fn published_manifest_beats_retail() {
        let root = scratch("manifest");
        let game = root.to_string_lossy().replace('\\', "/");
        slot(&root, "01", Some(RETAIL_01));
        let two = slot(&root, "02", None);

        let rival = read_dlclevel(&slot(&root, "01", Some(RETAIL_01))).unwrap_or(0);
        std::fs::write(format!("{two}/dlcinfo.ini"), dlcinfo("Test mod", rival + 1)).expect("write");

        assert_eq!(read_dlclevel(&two), Some(2));
        assert_eq!(winning_slot(&game), Some(("02", 2)));
    }

    #[test]
    fn mirror_carries_retail_payload_but_never_the_mods_own_files() {
        let root = scratch("mirror");
        let from = root.join("from");
        let to = root.join("to");
        std::fs::create_dir_all(from.join("France")).expect("mk");
        std::fs::write(from.join("dlc01mega0.megapack"), b"pack").expect("w");
        std::fs::write(from.join("GameTemplates.wsd"), b"retail templates").expect("w");
        std::fs::write(from.join("France/0.pack"), b"france").expect("w");
        std::fs::create_dir_all(&to).expect("mk");
        std::fs::write(to.join("GameTemplates.wsd"), b"mine").expect("w");

        let f = from.to_string_lossy().to_string();
        let to_s = to.to_string_lossy().to_string();
        let skip = vec!["gametemplates.WSD".to_string()];
        let (n, bytes) = mirror_dlc(&f, &to_s, &skip).expect("mirror");
        assert_eq!(n, 2, "the two retail files, not the mod's own");
        assert_eq!(bytes, 10);
        assert_eq!(std::fs::read(to.join("GameTemplates.wsd")).unwrap(), b"mine");
        assert_eq!(std::fs::read(to.join("France/0.pack")).unwrap(), b"france");

        // Republishing must not re-copy 174 MB every time.
        let (n2, _) = mirror_dlc(&f, &to_s, &skip).expect("mirror twice");
        assert_eq!(n2, 0);
    }

    /// The install these tests read from, found the same way the app finds it. Never a literal path:
    /// the one setting everything derives from is `settings::game_dir`, and a test that hardcodes it
    /// passes or fails on where the game happens to live.
    fn install() -> Option<String> {
        let dir = crate::settings::detect_install();
        if dir.is_none() {
            eprintln!("skip: no Saboteur install detected");
        }
        dir
    }

    /// Ground truth: retail's own manifest, read off the real install rather than a fixture. If this
    /// ever disagrees, the level `publish` picks is being computed against the wrong baseline.
    #[test]
    fn retail_dlc01_is_level_one_on_disk() {
        let Some(game) = install() else { return };
        assert_eq!(read_dlclevel(&format!("{game}/DLC/01")), Some(1));
    }

    /// Stage a temp install carrying a real retail `GameText.dlg`, or `None` when the game is not
    /// on this machine.
    fn staged_install(name: &str) -> Option<(std::path::PathBuf, String)> {
        let retail = format!("{}/Cinematics/Dialog/English/GameText.dlg", install()?);
        if !std::path::Path::new(&retail).exists() {
            eprintln!("skip: {retail} not present");
            return None;
        }
        let root = scratch(name);
        let dir = root.join("Cinematics/Dialog/English");
        std::fs::create_dir_all(&dir).expect("mk");
        std::fs::copy(&retail, dir.join("GameText.dlg")).expect("seed");
        let game = root.to_string_lossy().replace('\\', "/");
        Some((root, game))
    }

    /// An edit of an EXISTING string must land in the base file — a DLC slot cannot override a hash
    /// the base map already holds (`FUN_009603f0` rejects the duplicate) — and must leave retail
    /// recoverable. The slot must stay untouched: claiming the mount for a text edit would unmount
    /// DLC/01 for nothing.
    #[test]
    fn editing_a_retail_string_patches_base_and_leaves_the_slot_alone() {
        let Some((root, game)) = staged_install("setstring") else { return };
        let mut ed = Editor::new(&game, 0, "02", "Test mod");
        // Editor::new restores the real mod document for this slot; a test must not read
        // (or assert against) whatever the person running it happens to have staged.
        ed.changes.clear();

        let base = root.join("Cinematics/Dialog/English/GameText.dlg");
        let retail = std::fs::read(&base).expect("read");
        let gt = GameText::parse(&retail).expect("parse");
        let id = gt.records[0].asset_id;

        ed.changes.push(Change {
            target: format!("EN 0x{id:08X}"),
            before: "before".into(),
            after: "after".into(),
            op: Op::SetString { lang: 0, asset_id: id, scene: None, text: "PATCHED BY TEST".into() },
        });
        let note = ed.publish_inner().expect("publish");

        let bak = root.join("Cinematics/Dialog/English/GameText.dlg.sabbak");
        assert!(bak.exists(), "retail must be preserved: {note}");
        assert_eq!(std::fs::read(&bak).unwrap(), retail, "the backup must BE retail");
        let patched = GameText::parse(&std::fs::read(&base).unwrap()).expect("reparse");
        assert_eq!(patched.find(id).map(|r| r.text_string()), Some("PATCHED BY TEST".to_string()));
        assert!(!root.join("DLC/02").exists(), "a base-only edit must not touch the slot");

        // A slot file and manifest left by an earlier publish must be cleaned up, not left lying
        // there looking like a live override — that residue is what made the last diagnosis hard.
        let stale = root.join("DLC/02/Cinematics/Dialog/English/GameText.dlg");
        std::fs::create_dir_all(stale.parent().unwrap()).expect("mk");
        std::fs::write(&stale, &retail).expect("stale");
        std::fs::write(root.join("DLC/02/dlcinfo.ini"), dlcinfo("old", 2)).expect("stale ini");
        ed.publish_inner().expect("republish over residue");
        assert!(!stale.exists(), "the dead slot copy must be removed");
        assert!(!root.join("DLC/02/dlcinfo.ini").exists(), "an empty slot must stop mounting");

        // Publishing again reads the .sabbak, so the edit never stacks on yesterday's output.
        assert_eq!(ed.md.gametext_src(0), bak.to_string_lossy().replace('\\', "/"));
        ed.publish_inner().expect("republish");
        assert_eq!(std::fs::read(&bak).unwrap(), retail, "republish must not re-back-up the patch");

        // And unpublish puts it back byte for byte.
        ed.unpublish();
        assert_eq!(std::fs::read(&base).unwrap(), retail);
        assert!(!bak.exists(), "the backup is consumed by the restore");
    }

    /// A DNEC cinematic subtitle edit is a `SetString` with a `scene`; it patches the base file (same
    /// as a base-record edit — the DNEC records live in the base `GameText.dlg`), and the target must
    /// resolve through the scene, not by asset_id alone.
    #[test]
    fn editing_a_subtitle_patches_the_base_dnec_section() {
        let Some((root, game)) = staged_install("setsub") else { return };
        let mut ed = Editor::new(&game, 0, "02", "Test mod");
        ed.changes.clear();

        let base = root.join("Cinematics/Dialog/English/GameText.dlg");
        let retail = std::fs::read(&base).expect("read");
        let gt = GameText::parse(&retail).expect("parse");
        let grp = gt.dnec_groups().first().expect("a DNEC scene group");
        let (scene, sub_id, was) = {
            let r = &grp.records[0];
            (grp.scene_hash, r.asset_id, r.text_string())
        };

        ed.changes.push(Change {
            target: format!("EN 0x{sub_id:08X} @{scene:08X}"),
            before: was.clone(),
            after: "SUBTITLE PATCHED".into(),
            op: Op::SetString { lang: 0, asset_id: sub_id, scene: Some(scene), text: "SUBTITLE PATCHED".into() },
        });
        ed.publish_inner().expect("publish");

        // The base file's DNEC record now carries the new text; the base section is otherwise intact.
        let patched = GameText::parse(&std::fs::read(&base).unwrap()).expect("reparse");
        assert_eq!(
            patched.find_any(sub_id, Some(scene)).map(|r| r.text_string()),
            Some("SUBTITLE PATCHED".to_string())
        );
        assert_eq!(patched.records.len(), gt.records.len(), "base record count unchanged");
        assert!(!root.join("DLC/02").exists(), "a subtitle edit is a base edit, not a slot add");

        // Unpublish restores retail byte-for-byte (proves the DNEC rebuild is exact).
        ed.unpublish();
        assert_eq!(std::fs::read(&base).unwrap(), retail);
    }

    /// A NEW id is the one string change a DLC slot can carry, so it goes there — and only then does
    /// the slot need a manifest.
    #[test]
    fn adding_a_string_goes_to_the_slot_with_a_manifest() {
        let Some((root, game)) = staged_install("addstring") else { return };
        let mut ed = Editor::new(&game, 0, "02", "Test mod");
        // Editor::new restores the real mod document for this slot; a test must not read
        // (or assert against) whatever the person running it happens to have staged.
        ed.changes.clear();
        ed.changes.push(Change {
            target: "EN MyMod_Text.MyKey".into(),
            before: String::new(),
            after: "hello".into(),
            op: Op::AddString { lang: 0, dotted: "MyMod_Text.MyKey".into(), text: "hello".into() },
        });
        let note = ed.publish_inner().expect("publish");

        assert!(root.join("DLC/02/Cinematics/Dialog/English/GameText.dlg").exists());
        assert_eq!(read_dlclevel(&format!("{game}/DLC/02")), Some(1), "no rival slot here");
        assert!(note.contains("engine mounts DLC/02"), "{note}");
        assert!(
            !root.join("Cinematics/Dialog/English/GameText.dlg.sabbak").exists(),
            "an addition must not patch the base file"
        );

        let out = GameText::parse(&std::fs::read(root.join("DLC/02/Cinematics/Dialog/English/GameText.dlg")).unwrap())
            .expect("parse");
        assert_eq!(out.find(pandemic_hash("MyMod_Text.MyKey")).map(|r| r.text_string()), Some("hello".into()));
    }

    /// The mod must survive closing the window. Every op kind round-trips, because a document that
    /// drops one silently is worse than no document at all.
    #[test]
    fn the_mod_document_survives_a_restart() {
        let dir = scratch("doc").join("nested");
        let path = dir.join("dlc-02.json");
        let changes = vec![
            Change {
                target: "EN 0x11111111".into(),
                before: "CHECKPOINT".into(),
                after: "MODDED CHECKPOINT".into(),
                op: Op::SetString { lang: 0, asset_id: 0x1111_1111, scene: None, text: "MODDED CHECKPOINT".into() },
            },
            Change {
                target: "EN MyMod_Text.MyKey".into(),
                before: String::new(),
                after: "hello".into(),
                op: Op::AddString { lang: 0, dotted: "MyMod_Text.MyKey".into(), text: "hello".into() },
            },
            Change {
                target: "tmpl 7 pair 2".into(),
                before: "1".into(),
                after: "2".into(),
                op: Op::SetPair { entry: 7, pair: 2, bytes: vec![2, 0, 0, 0] },
            },
            Change {
                target: "tex MyIcon".into(),
                before: String::new(),
                after: "MyIcon".into(),
                op: Op::ReserveTexture { name: "MyIcon".into() },
            },
        ];
        // save_to must create the intermediate directories, not fail on them.
        ModDoc::save_to(&path, "Test mod", &changes).expect("save");

        let doc = ModDoc::load_from(&path);
        assert_eq!(doc.name, "Test mod");
        assert_eq!(doc.changes.len(), 4);
        assert_eq!(doc.changes[0].before, "CHECKPOINT");
        assert!(matches!(doc.changes[0].op, Op::SetString { asset_id: 0x1111_1111, lang: 0, .. }));
        assert!(matches!(&doc.changes[1].op, Op::AddString { dotted, .. } if dotted == "MyMod_Text.MyKey"));
        assert!(matches!(&doc.changes[2].op, Op::SetPair { entry: 7, pair: 2, bytes } if bytes == &[2,0,0,0]));
        assert!(matches!(&doc.changes[3].op, Op::ReserveTexture { name } if name == "MyIcon"));

        // A corrupt document opens the app empty rather than not at all.
        std::fs::write(&path, "{ not json").expect("clobber");
        assert!(ModDoc::load_from(&path).changes.is_empty());
        // As does one that was never written.
        assert!(ModDoc::load_from(&dir.join("dlc-99.json")).changes.is_empty());
    }

    /// Slot, not name: renaming a mod must not start a fresh empty document.
    #[test]
    fn the_document_is_keyed_by_slot() {
        assert_eq!(ModDoc::path("02"), ModDoc::path("02"));
        assert_ne!(ModDoc::path("02"), ModDoc::path("03"));
        assert!(ModDoc::path("02").ends_with("dlc-02.json"));
        // A hand-edited slot must not escape the mods directory.
        assert!(ModDoc::path("../../evil").ends_with("dlc-evil.json"));
    }

    /// A freshly loaded language arrives as retail. The staged edits have to go back on, or the mod
    /// is invisible in the very window that is editing it — which is what a language switch and
    /// every new session looked like.
    #[test]
    fn a_reload_gets_the_staged_edits_put_back() {
        let Some((root, game)) = staged_install("reapply") else { return };
        let mut ed = Editor::new(&game, 0, "02", "Test mod");
        // Editor::new restores the real mod document for this slot; a test must not read
        // (or assert against) whatever the person running it happens to have staged.
        ed.changes.clear();

        let retail = std::fs::read(root.join("Cinematics/Dialog/English/GameText.dlg")).expect("read");
        let gt = GameText::parse(&retail).expect("parse");
        let id = gt.records[0].asset_id;
        let was = gt.records[0].text_string();

        ed.changes.push(Change {
            target: format!("EN 0x{id:08X}"),
            before: was.clone(),
            after: "REAPPLIED".into(),
            op: Op::SetString { lang: 0, asset_id: id, scene: None, text: "REAPPLIED".into() },
        });

        // Exactly what `pump` sees when a load lands: a pristine parse of the source.
        ed.str_load = Load::Ready(GameText::parse(&retail).expect("parse"));
        ed.reapply_strings(0);
        assert_eq!(
            ed.str_load.ready().unwrap().find(id).map(|r| r.text_string()),
            Some("REAPPLIED".into())
        );
        // ...but the changelist still knows what retail said, so the diff stays honest.
        assert_eq!(ed.original_text(StrSel { asset_id: id, scene: None }), Some(was));

        // A change staged for another language must not bleed into this one.
        ed.str_load = Load::Ready(GameText::parse(&retail).expect("parse"));
        ed.reapply_strings(1);
        assert_ne!(
            ed.str_load.ready().unwrap().find(id).map(|r| r.text_string()),
            Some("REAPPLIED".into())
        );
    }

    /// The Objects page must read the GAME's object DB, not the DLC's patch table.
    ///
    /// `DLC/01/GameTemplates.wsd` holds five templates — the whole Midnight Show patch and none of
    /// the game — and the page shipped pointed at it. The real DB is an `AULB` blob inside
    /// `France/loosefiles_BinPC.pack`. Pinned with a floor rather than an exact count so it survives
    /// a differently-cooked install while still catching a regression to the DLC file.
    #[test]
    fn objects_read_the_full_template_db() {
        let Some(game) = install() else { return };
        let full = crate::assets::load_gametemplates(&game).expect("main GameTemplates DB");
        let n = full.templates().count();
        eprintln!("loaded {n} templates from loosefiles_BinPC.pack");
        assert!(n > 1000, "expected the game's DB, got {n} templates");

        // ...and the DLC's file is the thing it must NOT be.
        let dlc = std::fs::read(format!("{game}/DLC/01/GameTemplates.wsd")).expect("dlc templates");
        let (dlc, _) = GameTemplates::parse(&dlc).expect("parse dlc");
        assert_eq!(dlc.templates().count(), 5, "retail's DLC patch table");
        assert!(n > dlc.templates().count() * 100);
    }

    /// A document written before the Objects page changed source carries `SetPair` indices into a
    /// five-entry table. Re-pointing them at a 10,761-entry one would edit an unrelated template, so
    /// they are dropped — and nothing else in the document is.
    #[test]
    fn a_v1_document_loses_only_its_object_edits() {
        let path = scratch("v1doc").join("dlc-02.json");
        std::fs::write(
            &path,
            r#"{"name":"Old mod","changes":[
                {"target":"EN 0x1","before":"a","after":"b",
                 "op":{"SetString":{"lang":0,"asset_id":1,"text":"b"}}},
                {"target":"tmpl 3 pair 1","before":"1","after":"2",
                 "op":{"SetPair":{"entry":3,"pair":1,"bytes":[2,0,0,0]}}}
            ]}"#,
        )
        .expect("write v1");

        // Straight load keeps everything — the migration is what drops it.
        let raw = ModDoc::load_from(&path);
        assert_eq!(raw.version, 0, "a v1 file has no version field");
        assert_eq!(raw.changes.len(), 2);

        let migrated: Vec<Change> = raw
            .changes
            .into_iter()
            .filter(|c| !matches!(c.op, Op::SetPair { .. }))
            .collect();
        assert_eq!(migrated.len(), 1);
        assert!(matches!(migrated[0].op, Op::SetString { .. }));

        // What we write from here on is stamped, so this only ever happens once.
        ModDoc::save_to(&path, "Old mod", &migrated).expect("save");
        assert_eq!(ModDoc::load_from(&path).version, ModDoc::CURRENT);
    }

    #[test]
    fn case_insensitive_contains_matches_the_allocating_version() {
        for (hay, needle) in [
            ("UsePt_Brothel", "brothel"),
            ("CP_Burlesque_Stool", "stool"),
            ("Teleporter", "teleporter"),
            ("Teleporter", ""),
            ("Teleporter", "port"),
            ("Teleporter", "xyz"),
            ("short", "muchlongerneedle"),
        ] {
            assert_eq!(
                contains_ci(hay, needle),
                hay.to_ascii_lowercase().contains(needle),
                "{hay:?} vs {needle:?}"
            );
        }
    }

    /// The xref dedups by entry — a template naming the same hash in two pairs is listed once.
    #[test]
    fn xref_lists_each_template_once_per_value() {
        let mut gt = GameTemplates { entries: Vec::new() };
        gt.entries.push(Entry::Template(sab_formats::gametemplates::Template {
            unk1: 0,
            unk2: 1,
            name: "A".into(),
            ttype: "Prop".into(),
            pairs: vec![
                sab_formats::gametemplates::Pair { hash: 1, data: 7u32.to_le_bytes().to_vec() },
                sab_formats::gametemplates::Pair { hash: 2, data: 7u32.to_le_bytes().to_vec() },
            ],
        }));
        assert_eq!(build_xref(&gt).get(&7), Some(&vec![0usize]));
    }

    /// The list rebuild and the xref are what made the page struggle once it had the real DB behind
    /// it. Generous bounds — this is a guard against a return to per-template allocation, not a
    /// benchmark. Run under `--release` for a meaningful number; debug is ~10× slower.
    #[test]
    fn building_the_object_list_is_not_slow() {
        let Some(game) = install() else { return };
        let gt = crate::assets::load_gametemplates(&game).expect("templates");

        let t0 = std::time::Instant::now();
        let xref = build_xref(&gt);
        let xref_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let needle = "usept";
        let t1 = std::time::Instant::now();
        let mut hits = 0usize;
        for (_, t) in gt.templates() {
            if contains_ci(&t.name, needle) || contains_ci(&t.ttype, needle) {
                hits += 1;
            }
        }
        let filter_ms = t1.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "xref {} keys in {xref_ms:.1} ms; filtered {hits} of {} in {filter_ms:.2} ms",
            xref.len(),
            gt.templates().count()
        );
        assert!(filter_ms < 100.0, "a filter pass must not cost a frame: {filter_ms} ms");
    }

    /// A stripped install has no DLC/01 to carry; publishing must still succeed.
    #[test]
    fn mirror_of_a_missing_retail_dlc_is_not_an_error() {
        let root = scratch("nodlc");
        let missing = root.join("DLC").join("01").to_string_lossy().to_string();
        let to = root.join("DLC").join("02").to_string_lossy().to_string();
        assert_eq!(mirror_dlc(&missing, &to, &[]), Ok((0, 0)));
    }
}
