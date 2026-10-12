//! `texbind` — how does a part mesh find its textures?
//!
//! READ-ONLY. Answers, from real bytes only:
//!   * which megapack / sub-pack holds a given DTEX, and its exact record name;
//!   * what a mesh's per-drawcall `material` hashes are;
//!   * what `France.materials` (WSAO, magic `OASW`) says those material hashes point at;
//!   * whether the WSTX texture-name-hashes it yields are `pandemic_hash` of real DTEX names.
//!
//! Container facts are COPIED from `sab_formats` (megapack / SBLA / DTEX / MESH) — see those
//! modules for the ground-truthing. Nothing here writes into the game data; `index` writes one
//! TSV to a path the caller names.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

// --------------------------------------------------------------------------- scalars
fn u16at(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32at(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn i32at(b: &[u8], o: usize) -> i32 { i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn u64at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o+1], b[o+2], b[o+3], b[o+4], b[o+5], b[o+6], b[o+7]])
}

use sab_formats::pandemic_hash;

// --------------------------------------------------------------------------- megapack
#[derive(Clone, Copy)]
pub struct Entry { pub crc: u32, pub index: u32, pub size: u32, pub offset: u64 }

pub fn megapack_index(path: &str) -> Result<Vec<Entry>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    if &hdr[0..4] != b"00PM" { return Err(format!("{path}: not a megapack")); }
    let count = u32at(&hdr, 4) as usize;
    let mut idx = vec![0u8; count * 20];
    f.read_exact(&mut idx).map_err(|e| e.to_string())?;
    Ok((0..count).map(|i| {
        let o = i * 20;
        Entry { crc: u32at(&idx, o), index: u32at(&idx, o + 4), size: u32at(&idx, o + 8), offset: u64at(&idx, o + 12) }
    }).collect())
}

pub fn read_entry(f: &mut std::fs::File, e: &Entry, buf: &mut Vec<u8>) -> bool {
    buf.resize(e.size as usize, 0);
    f.seek(SeekFrom::Start(e.offset)).is_ok() && f.read_exact(buf).is_ok()
}

// --------------------------------------------------------------------------- DTEX scan
fn known_format(f: u32) -> bool {
    matches!(f, 0x3154_5844 | 0x3354_5844 | 0x3554_5844 | 0x15 | 0x16 | 0x21)
}

/// (offset, name) of every DTEX record in a raw sub-pack (records are stored uncompressed;
/// only their mip streams are zlib).
pub fn dtex_records(sub: &[u8]) -> Vec<(usize, String, u16, u16, u32)> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 26 < sub.len() {
        if known_format(u32at(sub, i)) {
            for nl in 1..80usize {
                if i < 4 + nl { break; }
                let lo = i - 4 - nl;
                if u32at(sub, lo) as usize == nl
                    && sub[lo + 4..lo + 4 + nl].iter().all(|&b| (0x20..0x7f).contains(&b))
                {
                    let name = String::from_utf8_lossy(&sub[lo + 4..lo + 4 + nl]).into_owned();
                    let w = u16at(sub, i + 8);
                    let h = u16at(sub, i + 10);
                    let fmt = u32at(sub, i);
                    v.push((lo, name, w, h, fmt));
                    break;
                }
            }
        }
        i += 1;
    }
    v
}

// --------------------------------------------------------------------------- MSHA scan
pub struct Msha { pub off: usize, pub name: String, pub unc0: u32, pub c0: u32 }

pub fn msha_records(buf: &[u8]) -> Vec<Msha> {
    const HEADER: usize = 276;
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + HEADER <= buf.len() {
        if &buf[i..i + 4] == b"AHSM" {
            let unc0 = u32at(buf, i + 4);
            let c0 = u32at(buf, i + 12);
            let nb = &buf[i + 20..i + HEADER];
            let end = nb.iter().position(|&b| b == 0).unwrap_or(0);
            if end > 0 && nb[..end].iter().all(|&b| (0x20..0x7f).contains(&b))
                && c0 > 0 && unc0 > 0 && i + HEADER + c0 as usize <= buf.len()
            {
                out.push(Msha { off: i, name: String::from_utf8_lossy(&nb[..end]).into_owned(), unc0, c0 });
            }
        }
        i += 1;
    }
    out
}

