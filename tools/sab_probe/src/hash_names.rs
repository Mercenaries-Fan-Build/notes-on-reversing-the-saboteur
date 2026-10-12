//! `hash-names <retail_dir> <Saboteur.exe> <proto2008_dir> <community_tools_dir>
//! <community_research_dir>` — which `pandemic_hash` values the retail game uses, and which
//! plain-text names from the game, the 2008 prototype and community files hash to them.
//!
//! A **target** is a hash the data uses as a key: megapack `crc` / `index`, ALBS record hashes, map
//! record and tile hashes, EditNodes container / object / param tags, GameTemplates property keys and
//! 4-byte values, GameText UI ids and scene hashes. A **candidate** is a name from some source; it
//! names a target when `pandemic_hash(name)` equals it.
//!
//! stdout: `data/hash_names.tsv` — one row per (hash, name), `hash  name  grade  chance  kinds
//! sources`, sorted by hash then name. stderr: targets and named targets per kind, candidates and hits
//! per source, the chance estimate per source and kind, the rows kept by grade, and every row dropped.
//! Grades and the chance estimate: `docs/hash_names.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sab_formats::editnodes::{self, LuaValue, Node, Root, Value};
use sab_formats::gametemplates::GameTemplates;
use sab_formats::gametext::GameText;
use sab_formats::{dtex, loosefiles, map6, megapack, mesh, pandemic_hash, sbla};

#[derive(Default)]
struct Targets {
    kinds: BTreeMap<u32, BTreeSet<&'static str>>,
}

impl Targets {
    fn add(&mut self, h: u32, kind: &'static str) {
        if h != 0 {
            self.kinds.entry(h).or_default().insert(kind);
        }
    }
}

struct Names<'a> {
    targets: &'a Targets,
    /// hash -> name -> sources
    hits: BTreeMap<u32, BTreeMap<String, BTreeSet<&'static str>>>,
    /// source -> (candidates offered, candidates that named a target)
    per_source: BTreeMap<&'static str, (usize, usize)>,
    /// source -> distinct candidate hashes
    distinct: BTreeMap<&'static str, std::collections::HashSet<u32>>,
    /// Names of a target that contain a tab or line break, which a TSV row cannot hold.
    unrepresentable: Vec<String>,
}

impl Names<'_> {
    fn offer(&mut self, name: &str, source: &'static str) {
        if name.is_empty() {
            return;
        }
        let s = self.per_source.entry(source).or_default();
        s.0 += 1;
        let h = pandemic_hash(name);
        self.distinct.entry(source).or_default().insert(h);
        if self.targets.kinds.contains_key(&h) {
            if name.contains(['\t', '\n', '\r']) {
                self.unrepresentable.push(format!("0x{h:08x} {name:?} ({source})"));
                return;
            }
            s.1 += 1;
            self.hits.entry(h).or_default().entry(name.to_string()).or_default().insert(source);
        }
    }

    /// Whole printable runs as `whole`; identifier and phrase tokens as `token`; a token's short
    /// suffixes and its name with an `m_` / `s_` / `g_` / lowercase type prefix dropped as `variant`.
    fn offer_text(&mut self, b: &[u8], whole: &'static str, token: &'static str, variant: &'static str) {
        for s in runs(b, 3) {
            self.offer(&s, whole);
        }
        for t in tokens(b) {
            self.offer(&t, token);
            for v in variants(&t) {
                self.offer(&v, variant);
            }
        }
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Maximal identifiers (`[A-Za-z_][A-Za-z0-9_]{2,63}`) and maximal phrases
/// (`[A-Za-z][A-Za-z0-9_ ]{2,63}`, trailing spaces trimmed).
fn tokens(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (start_ok, cont) in [(is_ident_start as fn(u8) -> bool, is_ident as fn(u8) -> bool), (|c: u8| c.is_ascii_alphabetic(), |c: u8| is_ident(c) || c == b' ')] {
        let mut i = 0;
        while i < b.len() {
            if start_ok(b[i]) && (i == 0 || !cont(b[i - 1])) {
                let mut j = i;
                while j < b.len() && cont(b[j]) {
                    j += 1;
                }
                let s = std::str::from_utf8(&b[i..j]).expect("ASCII").trim_end();
                if (3..=64).contains(&s.len()) {
                    out.push(s.to_string());
                }
                i = j;
            } else {
                i += 1;
            }
        }
    }
    out
}

/// A token's suffixes dropping 1..=5 leading bytes, and the name behind an `m_`/`s_`/`g_` prefix or a
/// one- or two-letter lowercase type prefix (`bEnabled` -> `Enabled`).
fn variants(t: &str) -> Vec<String> {
    let mut v: Vec<String> = (1..t.len().min(6)).filter(|&i| t.len() - i >= 3).map(|i| t[i..].to_string()).collect();
    for p in ["m_", "s_", "g_"] {
        if let Some(rest) = t.strip_prefix(p) {
            v.push(rest.to_string());
            let mut c = rest.chars();
            if let (Some(a), Some(b)) = (c.next(), c.next()) {
                if a.is_ascii_alphabetic() && b.is_ascii_uppercase() {
                    v.push(rest[1..].to_string());
                }
            }
        }
    }
    let lower = t.bytes().take_while(|c| c.is_ascii_lowercase()).count();
    if (1..=2).contains(&lower) && t.as_bytes().get(lower).is_some_and(|c| c.is_ascii_uppercase()) {
        v.push(t[lower..].to_string());
    }
    v
}

/// The text between matching double or single quotes on one line.
fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in s.lines() {
        for q in ['"', '\''] {
            let parts: Vec<&str> = line.split(q).collect();
            for p in parts.iter().skip(1).step_by(2).take(parts.len() / 2) {
                out.push(p.to_string());
            }
        }
    }
    out
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let p = e.expect("dir entry").path();
        if p.is_dir() {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Printable-ASCII runs of at least `min` bytes.
fn runs(b: &[u8], min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &c) in b.iter().chain(std::iter::once(&0)).enumerate() {
        if (0x20..0x7f).contains(&c) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            if i - s >= min {
                out.push(String::from_utf8_lossy(&b[s..i]).into_owned());
            }
        }
    }
    out
}

