//! Game gate for `proto2008` and `xex` over the 2008 prototype named by `proto2008_dir` in the repo-root
//! `.sab-local.toml`: every `.pack` is an MP00 archive whose entries tile it and are byte copies of the
//! loose files they hash the paths of, an `SBL*` sub-pack, `AP04`, or empty; every `meshes` and
//! `textures` slot directory reads; every `meshes` record parses; every `textures` record parses, is
//! keyed by its name's hash, inflates to `unpacked` bytes that its mip layout spans exactly, and every
//! level reads; every `materials` slot parses and names every material its pack's mesh primitives and
//! variant bindings use; every `wsd` slot is a big-endian EditNodes entry; every `.shaders` file parses
//! with a constant table in every variant; every `.xex` unwraps. Every `ANM0` clip, loose and in
//! `Animations.pack`, parses to its end and decodes, and samples wherever the engine's sampler accepts
//! it; the pack reads to its end.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_proto2008_bin360;
use sab_formats::proto2008::anim::{self, Codec};
use sab_formats::proto2008::sbl::{self, SlotKind};
use sab_formats::proto2008::{anim_pack, material, mesh, mp00, shaders, slot_dir, texture};
use sab_formats::{editnodes, pandemic_hash, xex, zlib};

fn bin360() -> PathBuf {
    local_config_proto2008_bin360(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
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

#[derive(Default)]
struct Counts {
    sbl: usize,
    mesh_records: usize,
    variant_records: usize,
    wsd_slots: usize,
    textures: usize,
    material_slots: usize,
    materials: usize,
}

fn check_sbl(buf: &[u8], at: &str, c: &mut Counts, failures: &mut Vec<String>) {
    let sp = match sbl::parse(buf) {
        Ok(sp) => sp,
        Err(e) => return failures.push(format!("{at}: {e}")),
    };
    c.sbl += 1;
    let mut uids = HashSet::new();
    if let Some(slot) = sp.slot(SlotKind::Materials) {
        match material::parse(slot) {
            Ok(ms) => {
                c.material_slots += 1;
                c.materials += ms.len();
                uids.extend(ms.iter().map(|m| m.uid));
            }
            Err(e) => failures.push(format!("{at} materials: {e}")),
        }
    }
    for (r, slot) in sp.records.iter().zip(&sp.slots) {
        match r.kind {
            SlotKind::Textures => {
                let recs = match slot_dir::parse(slot) {
                    Ok(r) => r,
                    Err(e) => return failures.push(format!("{at} textures: {e}")),
                };
                for (i, rec) in recs.iter().filter(|r| r.stored != 0).enumerate() {
                    let d = &slot[rec.offset as usize..(rec.offset + rec.stored) as usize];
                    let checked = texture::parse(d).and_then(|t| {
                        let h = texture::parse_header(&d[..d.len().min(texture::MAX_HEADER)])?;
                        if (h.name.as_str(), h.format, h.width, h.height, h.mips) != (t.name.as_str(), t.format, t.width, t.height, t.mips) {
                            return Err(format!("'{}': the header read from a prefix differs from the record's", t.name));
                        }
                        if pandemic_hash(&t.name) != rec.hash {
                            return Err(format!("record hash {:08X} is not pandemic_hash('{}')", rec.hash, t.name));
                        }
                        let memory = t.memory()?;
                        let (levels, end) = texture::levels(&t)?;
                        if end != memory.len() {
                            return Err(format!("'{}': mip layout spans {end} bytes, image is {}", t.name, memory.len()));
                        }
                        levels.iter().try_for_each(|l| texture::level_bytes(&t, &memory, l).map(drop))
                    });
                    match checked {
                        Ok(()) => c.textures += 1,
                        Err(e) => failures.push(format!("{at} texture {i}: {e}")),
                    }
                }
            }
            SlotKind::Meshes => {
                let recs = match slot_dir::parse(slot) {
                    Ok(r) => r,
                    Err(e) => return failures.push(format!("{at} meshes: {e}")),
                };
                for (i, rec) in recs.iter().filter(|r| r.stored != 0).enumerate() {
                    let d = &slot[rec.offset as usize..(rec.offset + rec.stored) as usize];
                    let compressed = rec.stored < rec.unpacked;
                    let prefix = if compressed { &d[..d.len().min(mesh::NAME_PREFIX)] } else { d };
                    let early_name = match mesh::record_name(prefix, compressed) {
                        Ok(n) => n,
                        Err(e) => {
                            failures.push(format!("{at} mesh {i} name: {e}"));
                            continue;
                        }
                    };
                    let body = if rec.stored < rec.unpacked {
                        match zlib::inflate_exact(d, rec.unpacked as usize, "mesh") {
                            Ok(b) => b,
                            Err(e) => {
                                failures.push(format!("{at} mesh {i}: {e}"));
                                continue;
                            }
                        }
                    } else {
                        d.to_vec()
                    };
                    let rec = match mesh::parse_record(&body) {
                        Ok(r) => r,
                        Err(e) => {
                            failures.push(format!("{at} mesh {i}: {e}"));
                            continue;
                        }
                    };
                    if rec.mesh().name != early_name {
                        failures.push(format!("{at} mesh {i}: record_name '{early_name}' != body name '{}'", rec.mesh().name));
                    }
                    match &rec {
                        mesh::Record::Mesh(_) => c.mesh_records += 1,
                        mesh::Record::Variant { bindings, .. } => {
                            c.variant_records += 1;
                            if let Some(b) = bindings.iter().find(|b| !uids.contains(&b.material)) {
                                failures.push(format!("{at} mesh {i}: variant binding material {:08X} is not in the pack", b.material));
                            }
                        }
                    }
                    for p in rec.mesh().streams.iter().flat_map(|s| &s.prims) {
                        if !uids.contains(&p.material) {
                            failures.push(format!("{at} mesh {i}: primitive material {:08X} is not in the pack", p.material));
                        }
                    }
                }
            }
            SlotKind::Wsd => match editnodes::parse_entry_be(slot) {
                Ok(_) => c.wsd_slots += 1,
                Err(e) => failures.push(format!("{at} wsd: {e}")),
            },
            _ => {}
        }
    }
}

#[test]
fn every_2008_pack_and_record_parses() {
    let root = bin360();
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut c = Counts::default();
    let mut failures = Vec::new();
    let mut loose: HashMap<String, Vec<u8>> = HashMap::new();
    let mut megas = Vec::new();
    let (mut empty, mut ap04) = (0usize, 0usize);
    for p in &all {
        let rel = p.strip_prefix(&root).expect("under root").to_string_lossy().replace('/', "\\");
        if !rel.to_ascii_lowercase().ends_with(".pack") {
            continue;
        }
        let mut f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let mut head = [0u8; 4];
        if f.read(&mut head).expect("read head") < 4 {
            empty += 1;
            continue;
        }
        match &head {
            m if m == mp00::MAGIC => megas.push((p.clone(), rel)),
            b"AP04" => ap04 += 1,
            m if m.starts_with(b"SBL") => {
                let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
                check_sbl(&buf, &rel, &mut c, &mut failures);
                loose.insert(rel.to_ascii_lowercase(), buf);
            }
            m => failures.push(format!("{rel}: unknown magic {m:02X?}")),
        }
    }
    let (mut entries, mut copies) = (0usize, 0usize);
    for (p, rel) in &megas {
        let mut f = std::fs::File::open(p).expect("open mega");
        let len = f.metadata().expect("metadata").len();
        let mut hdr = [0u8; 8];
        f.read_exact(&mut hdr).expect("mega header");
        let mut dir = vec![0u8; mp00::index_len(&hdr).unwrap_or_else(|e| panic!("{rel}: {e}"))];
        f.seek(SeekFrom::Start(0)).expect("seek");
        f.read_exact(&mut dir).expect("mega directory");
        let es = mp00::parse_index(&dir).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if let Err(e) = mp00::check_layout(&es, len) {
            failures.push(format!("{rel}: {e}"));
        }
        for e in &es {
            entries += 1;
            let mut buf = vec![0u8; e.size as usize];
            f.seek(SeekFrom::Start(e.offset)).expect("seek entry");
            f.read_exact(&mut buf).expect("read entry");
            if loose.iter().any(|(path, b)| pandemic_hash(path) == e.hash && *b == buf) {
                copies += 1;
            }
        }
    }
    let mut shader_variants = 0;
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()) == Some("shaders")) {
        let buf = std::fs::read(p).expect("read shaders");
        match shaders::parse(&buf) {
            Ok(s) => {
                for v in s.pixel.iter().chain(&s.vertex).flat_map(|g| &g.variants) {
                    match shaders::constant_table(v.code) {
                        Ok(_) => shader_variants += 1,
                        Err(e) => failures.push(format!("{}: {e}", p.display())),
                    }
                }
            }
            Err(e) => failures.push(format!("{}: {e}", p.display())),
        }
    }
    let mut unwrapped = 0;
    for p in all.iter().filter(|p| p.extension().and_then(|x| x.to_str()) == Some("xex")) {
        let buf = std::fs::read(p).expect("read xex");
        let r = xex::parse(&buf).and_then(|h| xex::unwrap(&buf, &h, &xex::DEVKIT_KEK));
        match r {
            Ok(_) => unwrapped += 1,
            Err(e) => failures.push(format!("{}: {e}", p.display())),
        }
    }
    eprintln!(
        "{} SBL sub-packs, {} mesh records + {} variant records, {} textures, {} materials in {} slots, {} wsd \
         slots; {entries} MP00 entries, {copies} byte copies of a loose SBL .pack keyed by its path; {unwrapped} \
         .xex unwrapped",
        c.sbl, c.mesh_records, c.variant_records, c.textures, c.materials, c.material_slots, c.wsd_slots
    );
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert_eq!((empty, ap04, megas.len()), (2, 1, 4));
    assert_eq!(c.sbl, 4389);
    assert_eq!(c.mesh_records + c.variant_records, 137_861);
    assert_eq!(c.variant_records, 215);
    assert_eq!(c.textures, 47_313);
    assert_eq!((c.material_slots, c.materials), (3_552, 97_597));
    assert_eq!(entries, 4960);
    assert_eq!(copies, 4385);
    assert_eq!(unwrapped, 6);
    assert_eq!(shader_variants, 3_340 + 135);
}

