//! egui host: a hand-rolled winit-0.29 -> egui event bridge plus the egui-wgpu paint path.
//!
//! ADAPTED from the Mercs2 workshop's `gui.rs`. Hand-rolled because `egui-winit` 0.28 targets
//! winit 0.30 while we are on 0.29. Carries the Mercs2 rewrite's two bridge upgrades — OS clipboard
//! delivery (for the inspector's copy actions) and cursor-icon delivery (hand over widgets, I-beam
//! over text) — plus the shared `theme` visual system.

use std::sync::Arc;

use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

pub struct Gui {
    pub ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
    pointer: egui::Pos2,
    ppp: f32,
    size: [u32; 2],
    jobs: Vec<egui::ClippedPrimitive>,
    tex_delta: egui::TexturesDelta,
    start: std::time::Instant,
    /// OS clipboard (lazy): egui only EMITS copied text via `PlatformOutput`; the integration must
    /// deliver it — this is what makes the inspector's "Copy …" actions real.
    clipboard: Option<arboard::Clipboard>,
    /// The window — the integration must deliver `PlatformOutput.cursor_icon` to it (egui only
    /// EMITS the desired cursor). Drives the hand cursor over buttons / I-beam over the search box.
    window: Arc<Window>,
    /// The cursor egui last requested, so we only call `set_cursor` when it changes.
    cursor: egui::CursorIcon,
}

impl Gui {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, window: &Arc<Window>) -> Gui {
        let ctx = egui::Context::default();
        // The workshop's "field-workbench" identity: warm gunmetal, brass = live/selected,
        // hazard-orange = irreversible. See the `theme` module below.
        theme::install(&ctx);
        let size = window.inner_size();
        Gui {
            ctx,
            renderer: egui_wgpu::Renderer::new(device, format, None, 1),
            events: Vec::new(),
            modifiers: egui::Modifiers::default(),
            pointer: egui::Pos2::ZERO,
            ppp: window.scale_factor() as f32,
            size: [size.width, size.height],
            jobs: Vec::new(),
            tex_delta: egui::TexturesDelta::default(),
            start: std::time::Instant::now(),
            clipboard: None,
            window: window.clone(),
            cursor: egui::CursorIcon::Default,
        }
    }

    /// Feed a winit event. Returns true when egui CONSUMED it (pointer over a panel, text into a
    /// widget) — the caller then skips its own camera/shortcut handling.
    pub fn on_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::Resized(s) => {
                self.size = [s.width, s.height];
                false
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.ppp = *scale_factor as f32;
                false
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.modifiers = egui::Modifiers {
                    alt: s.alt_key(),
                    ctrl: s.control_key(),
                    shift: s.shift_key(),
                    mac_cmd: false,
                    command: s.control_key(),
                };
                false
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = egui::pos2(position.x as f32 / self.ppp, position.y as f32 / self.ppp);
                self.events.push(egui::Event::PointerMoved(self.pointer));
                self.ctx.is_using_pointer()
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => egui::PointerButton::Primary,
                    MouseButton::Right => egui::PointerButton::Secondary,
                    MouseButton::Middle => egui::PointerButton::Middle,
                    _ => return false,
                };
                let pressed = *state == ElementState::Pressed;
                self.events.push(egui::Event::PointerButton {
                    pos: self.pointer,
                    button,
                    pressed,
                    modifiers: self.modifiers,
                });
                self.ctx.is_pointer_over_area() || self.ctx.wants_pointer_input()
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (unit, d) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (egui::MouseWheelUnit::Line, egui::vec2(*x, *y))
                    }
                    MouseScrollDelta::PixelDelta(p) => (
                        egui::MouseWheelUnit::Point,
                        egui::vec2(p.x as f32 / self.ppp, p.y as f32 / self.ppp),
                    ),
                };
                self.events.push(egui::Event::MouseWheel { unit, delta: d, modifiers: self.modifiers });
                self.ctx.is_pointer_over_area() || self.ctx.wants_pointer_input()
            }
            WindowEvent::KeyboardInput {
                event: KeyEvent { physical_key: PhysicalKey::Code(code), state, text, repeat, .. },
                ..
            } => {
                if let Some(key) = map_key(*code) {
                    self.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: *state == ElementState::Pressed,
                        repeat: *repeat,
                        modifiers: self.modifiers,
                    });
                }
                if *state == ElementState::Pressed && self.ctx.wants_keyboard_input() {
                    if let Some(t) = text {
                        let printable: String = t.chars().filter(|c| !c.is_control()).collect();
                        if !printable.is_empty() {
                            self.events.push(egui::Event::Text(printable));
                        }
                    }
                }
                self.ctx.wants_keyboard_input()
            }
            _ => false,
        }
    }

    /// Run one GUI frame: `build` lays out the panels; paint jobs are stashed for `paint`.
    pub fn run(&mut self, build: impl FnOnce(&egui::Context)) {
        let screen = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(self.size[0] as f32, self.size[1] as f32) / self.ppp,
        );
        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(self.start.elapsed().as_secs_f64()),
            modifiers: self.modifiers,
            events: std::mem::take(&mut self.events),
            focused: true,
            ..Default::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.ppp);
        let out = self.ctx.run(raw, build);
        // Deliver copy actions (context menus, Ctrl+C in text fields) to the OS clipboard.
        if !out.platform_output.copied_text.is_empty() {
            if self.clipboard.is_none() {
                self.clipboard = arboard::Clipboard::new()
                    .map_err(|e| eprintln!("[gui] clipboard unavailable: {e}"))
                    .ok();
            }
            if let Some(cb) = &mut self.clipboard {
                if let Err(e) = cb.set_text(out.platform_output.copied_text.clone()) {
                    eprintln!("[gui] clipboard write failed: {e}");
                }
            }
        }
        // Deliver the cursor: egui sets I-beam over text etc.; where it leaves Default but the
        // pointer is over an interactive widget, show a hand so clickable elements read as clickable.
        let mut want = out.platform_output.cursor_icon;
        if want == egui::CursorIcon::Default
            && self.ctx.wants_pointer_input()
            && !self.ctx.wants_keyboard_input()
        {
            want = egui::CursorIcon::PointingHand;
        }
        if want != self.cursor {
            self.cursor = want;
            self.window.set_cursor_icon(to_winit_cursor(want));
        }
        self.jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        self.tex_delta = out.textures_delta;
    }

    /// Paint the last `run` onto the swapchain view (its own render pass, load = keep).
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: [u32; 2],
    ) {
        for (id, delta) in &self.tex_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let desc = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: self.ppp };
        self.renderer.update_buffers(device, queue, encoder, &self.jobs, &desc);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer.render(&mut pass, &self.jobs, &desc);
        }
        for id in &self.tex_delta.free {
            self.renderer.free_texture(id);
        }
        self.tex_delta = egui::TexturesDelta::default();
    }
}