fn inflate(src: &[u8], unc: usize) -> Result<Vec<u8>, String> {
    if src.len() == unc { return Ok(src.to_vec()); }
    let mut out = Vec::with_capacity(unc);
    flate2::read::ZlibDecoder::new(src).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The MESH body's drawcall list: (primitiveIndex, materialHash, parentBone).
/// Offset walk COPIED from `tools/sab_mesh` `parse_mesh` (skeleton math dropped).
pub fn mesh_drawcalls(body: &[u8]) -> Result<Vec<(u32, u32, u16)>, String> {
    if body.len() < 244 { return Err("body < MESH header".into()); }
    let num_bones0 = u32at(body, 204) as usize;
    let num_bone_remaps = u32at(body, 208) as usize;
    let num_streams = u16at(body, 216) as usize;
    let num_primitives = u16at(body, 218) as usize;
    let num_draw_calls = u32at(body, 232) as usize;
    let mut p = 244usize;
    if p + 44 > body.len() { return Err("no MESHSkeleton header (unskinned mesh?)".into()); }
    // MESHSkeleton header (11 u32 = 44 bytes)
    let num_unk_bones0 = u32at(body, p) as usize;
    let num_bones = u32at(body, p + 12) as usize;
    let num_unk_bones1 = u32at(body, p + 16);
    if num_bones != num_bones0 {
        return Err(format!("numBones0({num_bones0}) != numBones2({num_bones})"));
    }
    p += 44;
    p += num_bones;            // boneIds u8
    p += num_unk_bones0;       // pad
    p += num_bones * 64;       // localTMS
    p += num_bones * 64;       // bones
    p += num_bones * 48;       // transforms
    p += num_bones * 2;        // parentIds
    p += num_bones * 4;        // null32
    if num_unk_bones1 != 0 { p += 2; }
    if num_bone_remaps > 0 {
        if p + 8 > body.len() { return Err("truncated at boneRemaps".into()); }
        let unk0 = u32at(body, p) as usize;
        if unk0 != num_bone_remaps { return Err(format!("boneRemap guard {unk0} != {num_bone_remaps}")); }
        p += 8 + num_bone_remaps * 68;
    }
    p += num_streams * 152;
    if p + num_primitives * 100 > body.len() { return Err("truncated at primitives".into()); }
    for k in 0..num_primitives {
        if i32at(body, p + k * 100 + 4) != -1 { return Err(format!("primitive {k} const0 != -1")); }
    }
    p += num_primitives * 100;
    if p + num_draw_calls * 16 > body.len() { return Err("truncated at drawcalls".into()); }
    let mut out = Vec::with_capacity(num_draw_calls);
    for k in 0..num_draw_calls {
        let o = p + k * 16;
        out.push((u32at(body, o), u32at(body, o + 4), u16at(body, o + 12)));
    }
    Ok(out)
}

// --------------------------------------------------------------------------- WSAO
pub struct Wsao {
    pub wstx: Vec<u32>,
    pub by_mat: HashMap<u32, (u32, u32)>, // matHash -> (texBegin, numTex)
    pub n_wsma: usize,
    pub blocks: Vec<(String, usize)>,
}

pub fn wsao_open(path: &str) -> Result<Wsao, String> {
    let b = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    if b.len() < 80 || &b[0..4] != b"OASW" { return Err("not a WSAO (OASW) file".into()); }
    let num_wstx = u32at(&b, 4 * 17) as usize;
    let num_wsma = u32at(&b, 8) as usize;
    let mut blocks = Vec::new();
    for m in [b"XTSW", b"AMSW", b"TSSW", b"APSW", b"PCSW", b"PPSW", b"PVSW"] {
        if let Some(p) = b.windows(4).position(|w| w == m) {
            let mut s: Vec<u8> = m.to_vec(); s.reverse();
            blocks.push((String::from_utf8_lossy(&s).into_owned(), p));
        }
    }
    let find = |m: &[u8; 4]| b.windows(4).position(|w| w == m).ok_or("block not found");
    let wstx_off = find(b"XTSW")? + 4;
    let wsma_off = find(b"AMSW")? + 4;
    let wstx: Vec<u32> = (0..num_wstx).map(|i| u32at(&b, wstx_off + i * 4)).collect();
    let mut by_mat: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut o = wsma_off;
    let mut parsed = 0usize;
    for _ in 0..num_wsma {
        if o + 8 > b.len() { break; }
        let uid = u32at(&b, o);
        let idc = u32at(&b, o + 4) as usize;
        o += 8;
        if idc > 64 || o + idc * 4 + 16 > b.len() { break; }
        let ids: Vec<u32> = (0..idc).map(|i| u32at(&b, o + i * 4)).collect();
        o += idc * 4;
        let num_tex = u32at(&b, o + 4);
        let tex_begin = u32at(&b, o + 8);
        o += 16;
        by_mat.entry(uid).or_insert((tex_begin, num_tex));
        for id in ids { by_mat.entry(id).or_insert((tex_begin, num_tex)); }
        parsed += 1;
    }
    Ok(Wsao { wstx, by_mat, n_wsma: parsed, blocks })
}

impl Wsao {
    pub fn textures(&self, mat: u32) -> Option<Vec<u32>> {
        let &(begin, n) = self.by_mat.get(&mat)?;
        let (b, e) = (begin as usize, (begin + n) as usize);
        (e <= self.wstx.len()).then(|| self.wstx[b..e].to_vec())
    }
}

// --------------------------------------------------------------------------- index TSV
pub struct IndexRow { pub kind: String, pub name: String, pub pack: String, pub entry: usize, pub crc: u32, pub off: usize }

pub fn build_index(packs: &[String]) -> Vec<IndexRow> {
    let mut rows = Vec::new();
    for pack in packs {
        let entries = match megapack_index(pack) { Ok(e) => e, Err(e) => { eprintln!("  ! {e}"); continue } };
        let mut f = std::fs::File::open(pack).unwrap();
        let mut buf = Vec::new();
        eprintln!("[*] {pack}: {} entries", entries.len());
        for (i, e) in entries.iter().enumerate() {
            if !read_entry(&mut f, e, &mut buf) { continue; }
            let crc = if buf.len() >= 12 && &buf[0..4] == b"ALBS" { u32at(&buf, 8) } else { 0 };
            for m in msha_records(&buf) {
                rows.push(IndexRow { kind: "MESH".into(), name: m.name, pack: pack.clone(), entry: i, crc, off: m.off });
            }
            for (off, name, w, h, _fmt) in dtex_records(&buf) {
                rows.push(IndexRow { kind: format!("TEX{w}x{h}"), name, pack: pack.clone(), entry: i, crc, off });
            }
        }
    }
    rows
}

pub fn write_index(rows: &[IndexRow], out: &str) {
    let mut f = std::fs::File::create(out).unwrap();
    for r in rows {
        writeln!(f, "{}\t{}\t{:08X}\t{}\t{}\t{:08X}\t{}",
            r.kind, r.name, pandemic_hash(&r.name), r.pack, r.entry, r.crc, r.off).unwrap();
    }
    eprintln!("[*] wrote {} rows -> {out}", rows.len());
}

pub fn read_index(path: &str) -> Vec<IndexRow> {
    let s = std::fs::read_to_string(path).unwrap_or_else(|e| { eprintln!("read {path}: {e}"); std::process::exit(1) });
    s.lines().filter_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < 7 { return None; }
        Some(IndexRow {
            kind: f[0].into(), name: f[1].into(), pack: f[3].into(),
            entry: f[4].parse().ok()?, crc: u32::from_str_radix(f[5], 16).ok()?, off: f[6].parse().ok()?,
        })
    }).collect()
}