#[derive(Default, Debug, PartialEq)]
struct AnimCounts {
    clips: usize,
    decoded: usize,
    sampled: usize,
    /// The sampler's preconditions fail: `duration > FLT_EPSILON`, two poses to interpolate.
    unsampleable: usize,
    tracks_ne_bones: usize,
}

/// Decodes `clip`, reads every stored pose, and samples it at its start, middle, end and past its end.
fn check_clip(clip: &anim::Clip, at: &str, c: &mut AnimCounts, failures: &mut Vec<String>) {
    c.clips += 1;
    c.tracks_ne_bones += (clip.tracks as usize != clip.bones.len()) as usize;
    let a = match clip.decode() {
        Ok(a) => a,
        Err(e) => return failures.push(format!("{at}: decode: {e}")),
    };
    c.decoded += 1;
    let finite = |p: &[anim::QsTransform]| p.iter().all(|x| x.translation.iter().chain(&x.rotation).chain(&x.scale).all(|v| v.is_finite()));
    for f in 0..a.poses() {
        match a.pose(f) {
            Ok(p) if p.len() == a.tracks() && finite(&p) => {}
            Ok(_) => return failures.push(format!("{at}: pose {f} is short or not finite")),
            Err(e) => return failures.push(format!("{at}: pose {f}: {e}")),
        }
    }
    if !(clip.duration > f32::EPSILON) || a.poses() < 2 {
        c.unsampleable += 1;
        return;
    }
    for t in [0.0, clip.duration * 0.5, clip.duration, clip.duration * 2.0] {
        match a.sample(t) {
            Ok(p) if p.len() == a.tracks() && finite(&p) => {}
            Ok(_) => return failures.push(format!("{at}: sample {t} is short or not finite")),
            Err(e) => return failures.push(format!("{at}: sample {t}: {e}")),
        }
    }
    c.sampled += 1;
}

