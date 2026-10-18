//! Assembled assets, from the game's own `GameTemplates` (not a name heuristic).
//!
//! A character is an **`FxHumanBodySetup`** template that references an `FxHumanHead` + several
//! `FxHumanBodyPart` templates (properties `0x622dd842` / `0xa1757f0a`), each of which references the
//! actual mesh. Weapons / vehicles / props reference their meshes directly. So an *assembled asset* is
//! an asset-kind template, and its *parts* are the meshes it resolves to (directly, or one hop through
//! a body-part template). Proven on `FBS_RS_Sean` → FM/FX/HD, UB, LB, HAT, GR, Hair; 1328/1559 distinct
//! meshes are template-referenced, the rest are `Ungrouped`.
//!
//! A body setup does not list parts to stack: it lists **slots** (head, upper body, lower body, three
//! gear slots, one more), each holding weighted *variations* — see [`SetupSlot`].

use std::collections::{HashMap, HashSet};

use sab_formats::gametemplates::{GameTemplates, Template};
use sab_formats::pandemic_hash;

use crate::source::{GameSource, MeshFind};

/// `WSFxHumanBodyPartBlueprint::SetProperty` (@0x514bd0) stores these two references and its resolve
/// step (@0x5192f0) looks each one up as an asset.
const HIGH_DETAIL_MODEL: u32 = 0x6240_4569;
const LOW_DETAIL_MODEL: u32 = 0xd972_5c55;
/// `CombinedLODModel`, stored by both the body-part and the body-setup blueprint. The 2008 engine
/// draws it as the whole figure at level 2: `WSFxHumanBodySetup::Render` (WildStar_p @0x8227e020)
/// calls `RenderCombinedLOD` for level 2 and falls back to level 1 when there is none;
/// `SetupCombinedLODModel` (@0x8227dc00) takes it from the first of slots 0–4 whose part has one,
/// else from the setup.
const COMBINED_LOD_MODEL: u32 = 0xcba8_d62c;

/// `WSFxHumanBodySetupBlueprint::SetProperty` (@0x515c40, a key compare chain) reads a body setup as
/// a stream: a slot key carries the slot's variation count and makes it the current slot;
/// `FxHumanBodyPartVariation 0x922cc098` starts the next variation in it; `BodyPart 0xa1757f0a` and
/// `BodyPartProbability 0xd0541403` fill that variation. The head list works the same way:
/// `Head 0x44c27fcb` (count), `FxHumanHeadVariation 0xc57d2b43`, `Face 0x622dd842`,
/// `FaceProbability 0xd594db5b`. Slot keys and the slot index the engine stores each under:
/// `UpperBody 0x79691a2f` 0, `LowerBody 0xdb9c23dc` 1, `GearA 0x9e288655` 2, `GearB 0xfc20cb8a` 3,
/// `GearC 0x1e233fa7` 4, `0x37a9525d` 5.
const SLOT_KEYS: [u32; 6] = [0x7969_1a2f, 0xdb9c_23dc, 0x9e28_8655, 0xfc20_cb8a, 0x1e23_3fa7, 0x37a9_525d];

/// A damage part's keys (docs/damage_and_destruction.md): `BoneName` starts a part. Retail states it
/// with `Pristine` (bit 0 of its byte: set draws in group state 0, clear in state 1 — `FUN_006678e0`);
/// the 2008 build with `0xb7b68d44`, the group state it draws in, negative for every state
/// (`WSDamageablePartBlueprint::SetDamageableProperty` @0x82380c48 stores it at part `+0x14`, which
/// `WSDamageableGroup::ActivatePart` @0x823803c8 compares with the group's state).
const BONE_NAME: u32 = 0x207f_ba52;
const PRISTINE: u32 = 0x86de_6639;
const PART_STATE: u32 = 0xb7b6_8d44;
const HEAD_KEY: u32 = 0x44c2_7fcb;
const BODY_PART_VARIATION: u32 = 0x922c_c098;
const HEAD_VARIATION: u32 = 0xc57d_2b43;
const BODY_PART: u32 = 0xa175_7f0a;
const FACE: u32 = 0x622d_d842;

/// Where a part sits in its body setup: which slot, and which variation of it. A slot's variations
/// are alternatives; a figure wears one per slot (`docs/formats/gametemplates.md`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SetupSlot {
    /// The slot key (`SLOT_KEYS`, or `HEAD_KEY` for the head list).
    pub key: u32,
    /// 0-based variation within the slot, in stream order.
    pub variation: u16,
}