// --------------------------------------------------------------------------- commands
pub fn cmd_index(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind index <out.tsv> <pack>..."); std::process::exit(2); }
    let rows = build_index(&args[1..]);
    write_index(&rows, &args[0]);
}

/// Full chain for every mesh whose name matches `filter`.
pub fn cmd_bind(args: &[String]) {
    if args.len() < 3 { eprintln!("texbind bind <index.tsv> <France.materials> <mesh_substr>"); std::process::exit(2); }
    let (idx_path, mats, filter) = (&args[0], &args[1], &args[2]);
    let rows = read_index(idx_path);
    let wsao = wsao_open(mats).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(1) });
    eprintln!("[*] WSAO {mats}: {} WSMA records parsed, {} WSTX hashes, {} material keys",
        wsao.n_wsma, wsao.wstx.len(), wsao.by_mat.len());
    for (n, o) in &wsao.blocks { eprintln!("      block {n} @0x{o:x}"); }

    // texture-hash -> names (a DTEX name may repeat across bundles)
    let mut by_hash: HashMap<u32, Vec<&IndexRow>> = HashMap::new();
    for r in &rows {
        if r.kind.starts_with("TEX") { by_hash.entry(pandemic_hash(&r.name)).or_default().push(r); }
    }

    let lower = filter.to_ascii_lowercase();
    let meshes: Vec<&IndexRow> = rows.iter()
        .filter(|r| r.kind == "MESH" && r.name.to_ascii_lowercase().contains(&lower)).collect();
    if meshes.is_empty() { println!("no mesh matched \"{filter}\""); return; }

    for m in meshes {
        println!("\n=== {}  [{} entry {} subpack crc {:08X} @0x{:x}]", m.name, short(&m.pack), m.entry, m.crc, m.off);
        println!("    pandemic_hash(name) = {:08X}", pandemic_hash(&m.name));
        // co-located textures in the same sub-pack
        let colocated: Vec<&IndexRow> = rows.iter()
            .filter(|r| r.kind.starts_with("TEX") && r.pack == m.pack && r.entry == m.entry).collect();
        println!("    DTEX in the SAME sub-pack: {}", colocated.len());
        for c in colocated.iter().take(40) { println!("        {} {}", c.kind, c.name); }

        let mut f = std::fs::File::open(&m.pack).unwrap();
        let entries = megapack_index(&m.pack).unwrap();
        let mut buf = Vec::new();
        if !read_entry(&mut f, &entries[m.entry], &mut buf) { println!("    ! entry read failed"); continue; }
        let unc0 = u32at(&buf, m.off + 4) as usize;
        let c0 = u32at(&buf, m.off + 12) as usize;
        let body = match inflate(&buf[m.off + 276..m.off + 276 + c0], unc0) {
            Ok(b) => b, Err(e) => { println!("    ! inflate: {e}"); continue }
        };
        let dcs = match mesh_drawcalls(&body) { Ok(d) => d, Err(e) => { println!("    ! mesh: {e}"); continue } };
        println!("    {} drawcalls", dcs.len());
        let mut seen: Vec<u32> = Vec::new();
        for (pi, mat, pb) in &dcs {
            if seen.contains(mat) { continue; }
            seen.push(*mat);
            match wsao.textures(*mat) {
                None => println!("      prim{pi:3} bone{pb:4} mat {mat:08X}  -> NOT IN WSAO"),
                Some(ts) => {
                    let names: Vec<String> = ts.iter().map(|t| match by_hash.get(t) {
                        Some(v) => format!("{}", v[0].name),
                        None => format!("<{t:08X}?>"),
                    }).collect();
                    println!("      prim{pi:3} bone{pb:4} mat {mat:08X}  -> {} tex: {}", ts.len(), names.join(", "));
                }
            }
        }
    }
}

