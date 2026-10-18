//! `proto2008-audio <Bin360>` — read the 2008 prototype's `sound/Saboteur.pck` with
//! `sab_formats::proto2008::{sound_pack, sound_bank}` and tally what playing a line rests on: the
//! header and both tables, every bank's chunk stream, `BKHD` and `HIRC` objects, every event, action
//! and sound source, every stream's `RIFX` chunks and `XMA2` format, the loose `sound/X360/` copies
//! against the pack, and where each keyed (voice) `GameText` line's `Play_<key>` event leads.
//! docs/2008_audio_format.md.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use sab_formats::gametext::GameText;
use sab_formats::hash::pandemic_hash;
use sab_formats::proto2008::sound_bank::{self, HircObject, SoundSource};
use sab_formats::proto2008::sound_ids;
use sab_formats::proto2008::sound_pack::{self, Chunk};
use sab_formats::wwise::wwise_fnv1;

fn be(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn tag(c: &Chunk) -> String {
    String::from_utf8_lossy(&c.tag).into_owned()
}

fn bump<K: Ord>(m: &mut BTreeMap<K, usize>, k: K) {
    *m.entry(k).or_default() += 1;
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-audio <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let path = root.join("sound/Saboteur.pck");
    let b = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let p = sound_pack::parse(&b, b.len() as u64).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    println!("{}: {} bytes, {:?}", path.display(), b.len(), p.header);
    let f0c: BTreeMap<u32, usize> = p.banks.iter().chain(&p.streams).fold(BTreeMap::new(), |mut m, r| {
        *m.entry(r.f0c).or_default() += 1;
        m
    });
    println!("record +0x0C values: {f0c:?}");
    let aligned = p.banks.iter().chain(&p.streams).filter(|r| r.offset % p.header.block_size == 0).count();
    println!("records at a block boundary: {aligned} of {}", p.banks.len() + p.streams.len());

    // ---- banks ----
    let mut walked = 0usize;
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    let mut versions: BTreeMap<u32, usize> = BTreeMap::new();
    let mut bkhd_id_is_record = 0usize;
    // id -> every (bank index, object) carrying it, payload ranges in bank bytes
    let mut objs: HashMap<u32, Vec<(usize, HircObject)>> = HashMap::new();
    let mut hirc_ok = 0usize;
    let mut hirc_bad = Vec::new();
    let mut types: BTreeMap<u32, usize> = BTreeMap::new();
    let mut bank_bytes: Vec<&[u8]> = Vec::new();
    let mut didx_media: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut didx_entries = 0usize;
    // bank index -> (DATA body start, DATA len, DIDX entries {id, offset, size})
    let mut media: HashMap<usize, (usize, usize, Vec<[u32; 3]>)> = HashMap::new();
    for (bi, r) in p.banks.iter().enumerate() {
        let d = r.data(&b).expect("bank in file");
        bank_bytes.push(d);
        let cs = match sound_pack::chunks(d) {
            Ok(cs) => cs,
            Err(e) => {
                println!("  bank {:08X}: {e}", r.id);
                continue;
            }
        };
        walked += 1;
        for c in &cs {
            *tags.entry(tag(c)).or_default() += 1;
            let body = &d[c.start..c.start + c.len];
            match &c.tag {
                b"BKHD" => {
                    *versions.entry(be(body, 0)).or_default() += 1;
                    if be(body, 4) == r.id {
                        bkhd_id_is_record += 1;
                    }
                }
                b"HIRC" => match sound_bank::hirc_objects(body) {
                    Ok(list) => {
                        hirc_ok += 1;
                        for mut o in list {
                            bump(&mut types, o.kind);
                            o.start += c.start;
                            objs.entry(o.id).or_default().push((bi, o));
                        }
                    }
                    Err(e) => hirc_bad.push(format!("bank {:08X}: {e}", r.id)),
                },
                b"DIDX" => {
                    if c.len % 12 != 0 {
                        println!("  bank {:08X}: DIDX of {} bytes is not 12-byte entries", r.id, c.len);
                    }
                    for e in 0..c.len / 12 {
                        didx_entries += 1;
                        didx_media.entry(be(body, 12 * e)).or_default().push(bi);
                        media.entry(bi).or_default().2.push([be(body, 12 * e), be(body, 12 * e + 4), be(body, 12 * e + 8)]);
                    }
                }
                b"DATA" => {
                    let m = media.entry(bi).or_default();
                    m.0 = c.start;
                    m.1 = c.len;
                }
                _ => {}
            }
        }
    }
    println!("\nbanks: {} of {} walk to their end; chunk tags {tags:?}", walked, p.banks.len());
    println!("BKHD versions {versions:?}; BKHD id == record id: {bkhd_id_is_record}");
    println!("HIRC chunks walking exactly: {hirc_ok}; failing: {hirc_bad:?}");
    let multi = objs.values().filter(|v| v.len() > 1).count();
    println!("HIRC object types: {types:?} ({} distinct ids, {multi} of them in more than one bank)", objs.len());
    // Ids several banks define with different payloads: by type and bank pair, and what differs.
    let mut differing: BTreeMap<String, usize> = BTreeMap::new();
    let mut diff_fields: BTreeMap<String, usize> = BTreeMap::new();
    for (id, defs) in &objs {
        let pl = |(bi, o): &(usize, HircObject)| &bank_bytes[*bi][o.start..o.start + o.len];
        for d in &defs[1..] {
            let (a, b2) = (pl(&defs[0]), pl(d));
            if a == b2 {
                continue;
            }
            let kind = defs[0].1.kind;
            bump(&mut differing, format!("type {kind}, banks {:08X} / {:08X}", p.banks[defs[0].0].id, p.banks[d.0].id));
            let what = match kind {
                sound_bank::HIRC_SOUND => match (sound_bank::parse_sound(a), sound_bank::parse_sound(b2)) {
                    (Ok(x), Ok(y)) => {
                        let mut f = Vec::new();
                        if x.source != y.source {
                            f.push("source");
                        }
                        if x.node.bus_id != y.node.bus_id {
                            f.push("bus");
                        }
                        if x.node.parent_id != y.node.parent_id {
                            f.push("parent");
                        }
                        if a[x.source.end..x.node.end] != b2[y.source.end..y.node.end] {
                            f.push("node bytes");
                        }
                        if x.tail != y.tail {
                            f.push("tail");
                        }
                        format!("sound: {}", f.join("+"))
                    }
                    _ => "sound: unparsed".into(),
                },
                sound_bank::HIRC_EVENT => "event".into(),
                sound_bank::HIRC_ACTION => match (sound_bank::parse_action(a), sound_bank::parse_action(b2)) {
                    (Ok(x), Ok(y)) if x.target == y.target && x.action_type == y.action_type => "action: same type and target".into(),
                    _ => "action: type or target".into(),
                },
                sound_bank::HIRC_RAN_SEQ => match (sound_bank::parse_ran_seq(a), sound_bank::parse_ran_seq(b2)) {
                    (Ok(x), Ok(y)) if x.children == y.children && x.playlist == y.playlist => "ran-seq: same children and playlist".into(),
                    _ => "ran-seq: children or playlist".into(),
                },
                k => format!("type {k}"),
            };
            bump(&mut diff_fields, what);
            if diff_fields.len() == 1 && differing.values().sum::<usize>() == 1 {
                println!("  first differing id {id:08X}");
            }
        }
    }
    println!("ids defined differently by two banks: {differing:?}");
    println!("what differs: {diff_fields:?}");
    println!("DIDX: {didx_entries} 12-byte entries, {} distinct first words", didx_media.len());
    let (mut in_data, mut media_riff, mut media_xma2) = (0usize, 0usize, 0usize);
    let mut media_bad = Vec::new();
    let mut heads: BTreeMap<String, usize> = BTreeMap::new();
    let stream_rec: HashMap<u32, &sound_pack::Record> = p.streams.iter().map(|r| (r.id, r)).collect();
    let mut didx_of: HashMap<(usize, u32), [u32; 3]> = HashMap::new();
    for (bi, (ds, dl, entries)) in &media {
        for e in entries {
            didx_of.insert((*bi, e[0]), *e);
            if e[1] as usize + e[2] as usize > *dl {
                media_bad.push(format!("bank {:08X} media {:08X}: 0x{:X}+0x{:X} past DATA 0x{dl:X}", p.banks[*bi].id, e[0], e[1], e[2]));
                continue;
            }
            in_data += 1;
            let m = &bank_bytes[*bi][ds + e[1] as usize..ds + e[1] as usize + e[2] as usize];
            match sound_pack::riff_chunks(m) {
                Ok(cs) => {
                    media_riff += 1;
                    if cs.iter().any(|c| &c.tag == b"XMA2" && sound_pack::parse_xma2(&m[c.start..c.start + c.len]).is_ok()) {
                        media_xma2 += 1;
                    }
                }
                Err(err) => {
                    let head = match stream_rec.get(&e[0]) {
                        None => "media id not a pack stream".to_string(),
                        Some(r) => {
                            let full = r.data(&b).expect("stream");
                            format!(
                                "pack stream; RIFX size word == stream size {}; entry bytes are the stream's first bytes {}",
                                u32::from_le_bytes([m[4], m[5], m[6], m[7]]) as usize == full.len(),
                                full.starts_with(m)
                            )
                        }
                    };
                    bump(&mut heads, head);
                    media_bad.push(format!("bank {:08X} media {:08X}: {err}", p.banks[*bi].id, e[0]));
                }
            }
        }
    }
    println!("DIDX media: inside DATA {in_data}, RIFX with its size word == entry size {media_riff}, with a parsed XMA2 chunk {media_xma2}");
    for e in media_bad.iter().take(5) {
        println!("  not whole {e}");
    }
    println!("DIDX media that are not a whole RIFX: {heads:?}");

    // ---- events, actions, sounds ----
    let payload = |bi: usize, o: &HircObject| &bank_bytes[bi][o.start..o.start + o.len];
    let (mut events, mut actions, mut sounds, mut ran_seqs) = (0usize, 0usize, 0usize, 0usize);
    let mut bad_obj = Vec::new();
    let mut action_classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut play_types: BTreeMap<String, usize> = BTreeMap::new();
    let mut play_params: BTreeMap<usize, usize> = BTreeMap::new();
    let mut ran_seq_of: HashMap<u32, sound_bank::RanSeq> = HashMap::new();
    let mut parent_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut plugins: BTreeMap<String, usize> = BTreeMap::new();
    let mut src_flags: BTreeMap<u8, usize> = BTreeMap::new();
    let mut source_of: HashMap<u32, SoundSource> = HashMap::new();
    let pack_streams: HashSet<u32> = p.streams.iter().map(|r| r.id).collect();
    let (mut file_in_pack, mut src_eq_file, mut inbank_in_didx) = (0usize, 0usize, 0usize);
    let mut inbank_vs_didx: BTreeMap<String, usize> = BTreeMap::new();
    let mut inbank_file: BTreeMap<String, usize> = BTreeMap::new();
    let bank_ids: HashSet<u32> = p.banks.iter().map(|r| r.id).collect();
    for (bi, o) in objs.values().flatten() {
        let pl = payload(*bi, o);
        match o.kind {
            sound_bank::HIRC_EVENT => match sound_bank::parse_event(pl) {
                Ok(_) => events += 1,
                Err(e) => bad_obj.push(e),
            },
            sound_bank::HIRC_ACTION => match sound_bank::parse_action(pl) {
                Ok(a) => {
                    actions += 1;
                    bump(&mut action_classes, format!("{:05X}", a.action_type & sound_bank::ACTION_CLASS_MASK));
                    if a.is_play() {
                        bump(&mut play_types, format!("{:08X}", a.action_type));
                        bump(&mut play_params, a.params.len());
                    }
                }
                Err(e) => bad_obj.push(e),
            },
            sound_bank::HIRC_RAN_SEQ => match sound_bank::parse_ran_seq(pl) {
                Ok(r) => {
                    ran_seqs += 1;
                    ran_seq_of.insert(r.id, r);
                }
                Err(e) => bad_obj.push(e),
            },
            sound_bank::HIRC_SOUND => match sound_bank::parse_sound(pl) {
                Ok(snd) => {
                    sounds += 1;
                    let parent = match objs.get(&snd.node.parent_id) {
                        None if snd.node.parent_id == 0 => "none".to_string(),
                        None => "not in any bank".to_string(),
                        Some(v) => format!("type {}", v[0].1.kind),
                    };
                    bump(&mut parent_kinds, parent);
                    let s = snd.source;
                    bump(&mut plugins, format!("{:08X} stream type {}", s.plugin_id, s.stream_type));
                    bump(&mut src_flags, s.flag);
                    if pack_streams.contains(&s.file_id) {
                        file_in_pack += 1;
                    }
                    if s.source_id == s.file_id {
                        src_eq_file += 1;
                    }
                    if s.in_bank.is_some() && didx_media.get(&s.source_id).is_some_and(|v| v.contains(bi)) {
                        inbank_in_didx += 1;
                    }
                    if let Some((off, size)) = s.in_bank {
                        let rel = match didx_of.get(&(*bi, s.source_id)) {
                            None => "no DIDX entry in its bank".to_string(),
                            Some(e) => format!(
                                "offset {} size {}",
                                if off == e[1] { "== DIDX" } else { "!= DIDX" },
                                if size == e[2] { "== DIDX" } else { "!= DIDX" }
                            ),
                        };
                        bump(&mut inbank_vs_didx, format!("stream type {}: {rel}", s.stream_type));
                        if !didx_of.contains_key(&(*bi, s.source_id)) {
                            bump(&mut inbank_file, format!("no DIDX entry: plugin {:08X}", s.plugin_id));
                        }
                        let f = if s.file_id == p.banks[*bi].id {
                            "its own bank's id"
                        } else if bank_ids.contains(&s.file_id) {
                            "another bank's id"
                        } else if pack_streams.contains(&s.file_id) {
                            "a pack stream id"
                        } else {
                            "none of these"
                        };
                        bump(&mut inbank_file, format!("stream type {}: file id is {f}", s.stream_type));
                    }
                    source_of.insert(s.id, s);
                }
                Err(e) => bad_obj.push(e),
            },
            _ => {}
        }
    }
    println!(
        "\nparsed to their exact end: events {events}, actions {actions}, sounds {sounds}, random/sequence containers {ran_seqs}; failures {}",
        bad_obj.len()
    );
    for e in bad_obj.iter().take(10) {
        println!("  bad {e}");
    }
    println!("action classes (type & 0xFF000): {action_classes:?}");
    println!("Play action types: {play_types:?}; parameter bytes: {play_params:?}");
    println!("sound parent object: {parent_kinds:?}");
    let mut child_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut child_is_listed = 0usize;
    for r in ran_seq_of.values() {
        for ch in &r.children {
            bump(&mut child_kinds, objs.get(ch).map_or("not in any bank".to_string(), |v| format!("type {}", v[0].1.kind)));
        }
        if r.playlist.iter().all(|(id, _)| r.children.contains(id)) {
            child_is_listed += 1;
        }
    }
    println!("random/sequence children: {child_kinds:?}; containers whose playlist ids are all children: {child_is_listed}");
    println!("sound plugin id / stream type: {plugins:?}");
    println!("sound source flag byte: {src_flags:?}");
    println!(
        "sounds: file id in the pack's stream table {file_in_pack}; source id == file id {src_eq_file}; in-bank source id in its bank's DIDX {inbank_in_didx}"
    );
    println!("in-bank sound offset/size against its DIDX entry: {inbank_vs_didx:?}");
    println!("in-bank sound file id: {inbank_file:?}");

    // ---- streams ----
    let mut riff_ok = 0usize;
    let mut stags: BTreeMap<String, usize> = BTreeMap::new();
    let mut rates: BTreeMap<u32, usize> = BTreeMap::new();
    let mut chans: BTreeMap<String, usize> = BTreeMap::new();
    let mut xversions: BTreeMap<u8, usize> = BTreeMap::new();
    let mut bad = Vec::new();
    let mut chunk_lens: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut seek_is_blocks = 0usize;
    let mut seek_rising_to_samples = 0usize;
    let mut data_is_blocks = 0usize;
    let mut loops: BTreeMap<u8, usize> = BTreeMap::new();
    let mut list_types: BTreeMap<String, usize> = BTreeMap::new();
    let mut xmac_rel: BTreeMap<String, usize> = BTreeMap::new();
    let mut xmac_examples: Vec<String> = Vec::new();
    for r in &p.streams {
        let d = r.data(&b).expect("stream in file");
        let cs = match sound_pack::riff_chunks(d) {
            Ok(cs) => cs,
            Err(e) => {
                bad.push(format!("{:08X}: {e}", r.id));
                continue;
            }
        };
        riff_ok += 1;
        let set: Vec<String> = cs.iter().map(tag).collect();
        *stags.entry(set.join(",")).or_default() += 1;
        for c in &cs {
            if matches!(&c.tag, b"XMA2" | b"XMAc" | b"cue " | b"LIST") {
                bump(chunk_lens.entry(tag(c)).or_default(), c.len.to_string());
            }
            if &c.tag == b"LIST" && c.len >= 4 {
                bump(&mut list_types, String::from_utf8_lossy(&d[c.start..c.start + 4]).into_owned());
            }
        }
        if let (Some(xc), Some(x2)) = (cs.iter().find(|c| &c.tag == b"XMAc"), cs.iter().find(|c| &c.tag == b"XMA2")) {
            if let Ok(x) = sound_pack::parse_xma2(&d[x2.start..x2.start + x2.len]) {
                let w: Vec<u32> = (0..3).map(|i| be(d, xc.start + 4 * i)).collect();
                let rel = |v: u32| -> String {
                    let named = [
                        ("samples encoded", x.samples_encoded),
                        ("samples in source", x.samples_in_source),
                        ("block count", x.block_count),
                        ("block size", x.block_size),
                        ("loop begin", x.loop_begin),
                        ("loop end", x.loop_end),
                        ("sample rate", x.sample_rate),
                    ];
                    named.iter().find(|(_, n)| *n == v).map_or(if v == 0 { "0".into() } else { "other".into() }, |(s, _)| s.to_string())
                };
                bump(&mut xmac_rel, format!("[{}, {}, {}]", rel(w[0]), rel(w[1]), rel(w[2])));
                if xmac_examples.len() < 4 {
                    xmac_examples.push(format!("{:08X}: XMAc {:08X?}; XMA2 {x:?}", r.id, w));
                }
            }
        }
        if let Some(c) = cs.iter().find(|c| &c.tag == b"XMA2") {
            match sound_pack::parse_xma2(&d[c.start..c.start + c.len]) {
                Ok(x) => {
                    *rates.entry(x.sample_rate).or_default() += 1;
                    *xversions.entry(x.version).or_default() += 1;
                    *chans.entry(format!("{:?}", x.streams.iter().map(|s| s.0).collect::<Vec<_>>())).or_default() += 1;
                    bump(&mut loops, x.loop_count);
                    if let Some(s) = cs.iter().find(|c| &c.tag == b"seek") {
                        if s.len == 4 * x.block_count as usize {
                            seek_is_blocks += 1;
                            let v: Vec<u32> = (0..x.block_count as usize).map(|i| be(d, s.start + 4 * i)).collect();
                            if v.windows(2).all(|w| w[0] < w[1]) && v.last() == Some(&x.samples_encoded) {
                                seek_rising_to_samples += 1;
                            }
                        }
                    }
                    if let Some(dc) = cs.iter().find(|c| &c.tag == b"data") {
                        let full = x.block_size as usize * x.block_count as usize;
                        if dc.len <= full && dc.len + x.block_size as usize > full {
                            data_is_blocks += 1;
                        }
                    }
                }
                Err(e) => bad.push(format!("{:08X}: XMA2 {e}", r.id)),
            }
        }
    }
    println!("\nstreams: {riff_ok} of {} are RIFX WAVE with a size word equal to the record size", p.streams.len());
    println!("chunk sets: {stags:?}");
    println!("XMA2 versions {xversions:?}; sample rates {rates:?}; channels per XMA stream {chans:?}; loop counts {loops:?}");
    println!("chunk body lengths: {chunk_lens:?}; LIST types {list_types:?}");
    println!("XMAc words equal to XMA2 fields: {xmac_rel:?}");
    for e in &xmac_examples {
        println!("  e.g. {e}");
    }
    println!(
        "seek: 4 bytes per XMA2 block {seek_is_blocks}, of them strictly rising and ending at samples encoded {seek_rising_to_samples}; data within the last block of block size × block count {data_is_blocks}"
    );
    for e in bad.iter().take(10) {
        println!("  bad {e}");
    }

    // ---- loose X360 copies ----
    let x360 = root.join("sound/X360");
    let mut loose: Vec<PathBuf> = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
            let p = e.expect("entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    walk(&x360, &mut loose);
    let stream_of: HashMap<u32, &sound_pack::Record> = p.streams.iter().map(|r| (r.id, r)).collect();
    let bank_of: HashMap<u32, &sound_pack::Record> = p.banks.iter().map(|r| (r.id, r)).collect();
    let (mut xma, mut xma_in, mut xma_same, mut xma_prefix) = (0usize, 0usize, 0usize, 0usize);
    let (mut bnk, mut bnk_named, mut bnk_in, mut bnk_same) = (0usize, 0usize, 0usize, 0usize);
    for f in &loose {
        let name = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let ext = f.extension().and_then(|s| s.to_str()).unwrap_or_default();
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        if ext == "xma" {
            xma += 1;
            if let Some(r) = name.parse::<u32>().ok().and_then(|id| stream_of.get(&id)) {
                xma_in += 1;
                let d = r.data(&b).expect("stream");
                if d == bytes.as_slice() {
                    xma_same += 1;
                } else if d.len() >= bytes.len() && &d[..bytes.len()] == bytes.as_slice() || bytes.starts_with(d) {
                    xma_prefix += 1;
                }
            }
        } else if ext == "bnk" {
            bnk += 1;
            let id = wwise_fnv1(name);
            if bytes.len() >= 16 && &bytes[0..4] == b"BKHD" && be(&bytes, 12) == id {
                bnk_named += 1;
            }
            if let Some(r) = bank_of.get(&id) {
                bnk_in += 1;
                if r.data(&b).expect("bank") == bytes.as_slice() {
                    bnk_same += 1;
                }
            }
        }
    }
    println!("\nX360: {} files; .xma {xma}, of them in the pack's stream table {xma_in}, byte-identical {xma_same}, one a prefix of the other {xma_prefix}", loose.len());
    println!("X360: .bnk {bnk}, BKHD id == wwise_fnv1(name) {bnk_named}, in the pack's bank table {bnk_in}, byte-identical {bnk_same}");
    let ini = std::fs::read_to_string(root.join("SoundBanks.ini")).unwrap_or_default();
    let listed: Vec<&str> = ini.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let in_pack = listed.iter().filter(|n| bank_of.contains_key(&wwise_fnv1(n.trim_end_matches(".bnk")))).count();
    println!("SoundBanks.ini: {} banks listed, {in_pack} in the pack's bank table", listed.len());

    // ---- WWiseIDTable.bin ----
    let event_ids: HashSet<u32> = objs.iter().filter(|(_, v)| v[0].1.kind == sound_bank::HIRC_EVENT).map(|(id, _)| *id).collect();
    let mut table = None;
    for rel in ["sound/WWiseIDTable.bin", "sound/X360/WWiseIDTable.bin"] {
        let path = root.join(rel);
        let t = sound_ids::parse(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let ids = |f: fn(&sound_ids::EventIds) -> u32| {
            let v: Vec<u32> = t.events.iter().map(f).filter(|&x| x != sound_ids::NONE).collect();
            (v.len(), v.iter().filter(|x| event_ids.contains(x)).count())
        };
        let (np, pin) = ids(|e| e.play);
        let (ns, sin) = ids(|e| e.stop);
        let members: usize = t.state_groups.iter().chain(&t.switch_groups).map(|g| g.members.len()).sum();
        println!(
            "\n{rel}: events {}, state groups {}, switch groups {} ({members} members), triggers {}, RTPCs {}",
            t.events.len(),
            t.state_groups.len(),
            t.switch_groups.len(),
            t.triggers.len(),
            t.rtpcs.len()
        );
        println!("  play ids {np}, of them a HIRC event in the pack {pin}; stop ids {ns}, in the pack {sin}");
        if rel == "sound/WWiseIDTable.bin" {
            table = Some(t);
        }
    }
    let table = table.expect("sound/WWiseIDTable.bin parsed");
    let by_key: HashMap<u32, sound_ids::EventIds> = table.events.iter().map(|e| (e.key, *e)).collect();

    // ---- voice lines ----
    let gt_path = root.join("Cinematics/Dialog/English/GameText.dlg");
    let gt = GameText::parse(&std::fs::read(&gt_path).unwrap_or_else(|e| panic!("{}: {e}", gt_path.display()))).expect("GameText");
    let keyed: Vec<String> = gt.records.iter().filter(|r| !r.is_ui()).map(|r| r.key_str()).collect();
    let mut key_forms: BTreeMap<String, usize> = BTreeMap::new();
    for k in &keyed {
        let fnv_play = wwise_fnv1(&sab_formats::wwise::event_name(k));
        let row = by_key.get(&pandemic_hash(k));
        let form = match row {
            None => "pandemic_hash(key) not a table key".to_string(),
            Some(e) => format!(
                "pandemic_hash(key) is a key; play == fnv1(Play_<key>) {}; stop is none {}",
                e.play == fnv_play,
                e.stop == sound_ids::NONE
            ),
        };
        bump(&mut key_forms, format!("{form}; Play_<key> is a pack event {}", event_ids.contains(&fnv_play)));
    }
    println!("\nvoice keys against the ID table: {key_forms:?}");
    let mut no_event = 0usize;
    let mut event_kinds: BTreeMap<u32, usize> = BTreeMap::new();
    let mut shapes: BTreeMap<String, usize> = BTreeMap::new();
    let mut targets: BTreeMap<String, usize> = BTreeMap::new();
    let mut voice_sources: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: BTreeMap<String, String> = BTreeMap::new();
    for k in &keyed {
        let ev = wwise_fnv1(&sab_formats::wwise::event_name(k));
        let Some(found) = objs.get(&ev) else {
            no_event += 1;
            continue;
        };
        let (bi, o) = &found[0];
        bump(&mut event_kinds, o.kind);
        if o.kind != sound_bank::HIRC_EVENT {
            continue;
        }
        let e = sound_bank::parse_event(payload(*bi, o)).expect("event parsed above");
        let mut plays = 0usize;
        for aid in &e.actions {
            let Some(a) = objs.get(aid).map(|v| &v[0]).filter(|(_, ao)| ao.kind == sound_bank::HIRC_ACTION) else {
                bump(&mut targets, "action id not a loaded action".into());
                continue;
            };
            let act = sound_bank::parse_action(payload(a.0, &a.1)).expect("action parsed above");
            if !act.is_play() {
                continue;
            }
            plays += 1;
            let what = match objs.get(&act.target).map(|v| v[0].1.kind) {
                None => "target not in any bank".to_string(),
                Some(sound_bank::HIRC_SOUND) => {
                    let s = &source_of[&act.target];
                    let key = format!(
                        "plugin {:08X} stream type {} file in pack {}",
                        s.plugin_id,
                        s.stream_type,
                        pack_streams.contains(&s.file_id)
                    );
                    examples.entry(key.clone()).or_insert_with(|| format!("{k} -> sound {:08X} file {:08X}", s.id, s.file_id));
                    bump(&mut voice_sources, key);
                    "sound".to_string()
                }
                Some(sound_bank::HIRC_RAN_SEQ) => {
                    let r = &ran_seq_of[&act.target];
                    let kids: Vec<String> = r
                        .children
                        .iter()
                        .map(|ch| match source_of.get(ch) {
                            Some(s) => format!("sound st{} in pack {}", s.stream_type, pack_streams.contains(&s.file_id)),
                            None => "not a sound".to_string(),
                        })
                        .collect();
                    examples
                        .entry("random/sequence".into())
                        .or_insert_with(|| format!("{k} -> {:08X} mode {} children {kids:?}", act.target, r.mode));
                    bump(&mut voice_sources, format!("via random/sequence: {} children, all sounds {}", r.children.len(), kids.iter().all(|s| s.starts_with("sound"))));
                    "random/sequence".to_string()
                }
                Some(t) => format!("type {t}"),
            };
            bump(&mut targets, what);
        }
        bump(&mut shapes, format!("{} actions, {plays} Play", e.actions.len()));
    }
    println!("\nGameText keyed lines {}: no object named Play_<key> {no_event}; object types found {event_kinds:?}", keyed.len());
    println!("voice event shapes: {shapes:?}");
    println!("Play targets: {targets:?}");
    println!("voice sound sources: {voice_sources:?}");
    for (k, v) in &examples {
        println!("  e.g. {k}: {v}");
    }
}