/// One constituent mesh of an assembled asset.
#[derive(Clone)]
pub struct PartRef {
    /// Index into the flat mesh list (what the loader takes).
    pub mesh_index: usize,
    /// The mesh name (shown in the parts panel).
    pub name: String,
    /// A short label — the mesh's part suffix (`UB`, `HD`, `HAT`, `whole`, …), plus `· alt N` for a
    /// slot's later variation and `· LOD1` for a low-detail model.
    pub label: String,
    /// `Some(0)` = the part template's `HighDetailModel`, `Some(1)` = its `LowDetailModel`, `Some(2)`
    /// = the setup's combined model (`COMBINED_LOD_MODEL`); `None` = any other mesh reference (an
    /// asset template's direct mesh, or a part template's other refs).
    pub lod: Option<u8>,
    /// The `FxHumanBodyPart` / `FxHumanHead` template this mesh came through, if any.
    pub template: Option<String>,
    /// The setup slot + variation the part template was listed under (`BodyPart` / `Face`).
    pub slot: Option<SetupSlot>,
}

/// An assembled asset: a template name, its browser category, and every mesh that folds into it.
pub struct Asset {
    pub name: String,
    pub category: &'static str,
    pub parts: Vec<PartRef>,
    /// Per part/head template, a model reference that names a mesh this install does not have —
    /// `(slot it was listed under, message)`. Surfaced in the assembly's notes.
    pub missing: Vec<(Option<SetupSlot>, String)>,
    /// A world block: every part is drawn, at level 0.
    pub whole: bool,
    /// The template's damage parts: `(bone name hash, the group state its geometry draws in)`.
    pub damage: Vec<(u32, i32)>,
}

/// The category of world blocks.
pub const WORLD: &str = "World";

/// What a click assembles: the meshes with their levels of detail, and anything that could not be.
#[derive(Clone, Default)]
pub struct Assembly {
    /// `(mesh_index, lod)`.
    pub picks: Vec<(usize, u8)>,
    /// Human-readable notes about references that name meshes not in this install.
    pub notes: Vec<String>,
    /// The asset's damage parts ([`Asset::damage`]), which decide each drawcall's damage state.
    pub damage: Vec<(u32, i32)>,
}

impl PartRef {
    /// A mesh that did not come through a part template's model references (no level of detail).
    pub fn plain(mesh_index: usize, name: &str, label: String) -> PartRef {
        PartRef { mesh_index, name: name.to_string(), label, lod: None, template: None, slot: None }
    }
}

/// The variation an assembly shows for a slot: the first one the setup lists.
fn is_shown(slot: Option<SetupSlot>) -> bool {
    slot.map_or(true, |s| s.variation == 0)
}