/// egui → winit cursor icon (the subset the tool produces; anything else falls back to the arrow).
fn to_winit_cursor(c: egui::CursorIcon) -> winit::window::CursorIcon {
    use egui::CursorIcon as E;
    use winit::window::CursorIcon as W;
    match c {
        E::PointingHand => W::Pointer,
        E::Text | E::VerticalText => W::Text,
        E::Crosshair => W::Crosshair,
        E::Move => W::Move,
        E::Grab => W::Grab,
        E::Grabbing => W::Grabbing,
        E::NotAllowed | E::NoDrop => W::NotAllowed,
        E::Wait => W::Wait,
        E::Progress => W::Progress,
        E::Help => W::Help,
        E::ResizeHorizontal | E::ResizeEast | E::ResizeWest => W::EwResize,
        E::ResizeVertical | E::ResizeNorth | E::ResizeSouth => W::NsResize,
        E::ResizeNeSw | E::ResizeNorthEast | E::ResizeSouthWest => W::NeswResize,
        E::ResizeNwSe | E::ResizeNorthWest | E::ResizeSouthEast => W::NwseResize,
        E::ResizeColumn => W::ColResize,
        E::ResizeRow => W::RowResize,
        _ => W::Default,
    }
}

/// The workshop's visual system — "Occupied": palette + type + the reusable inspector widgets, so
/// every panel reads as one tool.
///
/// The theme takes its cue from the game's **Will to Fight**: an occupied district is desaturated
/// and colour floods back as it is freed. So the chrome is TRUE-NEUTRAL greyscale (S=0) and colour
/// is *earned* by state the tool already tracks — a submesh with no texture bound is simply grey,
/// one with a texture is [`EMBER`]. That is a resting state, not a mode: nothing toggles it.
///
/// Roles — and there are only three hues, because a black-and-white city only gets one:
/// - [`RED`]    live / selected — and, in a different *form* (see [`stamp_button`]), irreversible,
///              and, outlined, an error. Red is the only colour in occupied Paris.
/// - [`EMBER`]  resolved / bound / ready — colour returning.
/// - [`COLD`]   source / reference / read-only.
///
/// Danger is deliberately NOT a hue (the old hazard-orange retired with the Mercs2 palette): with
/// one hot colour spent on selection there is none left for "irreversible", so that became a
/// printed form — see [`stamp_button`].
///
/// Adapted from the Mercs2 workshop's `gui::theme`, but the neutrals are re-based: Mercs2's greys
/// lean warm to sit under brass, and that warm bias was the single loudest tell that this tool was
/// wearing another game's clothes.
#[allow(dead_code)] // a complete token+widget set; not every item is wired into the UI yet
pub mod theme {
    use egui::{Color32, FontFamily, FontId, Rounding, Stroke, TextStyle};

    // ── palette (film base / newsprint — true neutral, S=0) ──
    pub const G0: Color32 = Color32::from_rgb(0x0a, 0x0b, 0x0c); // app ground
    pub const G1: Color32 = Color32::from_rgb(0x13, 0x14, 0x17); // panels
    pub const G2: Color32 = Color32::from_rgb(0x1a, 0x1c, 0x1f); // cards / inputs
    pub const G3: Color32 = Color32::from_rgb(0x24, 0x26, 0x2a); // raised / hover
    pub const LINE: Color32 = Color32::from_rgb(0x30, 0x33, 0x38);
    pub const LINE2: Color32 = Color32::from_rgb(0x43, 0x46, 0x4c);
    // Text ramp. Measured against the panel (`G1`) and card (`G2`) grounds, because a value that
    // only looks right on one of them fails on the other:
    //
    //   TX     13.8:1   primary
    //   DIM     6.7:1   secondary
    //   FAINT   4.8:1   tertiary  (was 2.5:1 — a hard WCAG-AA fail)
    //
    // FAINT was the real problem: it carried the hints, hashes and counts — the data this tool
    // exists to show — at 8-10 px, where small type needs MORE contrast, not less. A "quiet" colour
    // is only quiet if it can still be read.
    pub const TX: Color32 = Color32::from_rgb(0xe9, 0xe7, 0xe2); // newsprint white
    pub const DIM: Color32 = Color32::from_rgb(0xa5, 0xa2, 0x9c);
    pub const FAINT: Color32 = Color32::from_rgb(0x8a, 0x87, 0x7f);