#[test]
fn every_2008_animation_parses_and_decodes() {
    let root = bin360();
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut failures = Vec::new();
    let mut loose = AnimCounts::default();
    let mut loose_rejected = 0usize;
    let mut codecs: BTreeMap<(&str, Codec), usize> = BTreeMap::new();
    let mut by_key: HashMap<u32, Vec<Vec<u8>>> = HashMap::new();
    for p in &all {
        let rel = p.strip_prefix(&root).expect("under root").display().to_string();
        let mut head = [0u8; 4];
        let mut f = std::fs::File::open(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        if f.read(&mut head).expect("read head") < 4 || &head != anim::MAGIC {
            continue;
        }
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let clip = match anim::parse(&buf) {
            Ok(c) => c,
            Err(e) => {
                failures.push(format!("{rel}: {e}"));
                continue;
            }
        };
        if clip.len != buf.len() {
            failures.push(format!("{rel}: the clip ends at {} of {} bytes", clip.len, buf.len()));
        }
        *codecs.entry(("loose", clip.codec())).or_default() += 1;
        let stem = p.file_stem().expect("stem").to_string_lossy();
        by_key.entry(pandemic_hash(&stem)).or_default().push(buf.clone());
        if clip.check_loose().is_err() {
            loose_rejected += 1;
            continue;
        }
        check_clip(&clip, &rel, &mut loose, &mut failures);
    }

    let pack_path = root.join("Animations.pack");
    let buf = std::fs::read(&pack_path).unwrap_or_else(|e| panic!("{}: {e}", pack_path.display()));
    let pack = anim_pack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", pack_path.display()));
    let mut packed = AnimCounts::default();
    let mut twins = 0usize;
    for r in &pack.records {
        *codecs.entry(("pack", r.clip.codec())).or_default() += 1;
        check_clip(&r.clip, &format!("Animations.pack key {:08X}", r.key), &mut packed, &mut failures);
        let body = &buf[r.clip_at..r.clip_at + r.clip.len];
        twins += by_key.get(&r.key).is_some_and(|v| v.iter().any(|b| b == body)) as usize;
    }
    let transitions: usize = pack.transitions.iter().map(|t| t.transitions.len()).sum();
    eprintln!(
        "loose {loose:?}, rejected by LoadAnimFile {loose_rejected}; pack {packed:?}; codecs {codecs:?}; {twins} pack clips \
         byte-identical to a loose clip of the same key; {} intervals, {} sequences, {transitions} transitions, {} banks, \
         {} additive pairs, ALPH {} + {}",
        pack.intervals.len(),
        pack.sequences.len(),
        pack.banks.len(),
        pack.additives.len(),
        pack.list_88.len(),
        pack.list_7c.len()
    );
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(pack.len, buf.len());
    let all_of = |n| AnimCounts { clips: n, decoded: n, sampled: n, unsampleable: 0, tracks_ne_bones: 0 };
    assert_eq!((loose, loose_rejected), (all_of(3590), 30));
    assert_eq!(packed, all_of(1605));
    let count = |from, codec| codecs.get(&(from, codec)).copied().unwrap_or(0);
    assert_eq!([Codec::Wavelet, Codec::Delta, Codec::Uncompressed].map(|c| count("loose", c)), [3099, 286, 235]);
    assert_eq!([Codec::Wavelet, Codec::Delta, Codec::Uncompressed].map(|c| count("pack", c)), [1501, 6, 98]);
    assert_eq!(twins, 1601);
    assert_eq!((pack.intervals.len(), pack.sequences.len(), transitions, pack.banks.len()), (3, 1944, 286, 24));
}