impl Asset {
    /// A one-mesh `Ungrouped` asset — a mesh no template claims.
    pub fn single(mesh_index: usize, name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            category: "Ungrouped",
            parts: vec![PartRef::plain(mesh_index, name, "whole".into())],
            missing: Vec::new(),
            whole: false,
            damage: Vec::new(),
        }
    }

    /// The parts to ASSEMBLE when this asset is clicked.
    ///
    /// A character (parts came through `FxHumanBodyPart` / `FxHumanHead` templates) assembles from
    /// the templates' own model references: one variation per setup slot ([`SetupSlot`] — the
    /// first the setup lists, head included), and of each, the `HighDetailModel` at level 0 and the
    /// `LowDetailModel` at level 1 — the same mesh again when the template names the same one, since
    /// that is what the engine draws at that distance — plus the combined model at level 2.
    ///
    /// Anything else (weapons, props, vehicles, a name-grouped boot character) uses the slot rules on
    /// part labels, at level 0 only.
    pub fn assembly(&self) -> Assembly {
        if self.whole {
            return Assembly { picks: self.parts.iter().map(|p| (p.mesh_index, 0)).collect(), notes: Vec::new(), damage: self.damage.clone() };
        }
        if self.parts.iter().any(|p| p.lod.is_some()) {
            let mut picks: Vec<(usize, u8)> = Vec::new();
            for p in &self.parts {
                let Some(lod) = p.lod else { continue };
                if !is_shown(p.slot) {
                    continue; // another variation of the slot
                }
                if !picks.contains(&(p.mesh_index, lod)) {
                    picks.push((p.mesh_index, lod));
                }
            }
            let notes = self.missing.iter().filter(|(s, _)| is_shown(*s)).map(|(_, m)| m.clone()).collect();
            return Assembly { picks, notes, damage: self.damage.clone() };
        }
        let notes = self.missing.iter().map(|(_, m)| m.clone()).collect();
        Assembly { picks: self.label_assembly().into_iter().map(|i| (i, 0)).collect(), notes, damage: self.damage.clone() }
    }

    /// The slot rules for assets with no part templates — one mesh per body slot, by label.
    ///
    /// `parts` is everything the templates reference, which includes things that must NOT be stacked
    /// on top of each other:
    ///   * **LODs** (`…_LB_LOD`) are a lower-detail *substitute* for a part, not extra geometry.
    ///   * **Alternate heads** — a character ships `_HD`, and often `_FM` / `_FX` as well, which are
    ///     the same head at the same triangle count. Merging them all buries three faces in one skull.
    /// So group by slot and take one from each, preferring the plain part over any variant.
    fn label_assembly(&self) -> Vec<usize> {
        // slot key: the head variants collapse to one slot; everything else keys on its own label.
        fn slot(label: &str) -> String {
            let u = label.to_ascii_uppercase();
            match u.as_str() {
                "HD" | "FM" | "FX" | "HEAD" => "HEAD".into(),
                _ => u,
            }
        }
        let mut chosen: std::collections::BTreeMap<String, (&PartRef, i32)> = Default::default();
        for p in &self.parts {
            let u = p.label.to_ascii_uppercase();
            if u.contains("LOD") {
                continue; // a substitute, never an addition
            }
            // rank within a slot: a plain HD beats an FX/FM stand-in.
            let rank = match u.as_str() {
                "HD" | "HEAD" => 0,
                "FM" => 1,
                "FX" => 2,
                _ => 0,
            };
            let k = slot(&p.label);
            match chosen.get(&k) {
                Some((_, r)) if *r <= rank => {}
                _ => {
                    chosen.insert(k, (p, rank));
                }
            }
        }
        if chosen.is_empty() {
            // no labelled parts (a one-mesh prop): fall back to the single primary
            return vec![self.primary()];
        }
        chosen.values().map(|(p, _)| p.mesh_index).collect()
    }

    /// The mesh to load when the asset is clicked: prefer a full non-LOD body part, else first part.
    pub fn primary(&self) -> usize {
        const PREF: &[&str] = &["UB", "HD", "FM", "LB", "whole"];
        for want in PREF {
            if let Some(p) = self.parts.iter().find(|p| p.label.eq_ignore_ascii_case(want)) {
                return p.mesh_index;
            }
        }
        self.parts
            .iter()
            .find(|p| !p.label.to_ascii_uppercase().contains("LOD"))
            .or_else(|| self.parts.first())
            .map(|p| p.mesh_index)
            .unwrap_or(0)
    }
}

/// Browser category for an asset-kind template type, or `None` if the type is not a browsable asset
/// (structural/config templates, sub-parts, etc.).
fn category_for(ttype: &str) -> Option<&'static str> {
    Some(match ttype {
        "FxHumanBodySetup" => "Characters",
        "CAR" | "TRUCK" | "TANK" | "APC" | "BOAT" | "AIRPLANE" | "Train" | "TrainCarriage" | "TrainEngine" | "TrainItem" => "Vehicles",
        "Weapon" | "MeleeWeapon" | "Ammo" | "Explosion" => "Weapons",
        "Prop" | "Item" => "Props",
        _ => return None,
    })
}

/// A template that stands *between* an asset and its mesh (a character body part / head).
fn is_part_type(ttype: &str) -> bool {
    matches!(ttype, "FxHumanBodyPart" | "FxHumanHead")
}

/// The part label for a mesh name: its trailing token, or the last two if the trailing one is a bare
/// number (`..._01_UB` → `UB`, `..._GR_2` → `GR_2`).
pub fn part_label(name: &str) -> String {
    let toks: Vec<&str> = name.split('_').collect();
    match toks.as_slice() {
        [.., a, b] if b.bytes().all(|c| c.is_ascii_digit()) => format!("{a}_{b}"),
        [.., b] => b.to_string(),
        _ => name.to_string(),
    }
}