/// The object nodes of an entry root.
fn editnode_objects<'a>(r: &'a Root, out: &mut Vec<&'a Node>) {
    match r {
        Root::EditNode { objects } => out.extend(objects.iter()),
        Root::Container { children, .. } => children.iter().for_each(|c| editnode_objects(c, out)),
    }
}

struct Retail {
    globals: Vec<map6::GlobalMap>,
    regions: Vec<map6::RegionMap>,
    editnodes: Vec<editnodes::Pack>,
    templates: Vec<GameTemplates>,
    gametext: GameText,
    luap: Vec<u8>,
    loose_names: Vec<String>,
}

fn load(retail: &Path) -> Retail {
    let lf = std::fs::read(retail.join("France/loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let get = |n: &str| lf[loosefiles::find(&es, n).unwrap_or_else(|| panic!("{n} not in loosefiles")).data.clone()].to_vec();
    let read = |p: &str| std::fs::read(retail.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let gt = |b: &[u8], what: &str| GameTemplates::parse(b).unwrap_or_else(|e| panic!("{what}: {e}")).0;
    Retail {
        globals: vec![
            map6::parse_global(&get("global.map")).expect("global.map"),
            map6::parse_global(&read("DLC/01/Global.map")).expect("DLC Global.map"),
        ],
        regions: vec![
            map6::parse_region_main(&get("France.map")).expect("France.map"),
            map6::parse_region_dlc(&read("DLC/01/FRANCE.map")).expect("DLC FRANCE.map"),
        ],
        editnodes: vec![
            editnodes::Pack::parse(&get(r"France\EditNodes\EditNodes.pack")).expect("EditNodes.pack"),
            editnodes::Pack::parse(&read("DLC/01/France/EditNodes/EditNodes.pack")).expect("DLC EditNodes.pack"),
        ],
        templates: vec![gt(&get("GameTemplates.wsd"), "GameTemplates.wsd"), gt(&read("DLC/01/GameTemplates.wsd"), "DLC GameTemplates.wsd")],
        gametext: GameText::parse(&read("Cinematics/Dialog/English/GameText.dlg")).expect("GameText.dlg"),
        luap: read("LuaScripts.luap"),
        loose_names: es.iter().map(|e| e.name.clone()).collect(),
    }
}

/// Every ALBS sub-pack in the install with the map counts it needs, as (label, region, entry, bytes).
fn each_albs(retail: &Path, r: &Retail, mut f: impl FnMut(Option<&megapack::Entry>, Option<&str>, &[u8], Option<[u32; 9]>)) {
    let g: Vec<&map6::GlobalMap> = r.globals.iter().collect();
    let rg: Vec<&map6::RegionMap> = r.regions.iter().collect();
    let by_path = map6::pack_counts(&g, &rg).expect("pack counts");
    let by_hash = map6::object_counts(&g, &rg).expect("object counts");
    let region = String::from_utf8(r.regions[0].region.clone()).expect("region name");
    let mut all = Vec::new();
    files(retail, &mut all);
    all.sort();
    for p in &all {
        let rel = p.strip_prefix(retail).expect("under retail").to_string_lossy().replace('\\', "/");
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "megapack" || ext == "kiloPack" {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let mp = megapack::parse(&buf).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let reg = (rel.to_lowercase().starts_with("france/mega") || rel.to_lowercase().starts_with("dlc/01/dlc01mega"))
                .then_some(region.as_str());
            for e in &mp.entries {
                let sub = megapack::entry_slice(&buf, e).unwrap_or_else(|err| panic!("{rel}: {err}"));
                let counts = if ext == "kiloPack" { by_hash.get(&e.index) } else { by_path.get(&e.crc) };
                f(Some(e), reg, sub, counts.copied());
            }
        } else if (ext == "dynpack" || ext == "pack") && rel.starts_with("DLC/") {
            let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            f(None, None, &buf, by_hash.get(&pandemic_hash(stem)).copied());
        }
    }
}

pub fn run(args: &[String]) {
    let [retail, exe, proto, community, research] = args else {
        eprintln!(
            "usage: sab_probe hash-names <retail_dir> <Saboteur.exe> <proto2008_dir> <community_tools_dir> \
             <community_research_dir> > hash_names.tsv"
        );
        std::process::exit(2);
    };
    let (retail, exe, proto, community, research) =
        (Path::new(retail), Path::new(exe), Path::new(proto), Path::new(community), Path::new(research));
    let mut r = load(retail);

    // ---- targets
    let mut t = Targets::default();
    let mut blob_names: Vec<(&'static str, String)> = Vec::new();
    let mut streamblock_candidates: Vec<String> = Vec::new();
    let mut non_albs: BTreeMap<String, usize> = BTreeMap::new();
    let mut embedded_editnodes: Vec<editnodes::Pack> = Vec::new();
    each_albs(retail, &r, |e, region, sub, counts| {
        let albs = sub.get(0..4) == Some(sbla::MAGIC);
        let x = if albs { sbla::descriptor_len(sub).expect("ALBS descriptor length") } else { 0 };
        if let Some(e) = e {
            t.add(e.crc, "megapack.crc");
            match region {
                Some(region) if albs && x != 0 => {
                    streamblock_candidates.push(format!("{}.pack", map6::streamblock_path(region, e.index)))
                }
                _ => t.add(e.index, "megapack.index"),
            }
        }
        if !albs {
            *non_albs.entry(format!("{:02x?}", &sub[..sub.len().min(4)])).or_default() += 1;
            if sub.get(0..4) == Some(b"00ED") {
                embedded_editnodes.push(editnodes::Pack::parse(sub).expect("EditNodes pack in a megapack"));
            }
            return;
        }
        let sp = sbla::parse(sub, if x == 0 { Some(counts.expect("ALBS map counts")) } else { None }).expect("ALBS parse");
        for (rec, blob) in sp.records.iter().zip(&sp.blobs) {
            t.add(rec.hash, ["albs.type0", "albs.type1", "albs.type2", "albs.type3", "albs.type4", "albs.type5", "albs.type6", "albs.type7", "albs.type8"][rec.kind]);
            match rec.kind {
                0 => blob_names.push(("albs.msha-name", mesh::parse_msha(blob).expect("MSHA").name)),
                1 if !blob.is_empty() => {
                    let d = dtex::parse(blob).expect("DTEX");
                    blob_names.push(("albs.dtex-name", d.name.trim_end_matches('\0').to_string()));
                }
                _ => {}
            }
        }
    });
    eprintln!("EditNodes packs inside megapacks: {}", embedded_editnodes.len());
    r.editnodes.extend(embedded_editnodes);
    for g in &r.globals {
        for rec in g.list1.iter().chain(&g.list2).chain(&g.dynamics) {
            t.add(rec.hash, "map.record");
        }
    }
    for rm in &r.regions {
        for rec in rm.list2.iter().chain(&rm.list3) {
            t.add(rec.hash, "map.record");
        }
        for tile in &rm.tiles {
            t.add(tile.hash, "map.tile");
        }
    }
    // object tag -> hashes of the entries (containers) that hold it
    let mut object_containers: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    for pack in &r.editnodes {
        for e in &pack.entries {
            t.add(e.hash, "editnodes.container");
            let mut objs = Vec::new();
            editnode_objects(&e.root, &mut objs);
            for o in objs {
                t.add(o.tag(), "editnodes.object");
                object_containers.entry(o.tag()).or_default().insert(e.hash);
                if let Node::Container { children, .. } = o {
                    for c in children {
                        t.add(c.tag(), "editnodes.param");
                        if let Some(Value::Lua(p)) = c.value() {
                            if let LuaValue::Crc(h) = p.value {
                                t.add(h, "editnodes.luacrc");
                            }
                        }
                    }
                }
            }
        }
    }
    for gt in &r.templates {
        for (_, tpl) in gt.templates() {
            for pair in &tpl.pairs {
                t.add(pair.hash, "gametemplates.property");
                if let Some(v) = pair.as_u32() {
                    t.add(v, "gametemplates.value");
                }
            }
        }
    }
    for rec in r.gametext.records.iter().filter(|rec| rec.is_ui()) {
        t.add(rec.asset_id, "gametext.ui-id");
    }
    for g in r.gametext.dnec_groups() {
        t.add(g.scene_hash, "gametext.scene");
    }

    // ---- candidates
    let mut n = Names {
        targets: &t,
        hits: BTreeMap::new(),
        per_source: BTreeMap::new(),
        distinct: BTreeMap::new(),
        unrepresentable: Vec::new(),
    };
    for (src, name) in &blob_names {
        n.offer(name, src);
    }
    for s in &streamblock_candidates {
        n.offer(s, "map.streamblock-path");
    }
    let g: Vec<&map6::GlobalMap> = r.globals.iter().collect();
    let rg: Vec<&map6::RegionMap> = r.regions.iter().collect();
    for (path, _) in map6::pack_paths(&g, &rg).expect("pack paths") {
        n.offer(&path, "map.pack-path");
    }
    for name in &r.loose_names {
        n.offer(name, "loosefiles.name");
    }
    for gm in &r.globals {
        for rec in &gm.list1 {
            n.offer(&rec.name(), "map.name");
            n.offer(&format!("{}0", rec.name()), "map.name+list");
        }
        for rec in &gm.list2 {
            n.offer(&rec.name(), "map.name");
            n.offer(&format!("{}1", rec.name()), "map.name+list");
        }
        for rec in &gm.dynamics {
            n.offer(&rec.name(), "map.name");
        }
    }
    for rm in &r.regions {
        for rec in rm.list2.iter().chain(&rm.list3) {
            n.offer(&rec.name(), "map.name");
        }
        for tile in &rm.tiles {
            let end = tile.name.iter().position(|&c| c == 0).unwrap_or(tile.name.len());
            n.offer(&String::from_utf8_lossy(&tile.name[..end]), "map.name");
        }
    }
    // EditNodes strings, and for a backslash path the container and object forms the engine hashes.
    let mut en_strings: Vec<(&'static str, String)> = Vec::new();
    for pack in &r.editnodes {
        for e in &pack.entries {
            let mut objs = Vec::new();
            editnode_objects(&e.root, &mut objs);
            for o in objs {
                let Node::Container { children, .. } = o else { continue };
                for c in children {
                    match c.value() {
                        Some(Value::Str(s)) => en_strings.push(("editnodes.string", s)),
                        Some(Value::Lua(p)) => {
                            en_strings.push(("editnodes.lua-name", p.name));
                            match p.value {
                                LuaValue::Str(s) => en_strings.push(("editnodes.lua-string", s)),
                                LuaValue::CrcList(v) => v.into_iter().for_each(|s| en_strings.push(("editnodes.lua-crclist", s))),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    for (src, s) in &en_strings {
        n.offer(s, src);
        if let Some((parent, _)) = s.rsplit_once('\\') {
            n.offer(&format!("{}{parent}.wsd", editnodes::CONTAINER_PREFIX), "editnodes.derived-container");
        }
    }
    for gt in &r.templates {
        for (_, tpl) in gt.templates() {
            n.offer(&tpl.name, "gametemplates.name");
            n.offer(&tpl.ttype, "gametemplates.type");
            for pair in &tpl.pairs {
                if pair.data.len() > 1 && pair.data.last() == Some(&0) && pair.data[..pair.data.len() - 1].iter().all(|b| (0x20..0x7f).contains(b)) {
                    n.offer(&String::from_utf8_lossy(&pair.data[..pair.data.len() - 1]), "gametemplates.string");
                }
            }
        }
    }
    let props = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/gametemplates_properties.tsv");
    for line in std::fs::read_to_string(&props).unwrap_or_else(|e| panic!("{}: {e}", props.display())).lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if line.starts_with('#') || cols.first() == Some(&"key") || cols.len() < 3 || cols[1].is_empty() {
            continue;
        }
        let src = match cols[2] {
            "dict" => "gametemplates_properties.tsv:dict",
            "exe" => "gametemplates_properties.tsv:exe",
            "sym" => "gametemplates_properties.tsv:sym",
            other => panic!("{}: unknown name_src {other:?}", props.display()),
        };
        n.offer(cols[1], src);
    }
    n.offer_text(&r.luap, "luap.string", "luap.token", "luap.variant");
    let exe_bytes = std::fs::read(exe).unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
    n.offer_text(&exe_bytes, "exe.string", "exe.token", "exe.variant");
    // Names the engine builds with a two-digit counter (`sprintf("Node%.2d", i)` and kin).
    for s in runs(&exe_bytes, 4) {
        if s.matches("%.2d").count() == 1 && s.matches('%').count() == 1 {
            for i in 0..100 {
                n.offer(&s.replace("%.2d", &format!("{i:02}")), "engine-format.exe");
            }
        }
    }
    let mut proto_files: Vec<PathBuf> = std::fs::read_dir(proto)
        .unwrap_or_else(|e| panic!("{}: {e}", proto.display()))
        .map(|d| d.expect("proto entry").path())
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("pdb" | "map")))
        .collect();
    proto_files.sort();
    eprintln!("2008 prototype symbol/map files: {:?}", proto_files.iter().map(|p| p.file_name().expect("name")).collect::<Vec<_>>());
    for p in &proto_files {
        let b = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        n.offer_text(&b, "proto2008.string", "proto2008.token", "proto2008.variant");
    }
    let mut research_files = Vec::new();
    files(research, &mut research_files);
    research_files.sort();
    for p in &research_files {
        let b = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let s = String::from_utf8_lossy(&b);
        for q in quoted(&s) {
            n.offer(&q, "community-research.quoted");
        }
        for line in s.lines() {
            n.offer(line.trim(), "community-research.line");
        }
        for t in tokens(&b) {
            n.offer(&t, "community-research.token");
        }
    }
    let toolset = community.join("SaboteurToolset-master/saboteur_strings.txt");
    for line in std::fs::read_to_string(&toolset).unwrap_or_else(|e| panic!("{}: {e}", toolset.display())).lines() {
        n.offer(line.trim_end_matches('\r'), "saboteurtoolset.strings");
    }
    let sabtool = community.join("SabTool-master/src/SabTool.Utils/Hashes.txt");
    for line in std::fs::read_to_string(&sabtool).unwrap_or_else(|e| panic!("{}: {e}", sabtool.display())).lines() {
        let line = line.trim_end_matches('\r');
        if let Some((_, name)) = line.split_once(':') {
            n.offer(name, "sabtool.hashes");
        }
    }

    // ---- report
    let mut per_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for (h, kinds) in &t.kinds {
        for k in kinds {
            let e = per_kind.entry(k).or_default();
            e.0 += 1;
            e.1 += n.hits.contains_key(h) as usize;
        }
    }
    eprintln!("non-ALBS sub-packs by first four bytes: {non_albs:?}");
    eprintln!("targets: {} distinct hashes; named: {}", t.kinds.len(), n.hits.len());
    eprintln!("by kind -> (targets, named):");
    for (k, v) in &per_kind {
        eprintln!("  {k:<28} {v:?}");
    }
    eprintln!("by source: offers, distinct candidate hashes, hits, chance hits expected (distinct x targets / 2^32):");
    let targets = t.kinds.len() as f64;
    for (k, (offers, hits)) in &n.per_source {
        let d = n.distinct[k].len();
        eprintln!("  {k:<32} {offers:>9} {d:>9} {hits:>9} {:>10.1}", d as f64 * targets / 4294967296.0);
    }
    let mut multi = 0usize;
    let mut multi_examples = Vec::new();
    for (h, names) in &n.hits {
        let folded: BTreeSet<String> = names.keys().map(|s| s.to_lowercase()).collect();
        if folded.len() > 1 {
            multi += 1;
            if multi_examples.len() < 30 {
                multi_examples.push(format!("0x{h:08x}: {:?}", names.keys().collect::<Vec<_>>()));
            }
        }
    }
    eprintln!("names of a target with a tab or line break (not in the table): {}", n.unrepresentable.len());
    for e in &n.unrepresentable {
        eprintln!("  {e}");
    }
    eprintln!("hashes with more than one distinct name (case folded): {multi}");
    for e in &multi_examples {
        eprintln!("  {e}");
    }
    // ---- merge spellings, grade
    // `pandemic_hash` folds ASCII case, so names that differ only in case are one name: merge their
    // sources and keep the spelling a game-data source gives, else the one with the most sources.
    let mut merged: BTreeMap<u32, BTreeMap<String, (String, BTreeSet<&'static str>)>> = BTreeMap::new();
    for (h, names) in &n.hits {
        let by_fold = merged.entry(*h).or_default();
        let mut spellings: Vec<(&String, &BTreeSet<&'static str>)> = names.iter().collect();
        spellings.sort_by_key(|(name, sources)| {
            (!sources.iter().any(|s| DATA_FAMILIES.contains(&family(s))), std::cmp::Reverse(sources.len()), name.to_string())
        });
        for (name, sources) in spellings {
            let e = by_fold.entry(name.to_lowercase()).or_insert_with(|| (name.clone(), BTreeSet::new()));
            e.1.extend(sources.iter().copied());
        }
    }
    // Chance share of a source on a kind: the chance hits expected from it on that kind's targets
    // (distinct candidates x targets of the kind / 2^32) over the rows of that kind only its own
    // family names, which is where its chance hits land. `gametemplates_properties.tsv` rows were
    // harvested from a corpus by the same cut-and-variant method as a `.variant` source, so they take
    // that source's candidate count.
    let mut kind_size: BTreeMap<&str, usize> = BTreeMap::new();
    for kinds in t.kinds.values() {
        for k in kinds {
            *kind_size.entry(k).or_default() += 1;
        }
    }
    let mut text_rows: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (h, names) in &merged {
        for (_, sources) in names.values() {
            let fs: BTreeSet<&str> = sources.iter().map(|s| family(s)).collect();
            if fs.len() == 1 && !fs.iter().any(|f| DATA_FAMILIES.contains(f)) {
                for s in sources {
                    for k in &t.kinds[h] {
                        *text_rows.entry((s, k)).or_default() += 1;
                    }
                }
            }
        }
    }
    let chance = |s: &str, k: &str| -> f64 {
        let est = n.distinct[candidates_of(s)].len() as f64 * kind_size[k] as f64 / 4294967296.0;
        est / text_rows.get(&(s, k)).copied().unwrap_or(0).max(1) as f64
    };
    let row_chance = |h: u32, sources: &BTreeSet<&'static str>| -> f64 {
        sources.iter().flat_map(|s| t.kinds[&h].iter().map(move |k| (s, k))).map(|(s, k)| chance(s, k)).fold(f64::INFINITY, f64::min)
    };
    eprintln!("single-family non-data rows by source and kind: rows, estimated chance share:");
    for ((s, k), rows) in &text_rows {
        eprintln!("  {s:<36} {k:<24} {rows:>7} {:>8.2}%", 100.0 * chance(s, k));
    }
    // A name of an EditNodes object whose parent path hashes, as a container, to an entry holding
    // that object is `structural`; a name with a game-data source is `data`; else `text`, kept when
    // a source's chance share is <= 10% or sources of two families offer it whole (a `.variant` is a
    // fragment we cut, not a string found as such).
    let structural = |h: u32, name: &str| -> bool {
        match (object_containers.get(&h), name.rsplit_once('\\')) {
            (Some(cs), Some((parent, _))) => cs.contains(&editnodes::container_hash(parent)),
            _ => false,
        }
    };
    let mut out: Vec<(u32, String, &str, f64, String)> = Vec::new();
    let mut dropped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut struct_by_source: BTreeMap<&str, usize> = BTreeMap::new();
    for (h, names) in &merged {
        for (name, sources) in names.values() {
            let best = row_chance(*h, sources);
            let grade = if structural(*h, name) {
                for s in sources {
                    *struct_by_source.entry(s).or_default() += 1;
                }
                "structural"
            } else if sources.iter().any(|s| DATA_FAMILIES.contains(&family(s))) {
                "data"
            } else if best <= 0.10
                || sources.iter().filter(|s| !candidates_of(s).ends_with(".variant")).map(|s| family(s)).collect::<BTreeSet<_>>().len() >= 2
            {
                "text"
            } else {
                let key = sources.iter().copied().collect::<Vec<_>>().join(",");
                let kinds = t.kinds[h].iter().copied().collect::<Vec<_>>().join("+");
                dropped.entry(key).or_default().push(format!("0x{h:08x} {name:?} [{kinds}]"));
                continue;
            };
            out.push((*h, name.clone(), grade, best, sources.iter().copied().collect::<Vec<_>>().join(",")));
        }
    }
    let mut per_grade: BTreeMap<&str, usize> = BTreeMap::new();
    for row in &out {
        *per_grade.entry(row.2).or_default() += 1;
    }
    let kept: BTreeSet<u32> = out.iter().map(|r| r.0).collect();
    let mut ambiguous = 0usize;
    for h in &kept {
        if out.iter().filter(|r| r.0 == *h).count() > 1 {
            ambiguous += 1;
        }
    }
    eprintln!("rows kept by grade: {per_grade:?}; distinct hashes named: {}; with more than one name: {ambiguous}", kept.len());
    let expected: f64 = out.iter().filter(|r| r.2 == "text").map(|r| r.3.min(1.0)).sum();
    eprintln!("chance rows expected among the text rows (sum of their chance, each capped at 1): {expected:.1}");
    eprintln!("structural object names, by source: {struct_by_source:?}");
    eprintln!("rows dropped, by sources: {}", dropped.values().map(Vec::len).sum::<usize>());
    for (k, v) in &dropped {
        eprintln!("  {k}: {}", v.len());
        for row in v {
            eprintln!("    {row}");
        }
    }

    println!("hash\tname\tgrade\tchance\tkinds\tsources");
    for (h, name, grade, best, sources) in &out {
        let kinds = t.kinds[h].iter().copied().collect::<Vec<_>>().join(",");
        let best = if *grade == "text" { format!("{:.4}", best.min(1.0)) } else { "-".to_string() };
        println!("0x{h:08x}\t{name}\t{grade}\t{best}\t{kinds}\t{sources}");
    }
}

/// The source whose candidate count stands for `source` in the chance estimate.
fn candidates_of(source: &str) -> &str {
    match source {
        "gametemplates_properties.tsv:sym" => "proto2008.variant",
        "gametemplates_properties.tsv:exe" => "exe.variant",
        "gametemplates_properties.tsv:dict" => "community-research.quoted",
        s => s,
    }
}

/// Sources whose names are strings stored in the game data, or built by the engine.
const DATA_FAMILIES: [&str; 6] = ["albs", "map", "editnodes", "gametemplates", "loosefiles", "engine-format"];

/// A source's family: the part before the first `.`, except that `gametemplates_properties.tsv`
/// rows belong to the family their `name_src` was harvested from.
fn family(source: &str) -> &str {
    match source {
        "gametemplates_properties.tsv:sym" => "proto2008",
        "gametemplates_properties.tsv:exe" => "exe",
        "gametemplates_properties.tsv:dict" => "community-research",
        s => s.split('.').next().expect("split yields one part"),
    }
}