    // ── semantic accents ──
    /// Live / selected. Also errors (outlined) and irreversible ([`stamp_button`], filled).
    /// Brightened from #d42a34 (3.4:1 — fine as a fill, too dark as TEXT) to clear AA at 4.6:1,
    /// since selected rows and error lines are read, not just noticed.
    pub const RED: Color32 = Color32::from_rgb(0xeb, 0x4b, 0x53);
    pub const RED_DK: Color32 = Color32::from_rgb(0x8c, 0x1a, 0x22);
    pub const RED_SOFT: Color32 = Color32::from_rgb(0x2a, 0x11, 0x14);
    /// Resolved / bound / ready — the colour that comes back.
    pub const EMBER: Color32 = Color32::from_rgb(0xd9, 0x8f, 0x3d);
    pub const EMBER_DK: Color32 = Color32::from_rgb(0x8a, 0x56, 0x20);
    pub const EMBER_SOFT: Color32 = Color32::from_rgb(0x24, 0x1a, 0x10);
    /// Source / reference / read-only — cold window light.
    pub const COLD: Color32 = Color32::from_rgb(0x6e, 0x8f, 0xa6);
    pub const COLD_DK: Color32 = Color32::from_rgb(0x3d, 0x56, 0x66);
    pub const COLD_SOFT: Color32 = Color32::from_rgb(0x14, 0x1b, 0x20);
    /// Ink — text ON a filled [`RED`] or [`EMBER`] ground (a stamp, a primary button).
    pub const INK: Color32 = Color32::from_rgb(0x12, 0x08, 0x0a);

    /// The label display family — Oswald Medium, at ≤13px: eyebrows, chips, pills, buttons.
    /// Bundled (see [`install`]), so it resolves on any machine.
    pub fn disp() -> FontFamily {
        FontFamily::Name("disp".into())
    }

    /// The poster display family — Oswald SemiBold, at ≥18px: titles, the brand, the boot screen.
    /// The extra weight is what carries an occupation-notice headline; at 11px it would just clog.
    pub fn poster() -> FontFamily {
        FontFamily::Name("poster".into())
    }

    /// The data family — Courier New. Every number this tool prints is evidence about an occupied
    /// city: hashes, bone counts, durations, byte sizes. They all get the typewriter.
    pub fn data() -> FontFamily {
        FontFamily::Monospace
    }

    /// The corner cut, in points, for [`card`] / [`section`] — Deco, printed, not rounded.
    pub const CUT: f32 = 7.0;

    /// A chamfered rect: the card silhouette, with the top-left and bottom-right corners cut.
    /// egui's `Rounding` only does radii, so the frames paint this polygon themselves.
    pub fn chamfer(r: egui::Rect, cut: f32) -> Vec<egui::Pos2> {
        let c = cut.min(r.width() * 0.5).min(r.height() * 0.5).max(0.0);
        vec![
            egui::pos2(r.left() + c, r.top()),
            egui::pos2(r.right(), r.top()),
            egui::pos2(r.right(), r.bottom() - c),
            egui::pos2(r.right() - c, r.bottom()),
            egui::pos2(r.left(), r.bottom()),
            egui::pos2(r.left(), r.top() + c),
        ]
    }

    /// Oswald, instanced from the upstream variable font at a fixed weight (egui's glyph backend
    /// ignores `fvar`, so a variable TTF would silently render at wght=400). SIL OFL 1.1 — no
    /// Reserved Font Name, so the instances keep the name; see `assets/fonts/OFL.txt`.
    /// Embedded rather than read from disk: the old `C:/Windows/Fonts` reads worked only here.
    const OSWALD_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Oswald-Medium.ttf");
    const OSWALD_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Oswald-SemiBold.ttf");

    /// Every non-ASCII glyph the UI paints, in ONE place.
    ///
    /// A symbol only renders if some font in its family's stack has that codepoint; there is no
    /// substitution and no warning — a missing one paints an empty box, silently, forever. That is
    /// how `▤`/`▦` (rail) and `✓`/`✕` (checklist, clear buttons) shipped as tofu: they look like
    /// obvious characters, but NOTHING in the stack draws them. Segoe UI carries almost no dingbats,
    /// and neither Oswald nor egui's Ubuntu-Light carries any — in practice every symbol comes from
    /// one of the two bundled fallbacks, `NotoEmoji-Regular` and `emoji-icon-font`.
    ///
    /// So: every symbol lives here, annotated with the font that actually draws it, and
    /// `tests::glyphs_are_covered` fails the build if one is not in a BUNDLED font. Bundled-only is
    /// deliberate — leaning on Segoe UI would pass here and tofu on a machine without it.
    pub mod sym {
        /// Rail: the character/animation viewer.        (emoji-icon-font)
        pub const INSPECT: &str = "◎";
        /// Rail: GameText.dlg — the pilcrow, for prose. (Segoe/Ubuntu/Oswald — a real letterform)
        pub const STRINGS: &str = "¶";
        /// Rail: GameTemplates — stacked rows, a list.  (emoji-icon-font)
        pub const OBJECTS: &str = "☰";
        /// Rail + "Pick" button: the texture pool.      (emoji-icon-font)
        pub const ICONS: &str = "🖼";
        /// Rail: app configuration.                     (emoji-icon-font)
        pub const SETTINGS: &str = "⚙";