fn short(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// Where does a DTEX name (or substring) live?
pub fn cmd_where(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind where <index.tsv> <substr>"); std::process::exit(2); }
    let rows = read_index(&args[0]);
    let lower = args[1].to_ascii_lowercase();
    let mut n = 0;
    for r in &rows {
        if r.name.to_ascii_lowercase().contains(&lower) {
            println!("{}\t{}\t{:08X}\t{}\tentry {}\tsubpack {:08X}", r.kind, r.name, pandemic_hash(&r.name), short(&r.pack), r.entry, r.crc);
            n += 1;
        }
    }
    println!("-- {n} row(s)");
}

/// Coverage: for every mesh matching `filter`, how many drawcall materials resolve in WSAO and
/// how many of the resulting texture hashes name a DTEX we can find.
pub fn cmd_cover(args: &[String]) {
    if args.len() < 3 { eprintln!("texbind cover <index.tsv> <France.materials> <mesh_substr>"); std::process::exit(2); }
    let rows = read_index(&args[0]);
    let wsao = wsao_open(&args[1]).unwrap();
    let mut by_hash: HashMap<u32, &IndexRow> = HashMap::new();
    for r in &rows { if r.kind.starts_with("TEX") { by_hash.entry(pandemic_hash(&r.name)).or_insert(r); } }
    let lower = args[2].to_ascii_lowercase();
    let meshes: Vec<&IndexRow> = rows.iter()
        .filter(|r| r.kind == "MESH" && r.name.to_ascii_lowercase().contains(&lower)).collect();
    let mut packcache: HashMap<String, Vec<Entry>> = HashMap::new();
    println!("{:<44} {:>5} {:>5} {:>6} {:>5} {:>6}", "mesh", "dc", "mats", "inWSAO", "tex", "named");
    for m in meshes {
        let entries = packcache.entry(m.pack.clone()).or_insert_with(|| megapack_index(&m.pack).unwrap());
        let mut f = std::fs::File::open(&m.pack).unwrap();
        let mut buf = Vec::new();
        if !read_entry(&mut f, &entries[m.entry], &mut buf) { continue; }
        let unc0 = u32at(&buf, m.off + 4) as usize;
        let c0 = u32at(&buf, m.off + 12) as usize;
        let Ok(body) = inflate(&buf[m.off + 276..m.off + 276 + c0], unc0) else { continue };
        let Ok(dcs) = mesh_drawcalls(&body) else {
            println!("{:<44} {:>5}", m.name, "ERR"); continue };
        let mut mats: Vec<u32> = dcs.iter().map(|d| d.1).collect();
        mats.sort(); mats.dedup();
        let inw = mats.iter().filter(|m| wsao.by_mat.contains_key(m)).count();
        let mut tex = 0; let mut named = 0;
        for mt in &mats {
            if let Some(ts) = wsao.textures(*mt) {
                tex += ts.len();
                named += ts.iter().filter(|t| by_hash.contains_key(t)).count();
            }
        }
        println!("{:<44} {:>5} {:>5} {:>6} {:>5} {:>6}", m.name, dcs.len(), mats.len(), inw, tex, named);
    }
}

/// Raw bytes of the WSMA record for a material hash — so the field layout can be quoted.
pub fn cmd_wsma(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind wsma <France.materials> <matHash hex>"); std::process::exit(2); }
    let b = std::fs::read(&args[0]).unwrap();
    let want = u32::from_str_radix(args[1].trim_start_matches("0x"), 16).unwrap();
    let num_wstx = u32at(&b, 4 * 17) as usize;
    let num_wsma = u32at(&b, 8) as usize;
    let wsma_off = b.windows(4).position(|w| w == b"AMSW").unwrap() + 4;
    let wstx_off = b.windows(4).position(|w| w == b"XTSW").unwrap() + 4;
    println!("header: numWSMA(+0x08)={num_wsma}  numWSTX(+0x44)={num_wstx}");
    println!("WSMA block body @0x{wsma_off:x}   WSTX block body @0x{wstx_off:x}");
    let mut o = wsma_off;
    for i in 0..num_wsma {
        if o + 8 > b.len() { break; }
        let rec = o;
        let uid = u32at(&b, o);
        let idc = u32at(&b, o + 4) as usize;
        o += 8;
        if idc > 64 || o + idc * 4 + 16 > b.len() { println!("stop at record {i} (idc={idc})"); break; }
        let ids: Vec<u32> = (0..idc).map(|k| u32at(&b, o + k * 4)).collect();
        o += idc * 4;
        let w0 = u32at(&b, o); let num_tex = u32at(&b, o + 4);
        let tex_begin = u32at(&b, o + 8); let w3 = u32at(&b, o + 12);
        o += 16;
        if uid == want || ids.contains(&want) {
            println!("record #{i} @0x{rec:x}  size {} bytes", o - rec);
            println!("  +0x00 uid        {uid:08X}");
            println!("  +0x04 numAlias   {idc}");
            for (k, id) in ids.iter().enumerate() { println!("  +0x{:02X} alias[{k}]   {id:08X}", 8 + k * 4); }
            println!("  +0x{:02X} w0         {w0:08X} ({w0})", 8 + idc * 4);
            println!("  +0x{:02X} numTex     {num_tex}", 12 + idc * 4);
            println!("  +0x{:02X} texBegin   {tex_begin}", 16 + idc * 4);
            println!("  +0x{:02X} w3         {w3:08X}", 20 + idc * 4);
            for k in 0..num_tex as usize {
                let h = u32at(&b, wstx_off + (tex_begin as usize + k) * 4);
                println!("    WSTX[{}] @0x{:x} = {h:08X}", tex_begin as usize + k, wstx_off + (tex_begin as usize + k) * 4);
            }
            println!("  raw: {:02X?}", &b[rec..o.min(b.len())]);
        }
    }
}

/// Aggregate the whole chain over every mesh in the index (optionally filtered).
pub fn cmd_stats(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind stats <index.tsv> <France.materials> [mesh_substr]"); std::process::exit(2); }
    let rows = read_index(&args[0]);
    let wsao = wsao_open(&args[1]).unwrap();
    let filter = args.get(2).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let mut tex_by_hash: HashMap<u32, &IndexRow> = HashMap::new();
    let mut any_by_hash: HashMap<u32, &IndexRow> = HashMap::new();
    for r in &rows {
        any_by_hash.entry(pandemic_hash(&r.name)).or_insert(r);
        if r.kind.starts_with("TEX") { tex_by_hash.entry(pandemic_hash(&r.name)).or_insert(r); }
    }
    // dedupe meshes by (pack, entry, name)
    let mut seen_mesh: Vec<(String, usize, String)> = Vec::new();
    let mut packcache: HashMap<String, Vec<Entry>> = HashMap::new();
    let (mut n_mesh, mut n_err, mut n_dc) = (0usize, 0usize, 0usize);
    let (mut n_mat, mut n_mat_wsao, mut n_mat_zero) = (0usize, 0usize, 0usize);
    let (mut n_tex, mut n_tex_named, mut n_tex_local) = (0usize, 0usize, 0usize);
    let mut n_mat_isname = 0usize;
    let mut missing: Vec<(String, u32)> = Vec::new();
    let mut unknown_tex: Vec<(String, u32)> = Vec::new();
    for m in rows.iter().filter(|r| r.kind == "MESH" && r.name.to_ascii_lowercase().contains(&filter)) {
        let key = (m.pack.clone(), m.entry, m.name.clone());
        if seen_mesh.contains(&key) { continue; }
        seen_mesh.push(key);
        let entries = packcache.entry(m.pack.clone()).or_insert_with(|| megapack_index(&m.pack).unwrap());
        let mut f = std::fs::File::open(&m.pack).unwrap();
        let mut buf = Vec::new();
        if !read_entry(&mut f, &entries[m.entry], &mut buf) { continue; }
        let unc0 = u32at(&buf, m.off + 4) as usize;
        let c0 = u32at(&buf, m.off + 12) as usize;
        let Ok(body) = inflate(&buf[m.off + 276..m.off + 276 + c0], unc0) else { n_err += 1; continue };
        let Ok(dcs) = mesh_drawcalls(&body) else { n_err += 1; continue };
        n_mesh += 1;
        n_dc += dcs.len();
        let mut mats: Vec<u32> = dcs.iter().map(|d| d.1).collect();
        mats.sort(); mats.dedup();
        let local: Vec<u32> = dtex_records(&buf).iter().map(|t| pandemic_hash(&t.1)).collect();
        for mt in &mats {
            n_mat += 1;
            if any_by_hash.contains_key(mt) { n_mat_isname += 1; }
            match wsao.textures(*mt) {
                None => { if missing.len() < 60 { missing.push((m.name.clone(), *mt)); } }
                Some(ts) => {
                    n_mat_wsao += 1;
                    if ts.is_empty() { n_mat_zero += 1; }
                    for t in &ts {
                        n_tex += 1;
                        if tex_by_hash.contains_key(t) { n_tex_named += 1; }
                        else if !unknown_tex.contains(&(m.name.clone(), *t)) && unknown_tex.len() < 40 {
                            unknown_tex.push((m.name.clone(), *t));
                        }
                        if local.contains(t) { n_tex_local += 1; }
                    }
                }
            }
        }
    }
    println!("filter                       : {:?}", filter);
    println!("meshes parsed                : {n_mesh}   (mesh-parse errors: {n_err})");
    println!("drawcalls                    : {n_dc}");
    println!("distinct materials (per mesh): {n_mat}");
    println!("  found in WSAO              : {n_mat_wsao}  ({:.1}%)", pct(n_mat_wsao, n_mat));
    println!("  of those, numTex == 0      : {n_mat_zero}");
    println!("  matHash == hash(some asset): {n_mat_isname}  <- naming-shortcut test");
    println!("texture hashes yielded       : {n_tex}");
    println!("  name a DTEX we indexed     : {n_tex_named}  ({:.1}%)", pct(n_tex_named, n_tex));
    println!("  and it is in the SAME bundle as the mesh: {n_tex_local}  ({:.1}%)", pct(n_tex_local, n_tex));
    if !unknown_tex.is_empty() {
        println!("  texture hashes naming NO indexed DTEX (first {}):", unknown_tex.len());
        for (m, h) in &unknown_tex { println!("      {h:08X}  wanted by {m}"); }
    }
    if !missing.is_empty() {
        println!("  materials with NO WSMA record (first {}):", missing.len());
        for (m, h) in &missing { println!("      {h:08X}  in {m}"); }
    }
}

fn pct(a: usize, b: usize) -> f64 { if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 } }

