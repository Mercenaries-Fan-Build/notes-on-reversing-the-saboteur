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

use crate::meshload::MeshEntry;

/// `WSFxHumanBodyPartBlueprint::SetProperty` (@0x514bd0) stores these two references and its resolve
/// step (@0x5192f0) looks each one up as an asset. Data (retail): `LowDetailModel` names the part's
/// `_LOD` mesh for 456 body parts, the same mesh as `HighDetailModel` for 106 (no separate LOD).
const HIGH_DETAIL_MODEL: u32 = 0x6240_4569;
const LOW_DETAIL_MODEL: u32 = 0xd972_5c55;

/// `WSFxHumanBodySetupBlueprint::SetProperty` (@0x515c40, a key compare chain) reads a body setup as
/// a stream: a slot key carries the slot's variation count and makes it the current slot;
/// `FxHumanBodyPartVariation 0x922cc098` starts the next variation in it; `BodyPart 0xa1757f0a` and
/// `BodyPartProbability 0xd0541403` fill that variation. The head list works the same way:
/// `Head 0x44c27fcb` (count), `FxHumanHeadVariation 0xc57d2b43`, `Face 0x622dd842`,
/// `FaceProbability 0xd594db5b`. Slot keys and the slot index the engine stores each under:
/// `UpperBody 0x79691a2f` 0, `LowerBody 0xdb9c23dc` 1, `GearA 0x9e288655` 2, `GearB 0xfc20cb8a` 3,
/// `GearC 0x1e233fa7` 4, `0x37a9525d` (name not recovered) 5.
const SLOT_KEYS: [u32; 6] = [0x7969_1a2f, 0xdb9c_23dc, 0x9e28_8655, 0xfc20_cb8a, 0x1e23_3fa7, 0x37a9_525d];
const HEAD_KEY: u32 = 0x44c2_7fcb;
const BODY_PART_VARIATION: u32 = 0x922c_c098;
const HEAD_VARIATION: u32 = 0xc57d_2b43;
const BODY_PART: u32 = 0xa175_7f0a;
const FACE: u32 = 0x622d_d842;

/// Where a part sits in its body setup: which slot, and which variation of it.
///
/// A slot's variations are ALTERNATIVES. Every retail weight is 1.0 (a slot's weights sum to its
/// variation count), and the lists hold things no body wears at once — `FBS_CV_Elderly_F`'s upper
/// body is a trench coat, a student top, a sweater or a flower dress; its GearB is one of four scarves
/// or `FBP_A_CF_NONE` (`CH_CM_None_GR`). So a figure is one variation per slot. (The draw that picks
/// one at spawn was not traced; this is the setup's structure plus what its lists contain.)
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
    /// `Some(0)` = the part template's `HighDetailModel`, `Some(1)` = its `LowDetailModel`; `None` =
    /// any other mesh reference (an asset template's direct mesh, or a part template's other refs).
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
    /// `(slot it was listed under, message)`. Reported with the assembly instead of silently missing.
    pub missing: Vec<(Option<SetupSlot>, String)>,
}

/// What a click assembles: the meshes with their levels of detail, and anything that could not be.
#[derive(Clone, Default)]
pub struct Assembly {
    /// `(mesh_index, lod)`.
    pub picks: Vec<(usize, u8)>,
    /// Human-readable notes about references that name meshes not in this install.
    pub notes: Vec<String>,
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
    /// A one-mesh `Ungrouped` asset — a mesh no template claims (or no templates at all).
    pub fn single(mesh_index: usize, name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            category: "Ungrouped",
            parts: vec![PartRef::plain(mesh_index, name, "whole".into())],
            missing: Vec::new(),
        }
    }

    /// The parts to ASSEMBLE when this asset is clicked.
    ///
    /// A character (parts came through `FxHumanBodyPart` / `FxHumanHead` templates) assembles from
    /// the templates' own model references: one variation per setup slot ([`SetupSlot`] — the
    /// first the setup lists, head included), and of each, the `HighDetailModel` at level 0 and the
    /// `LowDetailModel` at level 1 — the same mesh again when the template names the same one, since
    /// that is what the engine draws at that distance.
    ///
    /// Anything else (weapons, props, vehicles, a name-grouped boot character) uses the older
    /// slot rules on part labels, at level 0 only.
    pub fn assembly(&self) -> Assembly {
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
            return Assembly { picks, notes };
        }
        Assembly { picks: self.label_assembly().into_iter().map(|i| (i, 0)).collect(), notes: Vec::new() }
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
        "CAR" | "TrainCarriage" | "TrainEngine" | "TrainItem" => "Vehicles",
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