        /// Checklist: requirement met.                  (NotoEmoji + emoji-icon-font)
        pub const OK: &str = "✔";
        /// Checklist: required and missing; also every clear/dismiss button.
        ///                                              (NotoEmoji + emoji-icon-font)
        pub const NO: &str = "✖";
        /// Checklist: optional and missing — not a failure, so an en dash, not a mark.
        pub const NA: &str = "–";

        /// Transport.                                   (NotoEmoji + emoji-icon-font)
        pub const PLAY: &str = "▶";
        ///                                              (emoji-icon-font)
        pub const PAUSE: &str = "⏸";
        /// Revert one staged edit.                      (emoji-icon-font)
        pub const UNDO: &str = "↺";

        /// Everything above, for the coverage test.
        #[cfg(test)]
        pub const ALL: &[(&str, &str)] = &[
            ("INSPECT", INSPECT), ("STRINGS", STRINGS), ("OBJECTS", OBJECTS),
            ("ICONS", ICONS), ("SETTINGS", SETTINGS), ("OK", OK), ("NO", NO),
            ("NA", NA), ("PLAY", PLAY), ("PAUSE", PAUSE), ("UNDO", UNDO),
        ];
    }

    fn load_font(defs: &mut egui::FontDefinitions, key: &str, paths: &[&str]) -> bool {
        for p in paths {
            if let Ok(bytes) = std::fs::read(p) {
                defs.font_data.insert(key.to_owned(), egui::FontData::from_owned(bytes));
                return true;
            }
        }
        false
    }

    pub fn install(ctx: &egui::Context) {
        // ── fonts ──
        let mut fonts = egui::FontDefinitions::default();
        // Body: prefer Segoe UI (the native Windows UI face) ahead of egui's default proportional.
        if load_font(&mut fonts, "segoe", &["C:/Windows/Fonts/segoeui.ttf"]) {
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".to_owned());
        }
        // Data: Courier New — the typewriter voice for every number the tool prints. Bold, because
        // Courier Regular at 10-11px on a near-black ground is too thin to read.
        if load_font(&mut fonts, "courier", &["C:/Windows/Fonts/courbd.ttf", "C:/Windows/Fonts/cour.ttf"]) {
            fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "courier".to_owned());
        }
        // Display: bundled Oswald. The proportional stack is appended as a FALLBACK so glyphs
        // Oswald lacks (▶ ⏸ ↺ … and the rail marks) still resolve instead of rendering tofu.
        // NOTE: the tail of that stack is egui's own Ubuntu-Light + NotoEmoji + emoji-icon-font, and
        // those two emoji faces are where essentially every symbol comes from — Segoe UI carries
        // almost no dingbats. See `sym` for the glyphs this is allowed to draw.
        let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        for (key, bytes, family) in [
            ("oswald_md", OSWALD_MEDIUM, "disp"),
            ("oswald_sb", OSWALD_SEMIBOLD, "poster"),
        ] {
            fonts.font_data.insert(key.to_owned(), egui::FontData::from_static(bytes));
            // Oswald sits high in its em box next to Segoe; nudge it onto the same baseline.
            if let Some(fd) = fonts.font_data.get_mut(key) {
                fd.tweak.y_offset_factor = 0.06;
            }
            let mut stack = vec![key.to_owned()];
            stack.extend(fallback.iter().cloned());
            fonts.families.insert(FontFamily::Name(family.into()), stack);
        }
        ctx.set_fonts(fonts);