/// Everything about ONE bundle: its DTEX names, its meshes, their drawcall materials, and what
/// WSAO says each material's textures are.
pub fn cmd_bundle(args: &[String]) {
    if args.len() < 4 { eprintln!("texbind bundle <index.tsv> <France.materials> <pack> <entryIdx>"); std::process::exit(2); }
    let rows = read_index(&args[0]);
    let wsao = wsao_open(&args[1]).unwrap();
    let pack = &args[2];
    let ei: usize = args[3].parse().unwrap();
    let mut by_hash: HashMap<u32, &IndexRow> = HashMap::new();
    for r in &rows { if r.kind.starts_with("TEX") { by_hash.entry(pandemic_hash(&r.name)).or_insert(r); } }

    let entries = megapack_index(pack).unwrap();
    let e = &entries[ei];
    let mut f = std::fs::File::open(pack).unwrap();
    let mut buf = Vec::new();
    read_entry(&mut f, e, &mut buf);
    println!("bundle {} entry {ei}: e.crc {:08X}  e.index(name_crc) {:08X}  size {}", short(pack), e.crc, e.index, e.size);
    let texs = dtex_records(&buf);
    println!("  {} DTEX records:", texs.len());
    for (_o, n, w, h, _fm) in &texs { println!("      {:08X}  {:<40} {}x{}", pandemic_hash(n), n, w, h); }
    for m in msha_records(&buf) {
        println!("  MESH {}  hash {:08X}", m.name, pandemic_hash(&m.name));
        let c0 = m.c0 as usize;
        let body = match inflate(&buf[m.off + 276..m.off + 276 + c0], m.unc0 as usize) {
            Ok(b) => b, Err(er) => { println!("      ! inflate {er}"); continue } };
        let dcs = match mesh_drawcalls(&body) { Ok(d) => d, Err(er) => { println!("      ! {er}"); continue } };
        let mut seen = Vec::new();
        for (pi, mat, pb) in &dcs {
            if seen.contains(mat) { continue; }
            seen.push(*mat);
            match wsao.textures(*mat) {
                None => println!("      prim{pi:3} bone{pb:4} mat {mat:08X} -> NOT IN WSAO"),
                Some(ts) => {
                    let names: Vec<String> = ts.iter().map(|t| match by_hash.get(t) {
                        Some(r) => r.name.clone(), None => format!("<{t:08X} unknown>") }).collect();
                    println!("      prim{pi:3} bone{pb:4} mat {mat:08X} -> [{}] {}", ts.len(), names.join(", "));
                }
            }
        }
    }
}