/// Build the assembled-asset list from the game templates + the mesh list. Templates whose meshes
/// aren't in this megapack (world assets live in `Mega*`) resolve to nothing and are skipped; meshes
/// no asset references become one-part `Ungrouped` assets so nothing is hidden.
pub fn build(gt: &GameTemplates, mesh_list: &[MeshEntry]) -> Vec<Asset> {
    // mesh name-hash → first mesh_list index, and → its name (for labels).
    let mut mesh_idx: HashMap<u32, usize> = HashMap::new();
    let mut mesh_nm: HashMap<u32, &str> = HashMap::new();
    for (i, m) in mesh_list.iter().enumerate() {
        let h = pandemic_hash(&m.name);
        mesh_idx.entry(h).or_insert(i);
        mesh_nm.entry(h).or_insert(m.name.as_str());
    }
    // template name-hash → template (for the one-hop body-part resolution).
    let mut tpl_by_hash: HashMap<u32, &Template> = HashMap::new();
    for (_, t) in gt.templates() {
        tpl_by_hash.entry(pandemic_hash(&t.name)).or_insert(t);
    }
    // a template's directly-referenced mesh hashes (a 4-byte value that is a known mesh name-hash).
    let direct = |t: &Template| -> Vec<u32> {
        t.pairs.iter().filter_map(|p| p.as_u32()).filter(|u| mesh_idx.contains_key(u)).collect()
    };

    let mut assets: Vec<Asset> = Vec::new();
    let mut used: HashSet<u32> = HashSet::new(); // mesh name-hashes claimed by an asset

    for (_, t) in gt.templates() {
        let Some(cat) = category_for(&t.ttype) else { continue };
        let mut parts: Vec<PartRef> = Vec::new();
        let mut missing: Vec<(Option<SetupSlot>, String)> = Vec::new();
        let mut push = |parts: &mut Vec<PartRef>, u: u32, label: String, lod, template: Option<&str>, slot| {
            let key = (mesh_idx[&u], lod, template.map(str::to_string), slot);
            if parts.iter().any(|p| (p.mesh_index, p.lod, p.template.clone(), p.slot) == key) {
                return;
            }
            used.insert(u);
            parts.push(PartRef {
                mesh_index: mesh_idx[&u],
                name: mesh_nm[&u].to_string(),
                label,
                lod,
                template: template.map(str::to_string),
                slot,
            });
        };
        // The setup stream's state, as `SetProperty` keeps it (see `SLOT_KEYS`).
        let (mut body_slot, mut body_var, mut head_var): (Option<u32>, Option<u16>, Option<u16>) = (None, None, None);
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
            let Some(u) = p.as_u32() else { continue };
            if mesh_idx.contains_key(&u) {
                push(&mut parts, u, part_label(mesh_nm[&u]), None, None, None);
                continue;
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
            let high_label = high.filter(|h| mesh_idx.contains_key(h)).map(|h| part_label(mesh_nm[&h]));
            for (lvl, prop, h) in [(0u8, "HighDetailModel", high), (1u8, "LowDetailModel", low)] {
                let Some(h) = h else { continue };
                if mesh_idx.contains_key(&h) {
                    let base = alt(high_label.clone().unwrap_or_else(|| part_label(mesh_nm[&h])));
                    let label = if lvl == 0 { base } else { format!("{base} · LOD1") };
                    push(&mut parts, h, label, Some(lvl), Some(&sub.name), slot);
                } else {
                    missing.push((
                        slot,
                        format!("{}: {prop} {h:08X} names a mesh that is not in this install", sub.name),
                    ));
                }
            }
            // Its other mesh references stay listed (inspectable) but are not assembled.
            for v in direct(sub) {
                if Some(v) != high && Some(v) != low {
                    push(&mut parts, v, alt(part_label(mesh_nm[&v])), None, Some(&sub.name), slot);
                }
            }
        }
        if parts.is_empty() {
            continue; // a template with no renderable mesh in this pack — not browsable here
        }
        assets.push(Asset { name: t.name.clone(), category: cat, parts, missing });
    }

    // Everything a template didn't claim → one-part Ungrouped assets (deduped by name).
    let mut seen_ung = HashSet::new();
    for (i, m) in mesh_list.iter().enumerate() {
        let h = pandemic_hash(&m.name);
        if used.contains(&h) || !seen_ung.insert(h) {
            continue;
        }
        assets.push(Asset::single(i, &m.name));
    }
    assets
}

/// Fallback when GameTemplates can't be loaded: every mesh becomes its own `Ungrouped` asset, so the
/// browser still works (just without the assembled grouping).
pub fn build_flat(mesh_list: &[MeshEntry]) -> Vec<Asset> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (i, m) in mesh_list.iter().enumerate() {
        if !seen.insert(pandemic_hash(&m.name)) {
            continue;
        }
        out.push(Asset::single(i, &m.name));
    }
    out
}

/// Load the full `GameTemplates` object DB. The main DB is embedded in `France/loosefiles_BinPC.pack`
/// (magic `AULB`); we scan for it and parse the largest occurrence. Returns `None` if unavailable.
pub fn load_gametemplates(game_dir: &str) -> Option<GameTemplates> {
    let path = format!("{game_dir}/France/loosefiles_BinPC.pack");
    let data = std::fs::read(&path).ok()?;
    // Find every "AULB" and parse the one with the most entries (the main object DB).
    let mut best: Option<GameTemplates> = None;
    let mut best_n = 0usize;
    let mut from = 0usize;
    while let Some(rel) = data[from..].windows(4).position(|w| w == b"AULB") {
        let off = from + rel;
        if let Ok((gt, _)) = GameTemplates::parse(&data[off..]) {
            let n = gt.templates().count();
            if n > best_n {
                best_n = n;
                best = Some(gt);
            }
        }
        from = off + 4;
    }
    best
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
        ] {
            assert_eq!(pandemic_hash(name), key, "{name}");
        }
    }

    /// Every retail character assembles as ONE variation per setup slot: every `BodyPart` / `Face`
    /// sits inside a variation of a slot, no slot contributes two part templates at level 0, and
    /// every slot whose first variation names a mesh this install has contributes it.
    #[test]
    fn retail_characters_assemble_one_variation_per_slot() {
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let pack = crate::pack::Megapack::open(&s.megapack()).expect("megapack");
        let list = crate::meshload::list_meshes(&pack);
        let gt = load_gametemplates(&s.game_dir).expect("GameTemplates");
        let assets = build(&gt, &list);
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
                    bad.push(format!("{}: {} (LOD{lod}) is not from a shown variation", a.name, list[i].name));
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
        assert!(chars > 0, "no characters assembled from this install");
        assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));

        // The case that exposed this: four alternative upper bodies, one figure.
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
}