        // ── type scale + visuals ──
        let mut style = (*ctx.style()).clone();
        // Same scale as the text helpers, so egui's own widgets (buttons, text fields, tooltips)
        // never drift out of step with the theme's labels.
        let sc = |pt: f32| pt * type_scale();
        style.text_styles.insert(TextStyle::Heading, FontId::new(sc(20.0), poster()));
        style.text_styles.insert(TextStyle::Body, FontId::new(sc(13.0), FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Button, FontId::new(sc(13.0), FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Small, FontId::new(sc(11.0), FontFamily::Proportional));
        style.text_styles.insert(TextStyle::Monospace, FontId::new(sc(11.5), FontFamily::Monospace));

        let mut v = egui::Visuals::dark();
        v.panel_fill = G1;
        v.window_fill = G2;
        v.window_stroke = Stroke::new(1.0, LINE2);
        v.extreme_bg_color = G0;
        v.faint_bg_color = G2;
        v.override_text_color = Some(TX);
        v.hyperlink_color = RED;
        v.selection.bg_fill = RED_SOFT;
        v.selection.stroke = Stroke::new(1.0, RED);
        // Printed edges, not soft ones: widgets are square, and the cards cut their own corners
        // (see `chamfer`) rather than rounding them.
        v.window_rounding = Rounding::ZERO;
        let round = Rounding::ZERO;
        v.widgets.noninteractive.bg_fill = G1;
        v.widgets.noninteractive.weak_bg_fill = G1;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, DIM);
        v.widgets.noninteractive.rounding = round;
        v.widgets.inactive.bg_fill = G2;
        v.widgets.inactive.weak_bg_fill = G2;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TX);
        v.widgets.inactive.rounding = round;
        v.widgets.hovered.bg_fill = G3;
        v.widgets.hovered.weak_bg_fill = G3;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, LINE2);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, TX);
        v.widgets.hovered.rounding = round;
        v.widgets.active.bg_fill = G3;
        v.widgets.active.weak_bg_fill = G3;
        v.widgets.active.bg_stroke = Stroke::new(1.0, RED_DK);
        v.widgets.active.fg_stroke = Stroke::new(1.0, RED);
        v.widgets.active.rounding = round;
        v.widgets.open.bg_fill = G2;
        v.widgets.open.weak_bg_fill = G2;
        v.widgets.open.bg_stroke = Stroke::new(1.0, LINE);
        v.widgets.open.rounding = round;
        style.visuals = v;

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        style.spacing.window_margin = egui::Margin::same(10.0);
        style.spacing.menu_margin = egui::Margin::same(6.0);
        ctx.set_style(style);
    }

    /// Global type scale. Every size in the theme — the `TextStyle` scale in [`install`] and the
    /// three text helpers — is multiplied by this, so the whole system grows together and the
    /// hierarchy between voices is preserved. Change this, not individual call sites.
    ///
    /// Runtime-settable from the Settings page. It is read on nearly every widget built, so it lives
    /// in an atomic rather than behind a lock: reads must never contend, and a torn value is not
    /// possible for a 32-bit load. Stored as `f32::to_bits` since there is no `AtomicF32`.
    pub const DEFAULT_TYPE_SCALE: f32 = 1.2;
    static TYPE_SCALE_BITS: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0x3F99_999A); // 1.2f32

    pub fn type_scale() -> f32 {
        f32::from_bits(TYPE_SCALE_BITS.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Set the global type scale. Callers must follow with [`install`] so the `TextStyle` table —
    /// which egui caches rather than re-reading per frame — picks the new sizes up too.
    pub fn set_type_scale(v: f32) {
        TYPE_SCALE_BITS
            .store(v.clamp(0.8, 2.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    // Type floors, in PRE-scale points. Enforced here rather than trusted to call sites, because
    // the sizes that crept down to 8px were all written inline at the point of use and no reviewer
    // catches that by eye. Tracked uppercase needs the most room to stay legible; Courier the least.
    const MIN_DISP: f32 = 10.5;
    const MIN_DATA: f32 = 10.0;
    const MIN_POSTER: f32 = 15.0;

    /// Clamp to the voice's floor, then apply the global scale.
    fn sized(size: f32, floor: f32) -> f32 {
        size.max(floor) * type_scale()
    }

    /// The LABEL voice: Oswald Medium, tracked. UI chrome that names things — eyebrows, chips,
    /// pills, column heads. Tracking scales with size so the whole set reads as one system.
    pub fn disp_text(text: impl Into<String>, size: f32, color: Color32) -> egui::RichText {
        let size = sized(size, MIN_DISP);
        egui::RichText::new(text.into())
            .family(disp())
            .size(size)
            .color(color)
            .extra_letter_spacing(size * 0.11)
    }

    /// The POSTER voice: Oswald SemiBold. Titles and the brand — an occupation notice, not a
    /// caption. Too heavy to use small, hence the floor.
    pub fn poster_text(text: impl Into<String>, size: f32, color: Color32) -> egui::RichText {
        let size = sized(size, MIN_POSTER);
        egui::RichText::new(text.into())
            .family(poster())
            .size(size)
            .color(color)
            .extra_letter_spacing(size * 0.04)
    }

    /// The DATA voice: Courier. Anything the tool *found* rather than anything it says — asset
    /// names, hashes, counts, durations, byte sizes. Never uppercased: these are verbatim.
    pub fn data_text(text: impl Into<String>, size: f32, color: Color32) -> egui::RichText {
        egui::RichText::new(text.into()).family(data()).size(sized(size, MIN_DATA)).color(color)
    }

    /// A small red diamond — the mark that opens an eyebrow.
    fn tick(ui: &mut egui::Ui, color: Color32) {
        let (r, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        let c = r.center();
        let d = 3.5;
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                c + egui::vec2(0.0, -d),
                c + egui::vec2(d, 0.0),
                c + egui::vec2(0.0, d),
                c + egui::vec2(-d, 0.0),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }

    /// An eyebrow label (red diamond + tracked uppercase, dim) — the section-header voice.
    pub fn eyebrow(ui: &mut egui::Ui, text: &str) -> egui::Response {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            tick(ui, RED);
            ui.add(egui::Label::new(disp_text(text.to_uppercase(), 11.0, DIM)))
        })
        .inner
    }

    /// A HUD chip drawn over the viewport (Orbit / clip position / legend). `on` = lit ember,
    /// i.e. this facet is resolved; off = the resting grey.
    pub fn chip(ui: &mut egui::Ui, label: &str, on: bool, dot: Option<Color32>) {
        let (fg, bg, stroke) = if on {
            (EMBER, EMBER_SOFT, EMBER_DK)
        } else {
            (DIM, Color32::from_rgba_unmultiplied(10, 11, 12, 205), LINE)
        };
        egui::Frame::none()
            .fill(bg)
            .stroke(egui::Stroke::new(1.0, stroke))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::symmetric(9.0, 4.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    if let Some(c) = dot {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, egui::Rounding::same(1.0), c);
                    }
                    ui.label(disp_text(label.to_uppercase(), 9.5, fg));
                });
            });
    }

    /// A framed inspector card: a chamfered panel with a tracked header (title + optional
    /// right-aligned badge) and the body below. The defining inspector element.
    ///
    /// The corners are CUT, not rounded — `egui::Rounding` can't express that, so the card
    /// reserves a shape slot up front and paints its own silhouette once the content has told it
    /// how big it ended up.
    pub fn card<R>(
        ui: &mut egui::Ui,
        title: &str,
        badge: Option<&str>,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> R {
        let bg = ui.painter().add(egui::Shape::Noop);
        let ir = egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(11.0, 9.0))
            .outer_margin(egui::Margin { bottom: 10.0, ..Default::default() })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(disp_text(title.to_uppercase(), 11.5, DIM));
                    if let Some(b) = badge {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(data_text(b, 10.0, FAINT));
                        });
                    }
                });
                ui.add_space(3.0);
                ui.separator();
                ui.add_space(5.0);
                add(ui)
            });
        ui.painter().set(
            bg,
            egui::Shape::convex_polygon(
                chamfer(ir.response.rect, CUT),
                G2,
                egui::Stroke::new(1.0, LINE),
            ),
        );
        ir.inner
    }

    /// A COLLAPSIBLE framed inspector section (the `card()` look + an expand/collapse toggle).
    /// `title` must be STATIC (it is the persistence key); put dynamic counts in `badge`.
    pub fn section(
        ui: &mut egui::Ui,
        title: &str,
        badge: Option<&str>,
        default_open: bool,
        add: impl FnOnce(&mut egui::Ui),
    ) {
        let bg = ui.painter().add(egui::Shape::Noop);
        let ir = egui::Frame::none()
            .inner_margin(egui::Margin::symmetric(11.0, 8.0))
            .outer_margin(egui::Margin { bottom: 10.0, ..Default::default() })
            .show(ui, |ui| {
                let id = ui.make_persistent_id(("sect", title));
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open)
                    .show_header(ui, |ui| {
                        ui.label(disp_text(title.to_uppercase(), 11.5, DIM));
                        if let Some(b) = badge {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(data_text(b, 10.0, FAINT));
                            });
                        }
                    })
                    .body(|ui| {
                        ui.add_space(5.0);
                        add(ui);
                    });
            });
        ui.painter().set(
            bg,
            egui::Shape::convex_polygon(
                chamfer(ir.response.rect, CUT),
                G2,
                egui::Stroke::new(1.0, LINE),
            ),
        );
    }

    /// A full-width framed, clickable row (the clip / material chip). `fill`/`border` carry the
    /// state colour — ember = resolved, red = selected, neutral = still occupied. Returns the
    /// row's click response.
    pub fn row_chip<R>(
        ui: &mut egui::Ui,
        fill: egui::Color32,
        border: egui::Color32,
        add: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::Response {
        let ir = egui::Frame::none()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, border))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::symmetric(9.0, 5.0))
            .outer_margin(egui::Margin { bottom: 4.0, ..Default::default() })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                });
            });
        ir.response.interact(egui::Sense::click())
    }

    /// A small toggle pill. Red when `on` — a pill selects, it does not resolve.
    pub fn pill(ui: &mut egui::Ui, label: &str, on: bool) -> egui::Response {
        let (fill, stroke, txt) = if on { (RED_SOFT, RED_DK, RED) } else { (G0, LINE, DIM) };
        egui::Frame::none()
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::symmetric(9.0, 3.0))
            .outer_margin(egui::Margin { right: 4.0, bottom: 4.0, ..Default::default() })
            .show(ui, |ui| {
                ui.label(disp_text(label.to_uppercase(), 10.5, txt));
            })
            .response
            .interact(egui::Sense::click())
    }

    /// A key → value row inside a card body: tracked label left, tabular Courier value right.
    pub fn kv(ui: &mut egui::Ui, key: &str, value: egui::RichText) {
        ui.horizontal(|ui| {
            ui.label(disp_text(key.to_uppercase(), 11.0, DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(value.family(data()).size(11.5));
            });
        });
    }

    /// A filled red "go" button — the live action on this page. Dimmed when disabled.
    pub fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
        let fg = if enabled { INK } else { FAINT };
        let bg = if enabled { RED } else { G2 };
        ui.add_enabled(
            enabled,
            egui::Button::new(disp_text(label.to_uppercase(), 12.0, fg))
                .fill(bg)
                .rounding(egui::Rounding::ZERO)
                .stroke(egui::Stroke::new(1.0, if enabled { RED } else { LINE })),
        )
    }

    /// The ACTION: the one verb a page is FOR, at the weight that says so.
    ///
    /// [`EMBER`] rather than [`RED`] because red is spent on live/selected — a red export button
    /// would read as "this row is chosen". Ember is the palette's "resolved / ready / colour
    /// returning", which is exactly what handing an asset out to a DCC is. Chamfered like a card so
    /// it belongs to the same printed vocabulary.
    ///
    /// Sized to be FOUND, not to dominate. The ember fill and the chamfer are what make it findable
    /// — weight is not doing that work, so it does not need any. It sits at the [`MIN_DISP`] floor,
    /// the same size as a `chip`, with tight padding and no minimum height: the label's own line box
    /// sets the height. The 10pt version buried in the status bar was invisible; 12pt/22px and then
    /// 11pt/17px both overshot the other way.
    pub fn action_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
        let (fg, bg, edge) = if enabled { (INK, EMBER, EMBER_DK) } else { (FAINT, G2, LINE) };
        let bevel = ui.painter().add(egui::Shape::Noop);
        let r = ui
            .scope(|ui| {
                ui.spacing_mut().button_padding = egui::vec2(7.0, 2.0) * type_scale();
                ui.add_enabled(
                    enabled,
                    egui::Button::new(disp_text(label.to_uppercase(), MIN_DISP, fg))
                        .fill(egui::Color32::TRANSPARENT)
                        .rounding(egui::Rounding::ZERO)
                        .stroke(egui::Stroke::NONE),
                )
            })
            .inner;
        ui.painter().set(
            bevel,
            egui::Shape::convex_polygon(chamfer(r.rect, CUT * 0.7), bg, egui::Stroke::new(1.0, edge)),
        );
        if enabled {
            r.clone().on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            r
        }
    }

    /// The STAMP: an irreversible action, as a printed mark rather than a colour.
    ///
    /// With one hot hue spent on selection there is no second hue left to mean "danger", so this
    /// says it with form instead — filled red, tracked caps, ink text, and a hard offset shadow,
    /// like a notice stamped on paper. It out-shouts any hue at this size, and it cannot be
    /// confused with a selected row, which is only ever *outlined* red.
    pub fn stamp_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
        let (fg, bg) = if enabled { (INK, RED) } else { (FAINT, G2) };
        // Reserve the shadow behind the button, then place it once the button has been laid out.
        let shadow = ui.painter().add(egui::Shape::Noop);
        let r = ui.add_enabled(
            enabled,
            egui::Button::new(disp_text(label.to_uppercase(), 12.0, fg))
                .fill(bg)
                .rounding(egui::Rounding::ZERO)
                .stroke(egui::Stroke::new(1.0, if enabled { RED } else { LINE })),
        );
        if enabled {
            ui.painter().set(
                shadow,
                egui::Shape::rect_filled(
                    r.rect.translate(egui::vec2(2.0, 2.0)),
                    egui::Rounding::ZERO,
                    RED_DK,
                ),
            );
        }
        r
    }

    /// How much weight a small badge carries. The CALLER owns the meaning (see `resolve::Prov`);
    /// the theme only owns the look — filled reads as settled, outlined as provisional, muted as
    /// absent.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Badge {
        /// Settled — a thing that is so.
        Lit,
        /// Provisional — a thing we guessed.
        Outline,
        /// Absent — nothing here yet.
        Muted,
    }

    pub fn badge(ui: &mut egui::Ui, label: &str, kind: Badge) {
        let (fg, bg, stroke) = match kind {
            Badge::Lit => (EMBER, EMBER_SOFT, EMBER_DK),
            Badge::Outline => (EMBER_DK, Color32::TRANSPARENT, EMBER_DK),
            Badge::Muted => (FAINT, G2, LINE),
        };
        egui::Frame::none()
            .fill(bg)
            .stroke(egui::Stroke::new(1.0, stroke))
            .rounding(egui::Rounding::ZERO)
            .inner_margin(egui::Margin::symmetric(5.0, 1.0))
            .show(ui, |ui| {
                ui.label(disp_text(label.to_uppercase(), 9.5, fg));
            });
    }

    /// A liberation meter: `done / total` of this page's subject, drawn into `r`.
    pub fn meter_in(painter: &egui::Painter, r: egui::Rect, done: usize, total: usize) {
        painter.rect_filled(r, egui::Rounding::ZERO, G3);
        if total > 0 && done > 0 {
            let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
            let mut lit = r;
            lit.set_width(r.width() * frac);
            painter.rect_filled(lit, egui::Rounding::ZERO, EMBER);
        }
    }

    /// The rail's width. Fixed: it is a set of destinations, not a panel.
    pub const RAIL_W: f32 = 64.0;

    /// Command-bar and status-bar heights.
    ///
    /// Derived from [`type_scale`] so they stay proportionate when the type grows — a fixed 46 px
    /// bar that was right at 1.0 scale crops its own title at 1.2. The bars set these as an EXACT
    /// height and centre their contents, which is what keeps the space above and below equal;
    /// letting a panel size to its tallest child makes symmetric padding impossible.
    pub fn bar_h() -> f32 {
        40.0 * type_scale()
    }
    pub fn status_h() -> f32 {
        24.0 * type_scale()
    }
    /// The verb bar — taller than the status bar because it holds a button rather than a readout.
    pub fn verb_h() -> f32 {
        30.0 * type_scale()
    }

    /// The frame shared by both bars: panel fill, horizontal breathing room, and NO vertical
    /// margin — the vertical centring does that job, and a margin would fight it.
    pub fn bar_frame() -> egui::Frame {
        egui::Frame::none()
            .fill(G1)
            .inner_margin(egui::Margin { left: 12.0, right: 12.0, top: 0.0, bottom: 0.0 })
    }

    /// One rail destination: keycap, glyph, label, and a 2px meter of how much of that page's
    /// subject is resolved.
    ///
    /// The meter is the reason the rail is more than four icons — it answers "how much of THAT is
    /// accounted for?" without making you go and look. `total == 0` draws no meter at all, because
    /// a page with nothing to resolve should not imply that it has work waiting.
    pub fn rail_button(
        ui: &mut egui::Ui,
        key: &str,
        glyph: &str,
        label: &str,
        selected: bool,
        done: usize,
        total: usize,
    ) -> egui::Response {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(RAIL_W, RAIL_W), egui::Sense::click());
        let p = ui.painter();
        let fg = if selected {
            RED
        } else if resp.hovered() {
            DIM
        } else {
            FAINT
        };
        if selected {
            p.rect_filled(rect, egui::Rounding::ZERO, RED_SOFT);
            // the live marker: a red bar down the selected edge
            let mut bar = rect;
            bar.set_width(3.0);
            p.rect_filled(bar.shrink2(egui::vec2(0.0, 7.0)), egui::Rounding::ZERO, RED);
        } else if resp.hovered() {
            p.rect_filled(rect, egui::Rounding::ZERO, Color32::from_rgba_unmultiplied(255, 255, 255, 6));
        }
        // keycap, top-left
        p.text(
            rect.left_top() + egui::vec2(8.0, 6.0),
            egui::Align2::LEFT_TOP,
            key,
            FontId::new(9.0, data()),
            if selected { RED } else { FAINT },
        );
        // glyph + label, stacked and centred
        p.text(
            rect.center() - egui::vec2(0.0, 9.0),
            egui::Align2::CENTER_CENTER,
            glyph,
            FontId::new(17.0, FontFamily::Proportional),
            fg,
        );
        p.text(
            rect.center() + egui::vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            label.to_uppercase(),
            FontId::new(9.0, disp()),
            fg,
        );
        if total > 0 {
            let m = egui::Rect::from_min_size(
                rect.left_bottom() + egui::vec2(14.0, -9.0),
                egui::vec2(RAIL_W - 28.0, 2.0),
            );
            meter_in(p, m, done, total);
        }
        resp
    }

    /// A small square status dot + label (the "READY" pill in the status bar).
    pub fn status_dot(ui: &mut egui::Ui, label: &str, color: Color32) {
        let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
        ui.painter().rect_filled(r, egui::Rounding::ZERO, color);
        ui.add_space(2.0);
        ui.label(disp_text(label.to_uppercase(), 9.5, color));
    }

    /// The brand mark: the **Cross of Lorraine** in resistance red — the emblem of the Free French
    /// and of the Resistance this tool's subject belongs to. Replaces the Mercs2 brass diamond.
    ///
    /// Drawn to the heraldic proportion: a patriarchal cross has TWO crossbars with the upper one
    /// SHORTER than the lower. (Reversing them gives a different cross entirely.)
    pub fn brand_mark(ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(20.0, 26.0), egui::Sense::hover());
        let c = rect.center();
        let p = ui.painter();
        let bar = |w: f32, h: f32, dy: f32| {
            egui::Rect::from_center_size(c + egui::vec2(0.0, dy), egui::vec2(w, h))
        };
        // vertical, then the short upper crossbar, then the long lower one
        p.rect_filled(bar(3.0, 24.0, 0.0), egui::Rounding::ZERO, RED);
        p.rect_filled(bar(11.0, 3.0, -6.0), egui::Rounding::ZERO, RED);
        p.rect_filled(bar(15.0, 3.0, 2.0), egui::Rounding::ZERO, RED);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Every glyph in [`sym`] must be drawable by a font we SHIP — Oswald, or one of the three
        /// egui bundles (Ubuntu-Light, NotoEmoji, emoji-icon-font).
        ///
        /// Hack is excluded on purpose even though it is bundled: it is only reachable from the
        /// Monospace family, so a symbol that exists *only* in Hack still tofus on the rail. Segoe UI
        /// is excluded too — it is read from `C:/Windows/Fonts` at runtime and may be absent, and a
        /// test that trusted it would pass here and ship tofu elsewhere.
        #[test]
        fn glyphs_are_covered() {
            let defs = egui::FontDefinitions::default();
            let mut faces: Vec<ttf_parser::Face> = Vec::new();
            for name in ["Ubuntu-Light", "NotoEmoji-Regular", "emoji-icon-font"] {
                let d = defs.font_data.get(name).unwrap_or_else(|| {
                    panic!("egui no longer bundles {name}; update this test and `sym`")
                });
                faces.push(ttf_parser::Face::parse(d.font.as_ref(), 0).unwrap());
            }
            faces.push(ttf_parser::Face::parse(OSWALD_MEDIUM, 0).unwrap());
            faces.push(ttf_parser::Face::parse(OSWALD_SEMIBOLD, 0).unwrap());

            let mut tofu = Vec::new();
            for (name, s) in sym::ALL {
                for ch in s.chars() {
                    if !faces.iter().any(|f| f.glyph_index(ch).is_some()) {
                        tofu.push(format!("{name} = {s:?} (U+{:04X})", ch as u32));
                    }
                }
            }
            assert!(
                tofu.is_empty(),
                "no bundled font draws these — they will paint empty boxes:\n  {}",
                tofu.join("\n  ")
            );
        }
    }
}

fn map_key(code: KeyCode) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match code {
        KeyCode::ArrowUp => K::ArrowUp,
        KeyCode::ArrowDown => K::ArrowDown,
        KeyCode::ArrowLeft => K::ArrowLeft,
        KeyCode::ArrowRight => K::ArrowRight,
        KeyCode::Enter | KeyCode::NumpadEnter => K::Enter,
        KeyCode::Escape => K::Escape,
        KeyCode::Tab => K::Tab,
        KeyCode::Backspace => K::Backspace,
        KeyCode::Delete => K::Delete,
        KeyCode::Space => K::Space,
        KeyCode::Home => K::Home,
        KeyCode::End => K::End,
        KeyCode::PageUp => K::PageUp,
        KeyCode::PageDown => K::PageDown,
        KeyCode::KeyA => K::A,
        KeyCode::KeyC => K::C,
        KeyCode::KeyV => K::V,
        KeyCode::KeyX => K::X,
        KeyCode::KeyZ => K::Z,
        KeyCode::F10 => K::F10,
        _ => return None,
    })
}