/// Raw megapack directory + each sub-pack's ALBS header, plus what it contains.
pub fn cmd_entries(args: &[String]) {
    if args.is_empty() { eprintln!("texbind entries <pack> [first] [last]"); std::process::exit(2); }
    let entries = megapack_index(&args[0]).unwrap();
    let first: usize = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let last: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(entries.len() - 1);
    let mut f = std::fs::File::open(&args[0]).unwrap();
    let mut buf = Vec::new();
    println!("{:>4} {:>9} {:>9} {:>10} {:>12} {:>9} {:>4} {:>4}  first-mesh",
        "i", "e.crc", "e.index", "size", "offset", "albs_crc", "msh", "tex");
    for i in first..=last.min(entries.len() - 1) {
        let e = &entries[i];
        if !read_entry(&mut f, e, &mut buf) { continue; }
        let (magic, crc) = if buf.len() >= 12 {
            (String::from_utf8_lossy(&buf[0..4]).into_owned(), u32at(&buf, 8))
        } else { ("??".into(), 0) };
        let ms = msha_records(&buf);
        let ts = dtex_records(&buf);
        println!("{:>4} {:08X} {:08X} {:>10} {:>12} {} {:08X} {:>4} {:>4}  {}",
            i, e.crc, e.index, e.size, e.offset, magic, crc, ms.len(), ts.len(),
            ms.first().map(|m| m.name.clone()).unwrap_or_default());
    }
}