/// Build the assembled-asset list from a build's templates and meshes. A template's reference to a
/// mesh resolves through [`GameSource::find_mesh`] (the engine's rule for that build); templates whose
/// meshes resolve to nothing are skipped; meshes no asset claims become one-part `Ungrouped` assets so
/// nothing is hidden; each world block is one `World` asset of all its meshes.
pub fn build(src: &dyn GameSource) -> Vec<Asset> {
    let gt = src.templates();
    let meshes = src.meshes();
    // template name-hash → template (for the one-hop body-part resolution).
    let mut tpl_by_hash: HashMap<u32, &Template> = HashMap::new();
    for (_, t) in gt.templates() {
        tpl_by_hash.entry(pandemic_hash(&t.name)).or_insert(t);
    }
    let found = |u: u32, template: &str| match src.find_mesh(u, template) {
        MeshFind::Found(i) => Some(i),
        _ => None,
    };
    // a template's directly-referenced meshes (a 4-byte value that names a mesh).
    let direct = |t: &Template| -> Vec<usize> { t.pairs.iter().filter_map(|p| p.as_u32()).filter_map(|u| found(u, &t.name)).collect() };

    let mut assets: Vec<Asset> = Vec::new();
    let mut used: HashSet<usize> = HashSet::new(); // mesh items claimed by an asset

    for (_, t) in gt.templates() {
        let Some(cat) = category_for(&t.ttype) else { continue };
        let mut parts: Vec<PartRef> = Vec::new();
        let mut missing: Vec<(Option<SetupSlot>, String)> = Vec::new();
        let mut push = |parts: &mut Vec<PartRef>, i: usize, label: String, lod, template: Option<&str>, slot| {
            let key = (i, lod, template.map(str::to_string), slot);
            if parts.iter().any(|p| (p.mesh_index, p.lod, p.template.clone(), p.slot) == key) {
                return;
            }
            used.insert(i);
            parts.push(PartRef { mesh_index: i, name: meshes[i].name.clone(), label, lod, template: template.map(str::to_string), slot });
        };
        // The setup stream's state, as `SetProperty` keeps it (see `SLOT_KEYS`).
        let (mut body_slot, mut body_var, mut head_var): (Option<u32>, Option<u16>, Option<u16>) = (None, None, None);
        // Each shown slot part's combined model: (slot, its template, the model).
        let mut combined: Vec<(SetupSlot, String, u32)> = Vec::new();
        for p in &t.pairs {
            if SLOT_KEYS.contains(&p.hash) {
                (body_slot, body_var) = (Some(p.hash), None);
            } else if p.hash == HEAD_KEY {
                head_var = None;
            } else if p.hash == BODY_PART_VARIATION {
                body_var = Some(body_var.map_or(0, |v| v + 1));
            } else if p.hash == HEAD_VARIATION {
                head_var = Some(head_var.map_or(0, |v| v + 1));
            }
            if p.hash == COMBINED_LOD_MODEL {
                continue; // the setup's own; chosen after its slots
            }
            let Some(u) = p.as_u32() else { continue };
            match src.find_mesh(u, &t.name) {
                MeshFind::Found(i) => {
                    push(&mut parts, i, part_label(&meshes[i].name), None, None, None);
                    continue;
                }
                MeshFind::Ambiguous(paths) => {
                    missing.push((None, format!("{}: mesh {u:08X} has differing copies in {} and none in the template's own pack", t.name, paths.join(", "))));
                    continue;
                }
                MeshFind::Absent => {}
            }
            let Some(sub) = tpl_by_hash.get(&u) else { continue };
            if !is_part_type(&sub.ttype) {
                continue;
            }
            let slot = match p.hash {
                BODY_PART => match (body_slot, body_var) {
                    (Some(key), Some(variation)) => Some(SetupSlot { key, variation }),
                    _ => {
                        missing.push((None, format!("{}: BodyPart {} is not inside a slot variation", t.name, sub.name)));
                        continue;
                    }
                },
                FACE => match head_var {
                    Some(variation) => Some(SetupSlot { key: HEAD_KEY, variation }),
                    None => {
                        missing.push((None, format!("{}: Face {} is not inside a head variation", t.name, sub.name)));
                        continue;
                    }
                },
                _ => None,
            };
            let alt = |base: String| match slot {
                Some(s) if s.variation > 0 => format!("{base} · alt {}", s.variation),
                _ => base,
            };
            // One hop through FxHumanBodyPart / FxHumanHead: its model references, by level.
            let model = |key: u32| sub.pair(key).and_then(|q| q.as_u32()).filter(|v| *v != 0);
            let (high, low) = (model(HIGH_DETAIL_MODEL), model(LOW_DETAIL_MODEL));
            if let (Some(s), Some(c)) = (slot.filter(|s| s.key != HEAD_KEY && s.variation == 0), model(COMBINED_LOD_MODEL)) {
                combined.push((s, sub.name.clone(), c));
            }
            let (high_i, low_i) = (high.and_then(|h| found(h, &sub.name)), low.and_then(|h| found(h, &sub.name)));
            let high_label = high_i.map(|i| part_label(&meshes[i].name));
            for (lvl, prop, h) in [(0u8, "HighDetailModel", high), (1u8, "LowDetailModel", low)] {
                let Some(h) = h else { continue };
                match src.find_mesh(h, &sub.name) {
                    MeshFind::Found(i) => {
                        let base = alt(high_label.clone().unwrap_or_else(|| part_label(&meshes[i].name)));
                        let label = if lvl == 0 { base } else { format!("{base} · LOD1") };
                        push(&mut parts, i, label, Some(lvl), Some(&sub.name), slot);
                    }
                    MeshFind::Absent => {
                        missing.push((slot, format!("{}: {prop} {h:08X} names a mesh that is not in this install", sub.name)))
                    }
                    MeshFind::Ambiguous(paths) => missing.push((
                        slot,
                        format!("{}: {prop} {h:08X} has differing copies in {} and none in the template's own pack", sub.name, paths.join(", ")),
                    )),
                }
            }
            // Its other mesh references stay listed (inspectable) but are not assembled.
            for i in direct(sub) {
                if Some(i) != high_i && Some(i) != low_i {
                    push(&mut parts, i, alt(part_label(&meshes[i].name)), None, Some(&sub.name), slot);
                }
            }
        }
        // The combined model: the first of slots 0–4 whose shown part has one, else the setup's own.
        let own = t.pair(COMBINED_LOD_MODEL).and_then(|q| q.as_u32()).filter(|v| *v != 0).map(|c| (None, t.name.clone(), c));
        let chosen = SLOT_KEYS[..5]
            .iter()
            .find_map(|k| combined.iter().find(|(s, _, _)| s.key == *k).map(|(s, n, c)| (Some(*s), n.clone(), *c)))
            .or(own);
        if let Some((slot, template, c)) = chosen {
            match src.find_mesh(c, &template) {
                MeshFind::Found(i) => push(&mut parts, i, format!("{} · combined", part_label(&meshes[i].name)), Some(2), Some(&template), slot),
                MeshFind::Absent => missing.push((slot, format!("{template}: CombinedLODModel {c:08X} names a mesh that is not in this install"))),
                MeshFind::Ambiguous(paths) => missing.push((
                    slot,
                    format!("{template}: CombinedLODModel {c:08X} has differing copies in {} and none in the template's own pack", paths.join(", ")),
                )),
            }
        }
        if parts.is_empty() {
            continue; // a template with no renderable mesh in this build — not browsable here
        }
        let damage = damage_parts(t).into_iter().map(|(b, s)| (b, s.unwrap_or(-1))).collect();
        assets.push(Asset { name: t.name.clone(), category: cat, parts, missing, whole: false, damage });
    }

    // Everything a template didn't claim → one-part Ungrouped assets.
    for (i, m) in meshes.iter().enumerate() {
        if !m.world && !used.contains(&i) {
            assets.push(Asset::single(i, &m.name));
        }
    }
    for (block, items) in src.world_blocks() {
        let parts = items.iter().map(|&i| PartRef::plain(i, &meshes[i].name, part_label(&meshes[i].name))).collect();
        assets.push(Asset { name: block.clone(), category: WORLD, parts, missing: Vec::new(), whole: true, damage: Vec::new() });
    }
    assets
}

