//! `template-lod-damage <templates>` — over a templates file (retail `France/loosefiles_BinPC.pack`,
//! whose largest `AULB` table is read, or the 2008 `Bin360/GameTemplates.wsd`), count per template
//! type the `CombinedLODModel` keys and how many name a model, and the damage parts (`BoneName`
//! starts one) by how they state the group state they draw in: retail `Pristine`, 2008 `0xb7b68d44`,
//! or neither. docs/formats/gametemplates.md, docs/damage_and_destruction.md.

use std::collections::BTreeMap;

use sab_formats::gametemplates::GameTemplates;
use sab_formats::pandemic_hash;

const BONE_NAME: u32 = 0x207f_ba52;
const PRISTINE: u32 = 0x86de_6639;
const PART_STATE: u32 = 0xb7b6_8d44;

fn load(path: &str) -> GameTemplates {
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    if b.starts_with(b"BLU2") || b.starts_with(b"AULB") {
        return GameTemplates::parse(&b).unwrap_or_else(|e| panic!("{path}: {e}")).0;
    }
    let mut best: Option<GameTemplates> = None;
    let mut from = 0;
    while let Some(rel) = b[from..].windows(4).position(|w| w == b"AULB") {
        let off = from + rel;
        if let Ok((gt, _)) = GameTemplates::parse(&b[off..]) {
            if best.as_ref().map_or(true, |x| gt.templates().count() > x.templates().count()) {
                best = Some(gt);
            }
        }
        from = off + 4;
    }
    best.unwrap_or_else(|| panic!("{path}: no AULB table parses"))
}

pub fn run(args: &[String]) {
    let [path] = args else {
        eprintln!("usage: sab_probe template-lod-damage <loosefiles_BinPC.pack | GameTemplates.wsd>");
        std::process::exit(2);
    };
    let gt = load(path);
    println!("{path}: {:?}, {} templates", gt.order, gt.templates().count());
    let combined = pandemic_hash("CombinedLODModel");
    let mut clod: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    // type -> (parts, Pristine, 0xb7b68d44, neither)
    let mut parts: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    let mut stateless: Vec<&str> = Vec::new();
    for (_, t) in gt.templates() {
        if let Some(p) = t.pair(combined) {
            let e = clod.entry(t.ttype.as_str()).or_default();
            e.0 += 1;
            if p.as_u32().is_some_and(|v| v != 0) {
                e.1 += 1;
            }
        }
        // Each part as [has Pristine, has 0xb7b68d44].
        let mut own: Vec<[bool; 2]> = Vec::new();
        for p in &t.pairs {
            match (p.hash, own.last_mut()) {
                (BONE_NAME, _) => own.push([false, false]),
                (PRISTINE, Some(c)) => c[0] = true,
                (PART_STATE, Some(c)) => c[1] = true,
                _ => {}
            }
        }
        for [pr, st] in &own {
            let e = parts.entry(t.ttype.as_str()).or_default();
            e[0] += 1;
            e[1] += *pr as usize;
            e[2] += *st as usize;
            e[3] += (!pr && !st) as usize;
        }
        if own.iter().any(|[pr, st]| !pr && !st) {
            stateless.push(t.name.as_str());
        }
    }
    println!("\nCombinedLODModel by type: carrying the key, naming a model");
    for (ty, (with, named)) in &clod {
        println!("  {ty:24} {with:6} {named:6}");
    }
    println!("\ndamage parts by type: parts, with Pristine, with 0xb7b68d44, with neither");
    for (ty, [n, pr, st, none]) in &parts {
        println!("  {ty:24} {n:6} {pr:6} {st:6} {none:6}");
    }
    println!("\ntemplates with a part stating neither: {}", stateless.len());
    for name in stateless.iter().take(20) {
        println!("  {name}");
    }
}