/// Carve the embedded `AULB` GameTemplates DB out of a container (e.g.
/// `France/loosefiles_BinPC.pack`) so the existing `sab_gametemplates` reader can be pointed at it.
pub fn cmd_aulb(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind aulb <container> <out.wsd>"); std::process::exit(2); }
    let b = std::fs::read(&args[0]).unwrap();
    let mut found = 0;
    for i in 0..b.len().saturating_sub(8) {
        if &b[i..i + 4] == b"AULB" {
            let n = u32at(&b, i + 4);
            if n > 100 && n < 200_000 {
                println!("AULB @0x{i:x}  entry_count={n}");
                if found == 0 { std::fs::write(&args[1], &b[i..]).unwrap(); }
                found += 1;
            }
        }
    }
    println!("{found} plausible AULB block(s); first written to {}", args[1]);
}

/// pandemic_hash of each argument.
pub fn cmd_hash(args: &[String]) {
    for a in args { println!("{:08X}  {a}", pandemic_hash(a)); }
}

pub fn run(args: &[String]) {
    match args.first().map(|s| s.as_str()) {
        Some("entries") => cmd_entries(&args[1..]),
        Some("bundle") => cmd_bundle(&args[1..]),
        Some("wsma") => cmd_wsma(&args[1..]),
        Some("stats") => cmd_stats(&args[1..]),
        Some("slot0") => cmd_slot0(&args[1..]),
        Some("hash") => cmd_hash(&args[1..]),
        Some("hashfile") => cmd_hashfile(&args[1..]),
        Some("aulb") => cmd_aulb(&args[1..]),
        Some("index") => cmd_index(&args[1..]),
        Some("bind") => cmd_bind(&args[1..]),
        Some("where") => cmd_where(&args[1..]),
        Some("cover") => cmd_cover(&args[1..]),
        _ => {
            eprintln!("texbind index <out.tsv> <pack>...");
            eprintln!("texbind where <index.tsv> <substr>");
            eprintln!("texbind bind  <index.tsv> <France.materials> <mesh_substr>");
            eprintln!("texbind cover <index.tsv> <France.materials> <mesh_substr>");
            std::process::exit(2);
        }
    }
}