/// A template's damage parts as `(bone, state)`; `None` for a part that states none, which the 2008
/// part blueprint's constructor (@0x82381d98) leaves at `-1`: drawn in every state.
fn damage_parts(t: &Template) -> Vec<(u32, Option<i32>)> {
    let mut parts: Vec<(u32, Option<i32>)> = Vec::new();
    for p in &t.pairs {
        match (p.hash, parts.last_mut()) {
            (BONE_NAME, _) => {
                if let Some(b) = p.as_u32() {
                    parts.push((b, None));
                }
            }
            (PRISTINE, Some(last)) => last.1 = p.data.first().map(|v| if v & 1 != 0 { 0 } else { 1 }),
            (PART_STATE, Some(last)) => last.1 = p.as_u32().map(|v| v as i32),
            _ => {}
        }
    }
    parts
}

/// The asset of the build's player: the body setup its one `Player` template names as `FxBodySetup`.
pub fn player(gt: &GameTemplates, assets: &[Asset]) -> Result<usize, String> {
    let players: Vec<&Template> = gt.templates().map(|(_, t)| t).filter(|t| t.ttype == "Player").collect();
    let [p] = players.as_slice() else { return Err(format!("{} Player templates; the build has one", players.len())) };
    let fx = p.pair(pandemic_hash("FxBodySetup")).and_then(|v| v.as_u32()).ok_or_else(|| format!("Player template {} names no FxBodySetup", p.name))?;
    let setup = gt
        .templates()
        .map(|(_, t)| t)
        .find(|t| pandemic_hash(&t.name) == fx)
        .ok_or_else(|| format!("Player template {}'s FxBodySetup {fx:08X} names no template", p.name))?;
    assets.iter().position(|a| a.name == setup.name).ok_or_else(|| format!("{} assembles from no mesh of this build", setup.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key names in `SLOT_KEYS`' documentation are the strings these hashes come from.
    #[test]
    fn setup_key_names_hash_to_the_keys() {
        for (name, key) in [
            ("UpperBody", SLOT_KEYS[0]),
            ("LowerBody", SLOT_KEYS[1]),
            ("GearA", SLOT_KEYS[2]),
            ("GearB", SLOT_KEYS[3]),
            ("GearC", SLOT_KEYS[4]),
            ("Head", HEAD_KEY),
            ("FxHumanBodyPartVariation", BODY_PART_VARIATION),
            ("FxHumanHeadVariation", HEAD_VARIATION),
            ("BodyPart", BODY_PART),
            ("Face", FACE),
            ("HighDetailModel", HIGH_DETAIL_MODEL),
            ("LowDetailModel", LOW_DETAIL_MODEL),
            ("CombinedLODModel", COMBINED_LOD_MODEL),
        ] {
            assert_eq!(pandemic_hash(name), key, "{name}");
        }
    }

    /// Every character of both builds assembles as ONE variation per setup slot: every `BodyPart` /
    /// `Face` sits inside a variation of a slot, no slot contributes two part templates at level 0,
    /// and every slot whose first variation names a mesh the build has contributes it.
    #[cfg(feature = "retail")]
    #[test]
    fn characters_assemble_one_variation_per_slot() {
        for src in crate::testenv::both_builds() {
            check_characters(src);
        }
    }

    #[cfg(feature = "retail")]
    fn check_characters(src: &dyn GameSource) {
        let assets = build(src);
        let meshes = src.meshes();
        let mut chars = 0usize;
        let mut bad: Vec<String> = Vec::new();
        for a in assets.iter().filter(|a| a.category == "Characters") {
            chars += 1;
            for (_, m) in a.missing.iter().filter(|(s, _)| s.is_none()) {
                bad.push(m.clone());
            }
            let asm = a.assembly();
            // Every pick comes from a shown variation (or a part outside any slot).
            for &(i, lod) in &asm.picks {
                if !a.parts.iter().any(|p| p.mesh_index == i && p.lod == Some(lod) && is_shown(p.slot)) {
                    bad.push(format!("{}: {} (LOD{lod}) is not from a shown variation", a.name, meshes[i].name));
                }
            }
            // slot key -> the part templates of its shown variation. More than one means the stream
            // replay did not advance the variation index.
            let mut per_slot: HashMap<u32, HashSet<&str>> = HashMap::new();
            for p in a.parts.iter().filter(|p| p.lod.is_some() && is_shown(p.slot)) {
                if let (Some(slot), Some(t)) = (p.slot, p.template.as_deref()) {
                    per_slot.entry(slot.key).or_default().insert(t);
                }
            }
            for (key, ts) in &per_slot {
                if ts.len() > 1 {
                    bad.push(format!("{}: slot {key:08X} assembles {ts:?}", a.name));
                }
            }
            for p in a.parts.iter().filter(|p| p.lod == Some(0) && is_shown(p.slot)) {
                if !asm.picks.contains(&(p.mesh_index, 0)) {
                    bad.push(format!("{}: first variation {} not assembled", a.name, p.name));
                }
            }
        }
        assert!(chars > 0, "{}: no characters assembled", src.label());
        assert!(bad.is_empty(), "{}: {} problems:\n{}", src.label(), bad.len(), bad.join("\n"));
        // Every character's shown variation assembles.
        let mut failed: Vec<String> = Vec::new();
        for a in assets.iter().filter(|a| a.category == "Characters") {
            let asm = a.assembly();
            match crate::meshload::assemble_with(&asm.picks, |i| src.load_part(i), |i| meshes[i].name.clone()) {
                Ok((_, skipped)) if skipped.is_empty() => {}
                Ok((_, skipped)) => failed.push(format!("{}: {}", a.name, skipped.join("; "))),
                Err(e) => failed.push(format!("{}: {e}", a.name)),
            }
        }
        assert!(failed.is_empty(), "{}: {} characters do not assemble:\n{}", src.label(), failed.len(), failed.join("\n"));
        player(src.templates(), &assets).unwrap_or_else(|e| panic!("{}: {e}", src.label()));
    }

    /// Four alternative upper bodies; the figure wears the first.
    #[cfg(feature = "retail")]
    #[test]
    fn a_retail_setup_wears_its_first_upper_body() {
        let assets = build(crate::testenv::retail());
        let elderly = assets.iter().find(|a| a.name == "FBS_CV_Elderly_F").expect("FBS_CV_Elderly_F");
        let asm = elderly.assembly();
        let ub: Vec<&str> = elderly
            .parts
            .iter()
            .filter(|p| p.lod == Some(0) && p.slot.map(|s| s.key) == Some(SLOT_KEYS[0]))
            .filter(|p| asm.picks.contains(&(p.mesh_index, 0)))
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(ub, ["CH_CF_Trench_UB"], "the first UpperBody variation, alone");
    }

    /// Every retail damage part states its state (retail's default for a part without `Pristine` is
    /// not established; none needs it), and a damageable car's geometry splits into its intact (0)
    /// and damaged (1) state rather than drawing both at once.
    #[cfg(feature = "retail")]
    #[test]
    fn damage_parts_split_a_car_by_state() {
        let retail = crate::testenv::retail();
        let stateless: Vec<&str> = retail
            .templates()
            .templates()
            .filter(|(_, t)| category_for(&t.ttype).is_some() && damage_parts(t).iter().any(|(_, s)| s.is_none()))
            .map(|(_, t)| t.name.as_str())
            .collect();
        assert!(stateless.is_empty(), "retail parts without Pristine: {stateless:?}");
        for src in crate::testenv::both_builds() {
            let assets = build(src);
            let damaged = assets.iter().filter(|a| !a.damage.is_empty()).count();
            eprintln!("{}: {damaged} assets carry damage parts", src.label());
            assert!(damaged > 0, "{}", src.label());
        }
        let src = crate::testenv::proto2008();
        let assets = build(src);
        let car = assets.iter().find(|a| a.name == "VH_CV_CR_Allard_Skylar").expect("VH_CV_CR_Allard_Skylar");
        let model = crate::boot::assemble(src, &car.name, &car.assembly()).expect("assemble");
        let cover = crate::formats::submesh_cover(&model.lm.mesh.prims, model.lm.mesh.indices.len() as u32);
        let mut states: Vec<i32> = cover.iter().filter_map(|s| s.damage).collect();
        states.sort_unstable();
        states.dedup();
        assert_eq!(states, [0, 1], "the Allard's geometry by damage state");
    }

    /// Both builds' player is Sean's body setup.
    #[cfg(feature = "retail")]
    #[test]
    fn the_player_is_sean() {
        for src in crate::testenv::both_builds() {
            let assets = build(src);
            let p = player(src.templates(), &assets).unwrap_or_else(|e| panic!("{}: {e}", src.label()));
            assert_eq!(assets[p].name, "FBS_RS_Sean", "{}", src.label());
        }
    }
}