/// pandemic_hash of every line in a file (name dictionaries: template names, mesh names…).
pub fn cmd_hashfile(args: &[String]) {
    if args.is_empty() { eprintln!("texbind hashfile <names.txt>"); std::process::exit(2); }
    let s = std::fs::read_to_string(&args[0]).unwrap();
    for l in s.lines() {
        let l = l.trim();
        if !l.is_empty() { println!("{:08X}\t{l}", pandemic_hash(l)); }
    }
}

/// Is WSTX[texBegin] (slot 0 of a material's texture slice) always the colour/diffuse map?
/// Tests every material used by meshes matching a filter: resolves slot 0 to a DTEX name and
/// checks it does NOT carry a non-colour role suffix.
pub fn cmd_slot0(args: &[String]) {
    if args.len() < 2 { eprintln!("texbind slot0 <index.tsv> <France.materials> [mesh_substr]"); std::process::exit(2); }
    let rows = read_index(&args[0]);
    let wsao = wsao_open(&args[1]).unwrap();
    let filter = args.get(2).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let mut tex_by_hash: HashMap<u32, &IndexRow> = HashMap::new();
    for r in &rows { if r.kind.starts_with("TEX") { tex_by_hash.entry(pandemic_hash(&r.name)).or_insert(r); } }
    let nondiffuse = |n: &str| {
        let n = n.to_ascii_lowercase();
        n.ends_with("_nm") || n.ends_with("_n") || n.ends_with("_s") || n.ends_with("_wm") || n.ends_with("_mask")
    };
    let mut packcache: HashMap<String, Vec<Entry>> = HashMap::new();
    let mut seen_mesh: Vec<(String, usize, String)> = Vec::new();
    let (mut n, mut bad) = (0usize, 0usize);
    let (mut suffix_d, mut suffix_none) = (0usize, 0usize);
    let mut examples: Vec<String> = Vec::new();
    for m in rows.iter().filter(|r| r.kind == "MESH" && r.name.to_ascii_lowercase().contains(&filter)) {
        let key = (m.pack.clone(), m.entry, m.name.clone());
        if seen_mesh.contains(&key) { continue; }
        seen_mesh.push(key);
        let entries = packcache.entry(m.pack.clone()).or_insert_with(|| megapack_index(&m.pack).unwrap());
        let mut f = std::fs::File::open(&m.pack).unwrap();
        let mut buf = Vec::new();
        if !read_entry(&mut f, &entries[m.entry], &mut buf) { continue; }
        let unc0 = u32at(&buf, m.off + 4) as usize;
        let c0 = u32at(&buf, m.off + 12) as usize;
        let Ok(body) = inflate(&buf[m.off + 276..m.off + 276 + c0], unc0) else { continue };
        let Ok(dcs) = mesh_drawcalls(&body) else { continue };
        let mut mats: Vec<u32> = dcs.iter().map(|d| d.1).collect();
        mats.sort(); mats.dedup();
        for mt in &mats {
            let Some(ts) = wsao.textures(*mt) else { continue };
            let Some(&t0) = ts.first() else { continue };
            let Some(r) = tex_by_hash.get(&t0) else { continue };
            n += 1;
            if r.name.to_ascii_lowercase().ends_with("_d") || r.name.to_ascii_lowercase().ends_with("_d_ab") { suffix_d += 1; }
            else if !nondiffuse(&r.name) { suffix_none += 1; }
            if nondiffuse(&r.name) {
                bad += 1;
                if examples.len() < 15 { examples.push(format!("{} mat {mt:08X} slot0={}", m.name, r.name)); }
            }
        }
    }
    println!("slot-0 tests            : {n}");
    println!("  slot0 name ends _D    : {suffix_d}");
    println!("  slot0 name no role sfx: {suffix_none}   <- would be classified 'Other' by a suffix-only rule");
    println!("  slot0 IS a _N/_S/_WM  : {bad}  ({:.2}%)", pct(bad, n));
    for e in &examples { println!("      {e}"); }
}
