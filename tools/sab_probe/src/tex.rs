//! `sab_probe tex*` — where does a part mesh's texture come from?
//!
//! Read-only. Answers, from real bytes:
//!   * which megapack / sub-pack entry holds a given DTEX name,
//!   * which material hashes a MESH's drawcalls carry,
//!   * what `France.materials` (WSAO) maps those hashes to,
//!   * whether `pandemic_hash(dtexName)` reproduces the WSTX texture hashes.

use std::collections::HashMap;
use std::io::Read;

// ---------------------------------------------------------------------------
// small readers
// ---------------------------------------------------------------------------
fn u16a(b: &[u8], o: usize) -> u16 { u16::from_le_bytes([b[o], b[o + 1]]) }
fn u32a(b: &[u8], o: usize) -> u32 { u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) }
fn u64a(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

use sab_formats::pandemic_hash;

// ---------------------------------------------------------------------------
// megapack
// ---------------------------------------------------------------------------
#[derive(Clone, Copy)]
pub struct Ent { pub crc: u32, pub index: u32, pub size: u32, pub offset: u64 }

pub struct Pack { pub path: String, pub file: std::fs::File, pub ents: Vec<Ent> }

impl Pack {
    pub fn open(path: &str) -> Result<Pack, String> {
        let mut f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        let mut hdr = [0u8; 8];
        use std::io::Read as _;
        f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
        if &hdr[0..4] != b"00PM" { return Err(format!("{path}: not a megapack")); }
        let count = u32a(&hdr, 4) as usize;
        let mut idx = vec![0u8; count * 20];
        f.read_exact(&mut idx).map_err(|e| e.to_string())?;
        let ents = (0..count)
            .map(|i| Ent {
                crc: u32a(&idx, i * 20),
                index: u32a(&idx, i * 20 + 4),
                size: u32a(&idx, i * 20 + 8),
                offset: u64a(&idx, i * 20 + 12),
            })
            .collect();
        Ok(Pack { path: path.to_string(), file: f, ents })
    }
    pub fn read(&self, e: &Ent, buf: &mut Vec<u8>) -> usize {
        let n = e.size as usize;
        if buf.len() < n { buf.resize(n, 0); }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            self.file.seek_read(&mut buf[..n], e.offset).unwrap_or(0)
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_at(&mut buf[..n], e.offset).unwrap_or(0)
        }
    }
}

// ---------------------------------------------------------------------------
// DTEX record scan (copied logic from sab_workshop/src/dtex.rs::find_records)
// ---------------------------------------------------------------------------
const DXT1: u32 = 0x3154_5844;
const DXT3: u32 = 0x3354_5844;
const DXT5: u32 = 0x3554_5844;

fn known_fmt(f: u32) -> bool {
    matches!(f, DXT1 | DXT3 | DXT5 | 0x15 | 0x14 | 0x3f | 0x16 | 0x28 | 0x1c | 0x32)
}

/// (offset, name, w, h, fmt) for every DTEX record in a sub-pack.
pub fn dtex_records(sub: &[u8]) -> Vec<(usize, String, u16, u16, u32)> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 26 < sub.len() {
        let f = u32a(sub, i);
        if known_fmt(f) {
            for nl in 1..80usize {
                if i < 4 + nl { break; }
                let lo = i - 4 - nl;
                if u32a(sub, lo) as usize == nl
                    && sub[lo + 4..lo + 4 + nl].iter().all(|&b| (0x20..0x7f).contains(&b))
                {
                    let name = String::from_utf8_lossy(&sub[lo + 4..lo + 4 + nl]).into_owned();
                    let w = u16a(sub, i + 8);
                    let h = u16a(sub, i + 10);
                    v.push((lo, name, w, h, f));
                    break;
                }
            }
        }
        i += 1;
    }
    v
}

// ---------------------------------------------------------------------------
// MESH drawcalls
// ---------------------------------------------------------------------------
pub struct MeshInfo {
    pub name: String,
    pub file_off: usize,
    pub num_bones0: u32,
    /// (primitiveIndex, materialHash, parentBone)
    pub draws: Vec<(u32, u32, u16)>,
    /// (indexOffset, numIndices) per primitive
    pub prims: Vec<(u32, u32)>,
}

/// Walk a decompressed MESH body to its DrawCall array. Offsets mirror
/// `sab_workshop/src/meshload.rs::parse_mesh`.
pub fn mesh_draws(body: &[u8]) -> Result<(Vec<(u32, u32, u16)>, Vec<(u32, u32)>), String> {
    if body.len() < 244 { return Err("body < 244".into()); }
    let num_bones0 = u32a(body, 204) as usize;
    let num_bone_remaps = u32a(body, 208) as usize;
    let num_streams = u16a(body, 216) as usize;
    let num_prims = u16a(body, 218) as usize;
    let num_draws = u32a(body, 232) as usize;

    let mut p = 244usize;
    // The MESHSkeleton block exists ONLY for skinned meshes. Unskinned assets (props, foliage,
    // e.g. DO_Flower_B_A: bodyLen 528 = 244 header + 284 tail) have NO skeleton block at all —
    // the Stream array starts immediately at 244. Proven by `tex-raw`.
    if num_bones0 > 1 {
        let num_unk0 = u32a(body, p) as usize;
        let num_bones = u32a(body, p + 12) as usize;
        let num_unk1 = u32a(body, p + 16);
        let nb3 = u32a(body, p + 20) as usize;
        let nb4 = u32a(body, p + 28) as usize;
        if num_bones != nb3 || num_bones != nb4 || num_bones != num_bones0 {
            return Err(format!("bone count mismatch {num_bones0}/{num_bones}/{nb3}/{nb4}"));
        }
        p += 44;
        p += num_bones;      // boneIds u8
        p += num_unk0;       // pad
        p += num_bones * 64; // localTMS
        p += num_bones * 64; // bones
        p += num_bones * 48; // RTS
        p += num_bones * 2;  // parentIds
        p += num_bones * 4;  // null32
        if num_unk1 != 0 { p += 2; }
        if num_bone_remaps > 0 {
            if p + 8 > body.len() { return Err("trunc at boneRemap".into()); }
            let guard = u32a(body, p) as usize;
            if guard != num_bone_remaps { return Err(format!("remap guard {guard}!={num_bone_remaps}")); }
            p += 8 + num_bone_remaps * 68;
        }
    }
    p += num_streams * 152;
    let mut prims = Vec::new();
    for _ in 0..num_prims {
        if p + 100 > body.len() { return Err("trunc at prim".into()); }
        prims.push((u32a(body, p + 88), u32a(body, p + 96)));
        p += 100;
    }
    let mut draws = Vec::new();
    for _ in 0..num_draws {
        if p + 16 > body.len() { return Err("trunc at draw".into()); }
        draws.push((u32a(body, p), u32a(body, p + 4), u16a(body, p + 12)));
        p += 16;
    }
    Ok((draws, prims))
}

fn inflate(data: &[u8], expected: usize) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(data);
    let mut out = Vec::with_capacity(expected);
    d.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Every MSHA in `sub` whose name contains `filter` (case-insensitive), with drawcalls parsed.
pub fn meshes_in(sub: &[u8], filter: &str) -> Vec<MeshInfo> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let flow = filter.to_ascii_lowercase();
    while i + 276 <= sub.len() {
        if &sub[i..i + 4] == b"AHSM" {
            let unc0 = u32a(sub, i + 4);
            let c0 = u32a(sub, i + 12);
            let nb = &sub[i + 20..i + 276];
            let end = nb.iter().position(|&b| b == 0).unwrap_or(0);
            if end > 0 && nb[..end].iter().all(|&b| (0x20..0x7f).contains(&b)) && c0 > 0 && unc0 > 0 {
                let name = String::from_utf8_lossy(&nb[..end]).into_owned();
                if flow.is_empty() || name.to_ascii_lowercase().contains(&flow) {
                    let s = i + 276;
                    if s + c0 as usize <= sub.len() {
                        if let Some(body) = inflate(&sub[s..s + c0 as usize], unc0 as usize) {
                            if body.len() >= 244 {
                                let nb0 = u32a(&body, 204);
                                match mesh_draws(&body) {
                                    Ok((draws, prims)) => out.push(MeshInfo {
                                        name, file_off: i, num_bones0: nb0, draws, prims,
                                    }),
                                    Err(e) => out.push(MeshInfo {
                                        name: format!("{name}  [parse err: {e}]"),
                                        file_off: i, num_bones0: nb0,
                                        draws: Vec::new(), prims: Vec::new(),
                                    }),
                                }
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// WSAO (France.materials)
// ---------------------------------------------------------------------------
pub struct Wsao {
    pub wstx: Vec<u32>,
    /// materialHash -> (textureBegin, numTextures)
    pub by_mat: HashMap<u32, (u32, u32)>,
    pub n_wsma: usize,
    pub blocks: Vec<(String, usize)>,
}

impl Wsao {
    pub fn open(path: &str) -> Result<Wsao, String> {
        let b = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
        if b.len() < 80 || &b[0..4] != b"OASW" { return Err("not WSAO".into()); }
        let num_wstx = u32a(&b, 4 * 17) as usize;
        let num_wsma = u32a(&b, 8) as usize;
        let mut blocks = Vec::new();
        for m in ["OASW", "WSST", "XTSW", "APSW", "AMSW", "PCSW", "PPSW", "PVSW"] {
            if let Some(o) = b.windows(4).position(|w| w == m.as_bytes()) {
                blocks.push((m.to_string(), o));
            }
        }
        let find = |m: &[u8; 4]| b.windows(4).position(|w| w == m).ok_or("block missing");
        let wstx_off = find(b"XTSW")? + 4;
        let wsma_off = find(b"AMSW")? + 4;
        let wstx: Vec<u32> = (0..num_wstx).map(|i| u32a(&b, wstx_off + i * 4)).collect();
        let mut by_mat: HashMap<u32, (u32, u32)> = HashMap::new();
        let mut o = wsma_off;
        for _ in 0..num_wsma {
            if o + 8 > b.len() { break; }
            let uid = u32a(&b, o);
            let idc = u32a(&b, o + 4) as usize;
            o += 8;
            if idc > 64 || o + idc * 4 + 16 > b.len() { break; }
            let ids: Vec<u32> = (0..idc).map(|i| u32a(&b, o + i * 4)).collect();
            o += idc * 4;
            let num_tex = u32a(&b, o + 4);
            let tex_begin = u32a(&b, o + 8);
            o += 16;
            by_mat.entry(uid).or_insert((tex_begin, num_tex));
            for id in ids { by_mat.entry(id).or_insert((tex_begin, num_tex)); }
        }
        Ok(Wsao { wstx, by_mat, n_wsma: num_wsma, blocks })
    }
    pub fn textures(&self, mat: u32) -> Option<Vec<u32>> {
        let &(b, n) = self.by_mat.get(&mat)?;
        let (s, e) = (b as usize, (b + n) as usize);
        (e <= self.wstx.len()).then(|| self.wstx[s..e].to_vec())
    }
}

// ---------------------------------------------------------------------------
// DTEX name index (cached)
// ---------------------------------------------------------------------------
pub struct TexLoc { pub pack: String, pub entry: usize, pub crc: u32, pub name: String, pub w: u16, pub h: u16 }

pub fn build_index(packs: &[String], cache: &str) -> Vec<TexLoc> {
    if let Ok(s) = std::fs::read_to_string(cache) {
        let mut v = Vec::new();
        for l in s.lines() {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() == 6 {
                v.push(TexLoc {
                    pack: f[0].into(), entry: f[1].parse().unwrap_or(0),
                    crc: u32::from_str_radix(f[2], 16).unwrap_or(0), name: f[3].into(),
                    w: f[4].parse().unwrap_or(0), h: f[5].parse().unwrap_or(0),
                });
            }
        }
        if !v.is_empty() {
            eprintln!("[*] dtex index: {} records (cached {cache})", v.len());
            return v;
        }
    }
    let mut v = Vec::new();
    let mut buf = Vec::new();
    for p in packs {
        let Ok(pk) = Pack::open(p) else { eprintln!("[!] skip {p}"); continue };
        let short = p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
        eprintln!("[*] scanning {short} ({} entries)", pk.ents.len());
        for (i, e) in pk.ents.iter().enumerate() {
            let n = pk.read(e, &mut buf);
            if n == 0 { continue; }
            for (_, name, w, h, _) in dtex_records(&buf[..n]) {
                v.push(TexLoc { pack: short.clone(), entry: i, crc: e.crc, name, w, h });
            }
        }
    }
    let mut s = String::new();
    for t in &v {
        s.push_str(&format!("{}\t{}\t{:08X}\t{}\t{}\t{}\n", t.pack, t.entry, t.crc, t.name, t.w, t.h));
    }
    let _ = std::fs::write(cache, s);
    eprintln!("[*] dtex index: {} records (written to {cache})", v.len());
    v
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------
fn packs_of(root: &str) -> Vec<String> {
    let mut v = Vec::new();
    // `SAB_PROBE_DLC=1` also folds in the DLC's own megapacks (DLC/01), which the workshop does
    // not currently open.
    let dirs: Vec<String> = if std::env::var("SAB_PROBE_DLC").is_ok() {
        vec![format!("{root}/Global"), format!("{root}/DLC/01")]
    } else {
        vec![format!("{root}/Global")]
    };
    for g in dirs {
        if let Ok(rd) = std::fs::read_dir(&g) {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.to_ascii_lowercase().ends_with(".megapack"))
                .collect();
            names.sort();
            for n in names { v.push(format!("{g}/{n}")); }
        }
    }
    v
}

fn cache_path() -> String {
    let d = std::env::var("SAB_PROBE_CACHE").unwrap_or_else(|_| ".".into());
    format!("{d}/dtex_index.tsv")
}

/// `tex-find <root> <substr>` — every DTEX whose NAME contains substr, and where it lives.
pub fn cmd_find(root: &str, sub: &str) {
    let idx = build_index(&packs_of(root), &cache_path());
    let low = sub.to_ascii_lowercase();
    let mut hits: Vec<&TexLoc> = idx.iter().filter(|t| t.name.to_ascii_lowercase().contains(&low)).collect();
    hits.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
    let mut seen: Vec<String> = Vec::new();
    println!("# DTEX names containing \"{sub}\"");
    println!("name\thash\tpack\tentry\tentryCrc\tsize");
    for t in &hits {
        let k = format!("{}|{}|{}", t.name.to_ascii_lowercase(), t.pack, t.entry);
        if seen.contains(&k) { continue; }
        seen.push(k);
        println!("{}\t{:08X}\t{}\t{}\t{:08X}\t{}x{}", t.name, pandemic_hash(&t.name), t.pack, t.entry, t.crc, t.w, t.h);
    }
    println!("# {} record(s), {} unique placements", hits.len(), seen.len());
}

/// `tex-mesh <root> <meshNameSubstr>` — drawcall materials -> WSAO -> texture hashes -> DTEX names.
pub fn cmd_mesh(root: &str, filter: &str) {
    let idx = build_index(&packs_of(root), &cache_path());
    let mut by_hash: HashMap<u32, Vec<&TexLoc>> = HashMap::new();
    for t in &idx { by_hash.entry(pandemic_hash(&t.name)).or_default().push(t); }

    let wsao = match Wsao::open(&format!("{root}/France.materials")) {
        Ok(w) => { eprintln!("[*] WSAO: {} WSMA records, {} WSTX hashes, blocks {:?}", w.n_wsma, w.wstx.len(), w.blocks); Some(w) }
        Err(e) => { eprintln!("[!] WSAO unavailable: {e}"); None }
    };

    let mut buf = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        let short = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
        for (ei, e) in pk.ents.iter().enumerate() {
            let n = pk.read(e, &mut buf);
            if n == 0 { continue; }
            if !contains_ci(&buf[..n], filter) { continue; }
            let ms = meshes_in(&buf[..n], filter);
            if ms.is_empty() { continue; }
            // textures co-located in this same sub-pack
            let local: Vec<String> = dtex_records(&buf[..n]).into_iter().map(|r| r.1).collect();
            for m in &ms {
                if done.contains(&m.name) { continue; }
                done.push(m.name.clone());
                println!("\n=== {} ===", m.name);
                println!("  pack={short} entry={ei} entryCrc={:08X} entrySize={} numBones0={} draws={} prims={}",
                    e.crc, e.size, m.num_bones0, m.draws.len(), m.prims.len());
                println!("  co-located DTEX in this sub-pack: {}", local.len());
                let mut mats: Vec<u32> = m.draws.iter().map(|d| d.1).collect();
                mats.sort_unstable(); mats.dedup();
                println!("  distinct material hashes: {}", mats.len());
                for mh in &mats {
                    let mut line = format!("   mat {mh:08X} -> ");
                    match wsao.as_ref().and_then(|w| w.textures(*mh)) {
                        None => line.push_str("(no WSMA record)"),
                        Some(th) => {
                            let parts: Vec<String> = th.iter().map(|h| {
                                match by_hash.get(h) {
                                    None => format!("{h:08X}=?"),
                                    Some(l) => {
                                        let nm = &l[0].name;
                                        let here = local.iter().any(|x| x.eq_ignore_ascii_case(nm));
                                        let where_ = if here { "SAME-SUBPACK".to_string() }
                                            else {
                                                let mut w: Vec<String> = l.iter().map(|t| format!("{}#{}", t.pack, t.entry)).collect();
                                                w.dedup(); w.truncate(3); w.join(",")
                                            };
                                        format!("{h:08X}={nm} [{where_}]")
                                    }
                                }
                            }).collect();
                            line.push_str(&format!("{} tex: {}", th.len(), parts.join("  ")));
                        }
                    }
                    println!("{line}");
                }
            }
        }
    }
    if done.is_empty() { println!("no MSHA matched \"{filter}\""); }
}

pub fn contains_ci(hay: &[u8], needle: &str) -> bool {
    let n = needle.as_bytes();
    if n.is_empty() || hay.len() < n.len() { return false; }
    hay.windows(n.len()).any(|w| w.iter().zip(n).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

/// `tex-wsma <root> <matHashHex>` — raw dump of the WSMA record(s) for a material hash.
pub fn cmd_wsma(root: &str, arg: &str) {
    let b = match std::fs::read(format!("{root}/France.materials")) {
        Ok(b) => b, Err(e) => { println!("read: {e}"); return; }
    };
    let num_wstx = u32a(&b, 4 * 17) as usize;
    let num_wsma = u32a(&b, 8) as usize;
    println!("# header u32[0..20]:");
    for i in 0..20 { print!("  [{i}]={}", u32a(&b, i * 4)); if i % 5 == 4 { println!(); } }
    println!();
    let find = |m: &[u8; 4]| b.windows(4).position(|w| w == m).unwrap_or(usize::MAX);
    let wstx_off = find(b"XTSW") + 4;
    let wsma_off = find(b"AMSW") + 4;
    println!("# numWSMA={num_wsma} numWSTX={num_wstx} wsmaOff={wsma_off:#x} wstxOff={wstx_off:#x}");
    let want = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap_or(0);
    let mut o = wsma_off;
    let mut n = 0usize;
    let mut idc_hist: std::collections::BTreeMap<usize, usize> = Default::default();
    let mut dup = 0usize;
    let mut seen: HashMap<u32, usize> = HashMap::new();
    while n < num_wsma && o + 8 <= b.len() {
        let rec_start = o;
        let uid = u32a(&b, o);
        let idc = u32a(&b, o + 4) as usize;
        o += 8;
        if idc > 64 || o + idc * 4 + 16 > b.len() { println!("# STOP at rec {n}, idc={idc}"); break; }
        let ids: Vec<u32> = (0..idc).map(|i| u32a(&b, o + i * 4)).collect();
        o += idc * 4;
        let t0 = u32a(&b, o);
        let num_tex = u32a(&b, o + 4);
        let tex_begin = u32a(&b, o + 8);
        let t3 = u32a(&b, o + 12);
        o += 16;
        *idc_hist.entry(idc).or_default() += 1;
        if seen.insert(uid, n).is_some() { dup += 1; }
        if uid == want || ids.contains(&want) {
            println!("\n# WSMA rec #{n} @{rec_start:#x} size={} bytes", o - rec_start);
            println!("  uid={uid:08X} idCount={idc} ids={:08X?}", ids);
            println!("  tail: [0]={t0:08X} numTex={num_tex} texBegin={tex_begin} [3]={t3:08X}");
            let s = tex_begin as usize;
            for k in 0..num_tex as usize {
                if s + k < num_wstx { println!("    wstx[{}] = {:08X}", s + k, u32a(&b, wstx_off + (s + k) * 4)); }
            }
            println!("  raw: {:02X?}", &b[rec_start..o]);
        }
        n += 1;
    }
    println!("\n# parsed {n} WSMA records, ended at {o:#x}; duplicate uids: {dup}");
    println!("# idCount histogram: {idc_hist:?}");
}

/// `tex-cover <root> <meshNameSubstr>` — per-mesh coverage of the WSAO path (no per-material spam).
pub fn cmd_cover(root: &str, filter: &str) {
    let idx = build_index(&packs_of(root), &cache_path());
    let mut by_hash: HashMap<u32, Vec<&TexLoc>> = HashMap::new();
    for t in &idx { by_hash.entry(pandemic_hash(&t.name)).or_default().push(t); }
    let wsao = Wsao::open(&format!("{root}/France.materials")).ok();

    println!("mesh\tpack#entry\tdraws\tmats\tmatsNoWSMA\tmats0tex\ttexRefs\ttexNamed\ttexSameSubpack\ttexMissing\tdiffuseAt0");
    let mut buf = Vec::new();
    let mut done: Vec<String> = Vec::new();
    let (mut tot_mat, mut tot_nowsma, mut tot_zero, mut tot_ref, mut tot_named, mut tot_same, mut tot_d0, mut tot_dn) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        let short = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
        if short.starts_with("patch") { continue; }
        for (ei, e) in pk.ents.iter().enumerate() {
            let n = pk.read(e, &mut buf);
            if n == 0 || !contains_ci(&buf[..n], filter) { continue; }
            let ms = meshes_in(&buf[..n], filter);
            if ms.is_empty() { continue; }
            let local: Vec<String> = dtex_records(&buf[..n]).into_iter().map(|r| r.1).collect();
            for m in &ms {
                if done.contains(&m.name) { continue; }
                done.push(m.name.clone());
                let mut mats: Vec<u32> = m.draws.iter().map(|d| d.1).collect();
                mats.sort_unstable(); mats.dedup();
                let (mut nowsma, mut zero, mut refs, mut named, mut same, mut missing) = (0, 0, 0, 0, 0, 0);
                let (mut d0, mut dn) = (0usize, 0usize);
                for mh in &mats {
                    match wsao.as_ref().and_then(|w| w.textures(*mh)) {
                        None => nowsma += 1,
                        Some(th) if th.is_empty() => zero += 1,
                        Some(th) => {
                            refs += th.len();
                            let mut names: Vec<Option<&str>> = Vec::new();
                            for h in &th {
                                match by_hash.get(h) {
                                    None => { missing += 1; names.push(None); }
                                    Some(l) => {
                                        named += 1;
                                        let nm = l[0].name.as_str();
                                        if local.iter().any(|x| x.eq_ignore_ascii_case(nm)) { same += 1; }
                                        names.push(Some(nm));
                                    }
                                }
                            }
                            // is slot 0 the "colour" texture (name not suffixed _N/_NM/_S/_MASK/_WM)?
                            if let Some(Some(n0)) = names.first() {
                                let l = n0.to_ascii_lowercase();
                                let suffixed = l.ends_with("_n") || l.ends_with("_nm") || l.ends_with("_s")
                                    || l.ends_with("_mask") || l.ends_with("_wm");
                                if suffixed { dn += 1 } else { d0 += 1 }
                            }
                        }
                    }
                }
                println!("{}\t{}#{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}/{}",
                    m.name, short, ei, m.draws.len(), mats.len(), nowsma, zero, refs, named, same, missing, d0, d0 + dn);
                tot_mat += mats.len(); tot_nowsma += nowsma; tot_zero += zero;
                tot_ref += refs; tot_named += named; tot_same += same; tot_d0 += d0; tot_dn += dn;
            }
        }
    }
    println!("# TOTALS meshes={} mats={} noWSMA={} zeroTex={} texRefs={} named={} sameSubpack={} missing={} slot0-is-colour={}/{}",
        done.len(), tot_mat, tot_nowsma, tot_zero, tot_ref, tot_named, tot_same, tot_ref - tot_named, tot_d0, tot_d0 + tot_dn);
}

/// `tex-names <root> <substr>` — every MSHA asset name (header only, no inflate).
pub fn cmd_names(root: &str, filter: &str) {
    let mut buf = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let flow = filter.to_ascii_lowercase();
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        let short = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
        for (ei, e) in pk.ents.iter().enumerate() {
            let n = pk.read(e, &mut buf);
            let sub = &buf[..n];
            let mut i = 0usize;
            while i + 276 <= sub.len() {
                if &sub[i..i + 4] == b"AHSM" {
                    let nb = &sub[i + 20..i + 276];
                    let end = nb.iter().position(|&b| b == 0).unwrap_or(0);
                    if end > 0 && nb[..end].iter().all(|&b| (0x20..0x7f).contains(&b)) {
                        let name = String::from_utf8_lossy(&nb[..end]).into_owned();
                        if flow.is_empty() || name.to_ascii_lowercase().contains(&flow) {
                            let k = name.to_ascii_lowercase();
                            if !seen.contains(&k) { seen.push(k); println!("{name}\t{short}#{ei}"); }
                        }
                    }
                }
                i += 1;
            }
        }
    }
    println!("# {} distinct MSHA names", seen.len());
}

/// `tex-raw <root> <meshName>` — MESH header counts + walk trace, to pin unskinned layouts.
pub fn cmd_raw(root: &str, filter: &str) {
    let mut buf = Vec::new();
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        for (ei, e) in pk.ents.iter().enumerate() {
            let n = pk.read(e, &mut buf);
            let sub = &buf[..n];
            if !contains_ci(sub, filter) { continue; }
            let mut i = 0usize;
            while i + 276 <= sub.len() {
                if &sub[i..i + 4] == b"AHSM" {
                    let unc0 = u32a(sub, i + 4); let c0 = u32a(sub, i + 12);
                    let nb = &sub[i + 20..i + 276];
                    let end = nb.iter().position(|&b| b == 0).unwrap_or(0);
                    if end > 0 && String::from_utf8_lossy(&nb[..end]).eq_ignore_ascii_case(filter) {
                        let s = i + 276;
                        if let Some(body) = inflate(&sub[s..s + c0 as usize], unc0 as usize) {
                            println!("{filter} @entry {ei} bodyLen={}", body.len());
                            println!("  numBones0={} numBoneRemaps={} numStreams={} numPrims={} numDraws={}",
                                u32a(&body, 204), u32a(&body, 208), u16a(&body, 216), u16a(&body, 218), u32a(&body, 232));
                            println!("  skel hdr @244: {:?}", (0..11).map(|k| u32a(&body, 244 + k * 4)).collect::<Vec<_>>());
                            // brute force: find the offset where a Stream/Prim/Draw tail parses out to exactly bodyLen
                            let (ns, np, nd) = (u16a(&body, 216) as usize, u16a(&body, 218) as usize, u32a(&body, 232) as usize);
                            let tail = ns * 152 + np * 100 + nd * 16;
                            println!("  tail size = {tail}; body-tail = {}", body.len() as i64 - tail as i64);
                            let base = body.len() - tail;
                            let mut q = base + ns * 152;
                            let mut prims = Vec::new();
                            for _ in 0..np { prims.push((u32a(&body, q + 88), u32a(&body, q + 96))); q += 100; }
                            println!("  prims(from end) = {prims:?}");
                            let mut draws = Vec::new();
                            for _ in 0..nd { draws.push((u32a(&body, q), u32a(&body, q + 4))); q += 16; }
                            println!("  draws(from end) = {:08X?}", draws);
                            return;
                        }
                    }
                }
                i += 1;
            }
        }
    }
    println!("not found");
}

/// `tex-alt <root> <meshNameSubstr>` — test the ALTERNATIVE hypotheses against the WSAO answer:
///   H1 materialHash == pandemic_hash(a DTEX name)          (direct name-hash link)
///   H2 materialHash == pandemic_hash(the mesh's own name)  (mesh-name link)
///   H3 texture lives in the same sub-pack as the mesh      (bundle co-location)
///   H4 WSAO gives the answer                               (material -> WSTX -> DTEX hash)
pub fn cmd_alt(root: &str, filter: &str) {
    let idx = build_index(&packs_of(root), &cache_path());
    let mut by_hash: HashMap<u32, Vec<&TexLoc>> = HashMap::new();
    for t in &idx { by_hash.entry(pandemic_hash(&t.name)).or_default().push(t); }
    let wsao = Wsao::open(&format!("{root}/France.materials")).ok();

    let (mut mats, mut h1, mut h2, mut h4) = (0usize, 0usize, 0usize, 0usize);
    let (mut refs, mut h3_same, mut named) = (0usize, 0usize, 0usize);
    let mut meshes = 0usize;
    let mut parse_err = 0usize;
    let mut buf = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        let short = p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string();
        if short.starts_with("patch") { continue; }
        for e in pk.ents.iter() {
            let n = pk.read(e, &mut buf);
            if n == 0 || !contains_ci(&buf[..n], filter) { continue; }
            let ms = meshes_in(&buf[..n], filter);
            if ms.is_empty() { continue; }
            let local: Vec<String> = dtex_records(&buf[..n]).into_iter().map(|r| r.1).collect();
            for m in &ms {
                if done.contains(&m.name) { continue; }
                done.push(m.name.clone());
                if m.name.contains("[parse err") { parse_err += 1; continue; }
                meshes += 1;
                let mesh_h = pandemic_hash(&m.name);
                let mut mm: Vec<u32> = m.draws.iter().map(|d| d.1).collect();
                mm.sort_unstable(); mm.dedup();
                for mh in &mm {
                    mats += 1;
                    if by_hash.contains_key(mh) { h1 += 1; }
                    if *mh == mesh_h { h2 += 1; }
                    if let Some(th) = wsao.as_ref().and_then(|w| w.textures(*mh)) {
                        if !th.is_empty() { h4 += 1; }
                        for h in &th {
                            refs += 1;
                            if let Some(l) = by_hash.get(h) {
                                named += 1;
                                if local.iter().any(|x| x.eq_ignore_ascii_case(&l[0].name)) { h3_same += 1; }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("meshes={meshes} (parse-failed {parse_err}) distinctMaterials={mats}");
    println!("H1 materialHash is also a DTEX name-hash : {h1}/{mats}");
    println!("H2 materialHash == hash(mesh name)       : {h2}/{mats}");
    println!("H4 materialHash has a WSMA with >=1 tex  : {h4}/{mats}");
    println!("   texture refs={refs} resolved-to-a-DTEX-name={named}  of which co-located with the mesh: {h3_same}");
    println!("H3 bundle co-location holds for {h3_same}/{named} resolved refs");
}

/// `tex-key <root> <meshNameSubstr>` — is a drawcall's material hash the WSMA record's `uid`
/// or its `ids[]` entry? And where does the "slot 0 is the colour map" rule break?
pub fn cmd_key(root: &str, filter: &str) {
    let b = std::fs::read(format!("{root}/France.materials")).expect("France.materials");
    let num_wstx = u32a(&b, 4 * 17) as usize;
    let num_wsma = u32a(&b, 8) as usize;
    let find = |m: &[u8; 4]| b.windows(4).position(|w| w == m).unwrap();
    let wstx_off = find(b"XTSW") + 4;
    let wsma_off = find(b"AMSW") + 4;
    let wstx: Vec<u32> = (0..num_wstx).map(|i| u32a(&b, wstx_off + i * 4)).collect();
    let mut by_uid: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut by_id: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut o = wsma_off;
    for _ in 0..num_wsma {
        let uid = u32a(&b, o);
        let idc = u32a(&b, o + 4) as usize;
        o += 8;
        let ids: Vec<u32> = (0..idc).map(|i| u32a(&b, o + i * 4)).collect();
        o += idc * 4;
        let num_tex = u32a(&b, o + 4);
        let tex_begin = u32a(&b, o + 8);
        o += 16;
        by_uid.insert(uid, (tex_begin, num_tex));
        for id in ids { by_id.insert(id, (tex_begin, num_tex)); }
    }
    println!("# WSMA: {} distinct uid, {} distinct ids", by_uid.len(), by_id.len());
    let overlap = by_uid.keys().filter(|k| by_id.contains_key(k)).count();
    println!("# uid values that are also an id value: {overlap}");

    let idx = build_index(&packs_of(root), &cache_path());
    let mut by_hash: HashMap<u32, &str> = HashMap::new();
    for t in &idx { by_hash.entry(pandemic_hash(&t.name)).or_insert(&t.name); }

    let (mut only_id, mut only_uid, mut both, mut neither) = (0usize, 0usize, 0usize, 0usize);
    let mut slot0_bad: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    let mut done: Vec<String> = Vec::new();
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        if p.to_ascii_lowercase().contains("patchdynamic") { continue; }
        for e in pk.ents.iter() {
            let n = pk.read(e, &mut buf);
            if n == 0 || !contains_ci(&buf[..n], filter) { continue; }
            for m in meshes_in(&buf[..n], filter) {
                if done.contains(&m.name) || m.name.contains("[parse err") { continue; }
                done.push(m.name.clone());
                let mut mm: Vec<u32> = m.draws.iter().map(|d| d.1).collect();
                mm.sort_unstable(); mm.dedup();
                for mh in mm {
                    match (by_id.get(&mh), by_uid.get(&mh)) {
                        (Some(_), Some(_)) => both += 1,
                        (Some(_), None) => only_id += 1,
                        (None, Some(_)) => only_uid += 1,
                        (None, None) => neither += 1,
                    }
                    if let Some(&(beg, n)) = by_id.get(&mh) {
                        if n > 0 && (beg as usize) < wstx.len() {
                            if let Some(nm) = by_hash.get(&wstx[beg as usize]) {
                                let l = nm.to_ascii_lowercase();
                                if l.ends_with("_n") || l.ends_with("_nm") || l.ends_with("_s")
                                    || l.ends_with("_mask") || l.ends_with("_wm") {
                                    let s = format!("{} mat {mh:08X} slot0={nm}", m.name);
                                    if !slot0_bad.contains(&s) { slot0_bad.push(s); }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("# drawcall material hashes: id-only={only_id} uid-only={only_uid} both={both} neither={neither}");
    println!("# slot-0 exceptions ({}):", slot0_bad.len());
    for s in slot0_bad.iter().take(50) { println!("   {s}"); }
}

/// `tex-hash <root> <hex|name>` — pandemic_hash a name, or find a loose game-root file whose
/// stem hashes to the given value.
pub fn cmd_hash(root: &str, arg: &str) {
    if let Ok(h) = u32::from_str_radix(arg.trim_start_matches("0x"), 16) {
        if arg.len() == 8 {
            println!("# looking for a name hashing to {h:08X}");
            for dir in [root.to_string(), format!("{root}/Global"), format!("{root}/DLC/01")] {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let f = e.file_name().to_string_lossy().into_owned();
                        let stem = f.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or(f.clone());
                        for cand in [f.clone(), stem.clone()] {
                            if pandemic_hash(&cand) == h { println!("  MATCH {dir}/{f}  (as \"{cand}\")"); }
                        }
                    }
                }
            }
            println!("# done");
            return;
        }
    }
    println!("pandemic_hash(\"{arg}\") = {:08X}", pandemic_hash(arg));
}

/// `tex-prim <root> <meshNameSubstr>` — per PRIMITIVE (index range): which drawcalls target it,
/// and does every one of their materials resolve to the SAME slot-0 (colour) texture?
pub fn cmd_prim(root: &str, filter: &str) {
    let idx = build_index(&packs_of(root), &cache_path());
    let mut by_hash: HashMap<u32, String> = HashMap::new();
    for t in &idx { by_hash.entry(pandemic_hash(&t.name)).or_insert(t.name.clone()); }
    let wsao = Wsao::open(&format!("{root}/France.materials")).ok();
    let mut buf = Vec::new();
    let mut done: Vec<String> = Vec::new();
    let (mut agree, mut disagree) = (0usize, 0usize);
    for p in packs_of(root) {
        let Ok(pk) = Pack::open(&p) else { continue };
        if p.to_ascii_lowercase().contains("patchdynamic") { continue; }
        for e in pk.ents.iter() {
            let n = pk.read(e, &mut buf);
            if n == 0 || !contains_ci(&buf[..n], filter) { continue; }
            for m in meshes_in(&buf[..n], filter) {
                if done.contains(&m.name) || m.name.contains("[parse err") { continue; }
                done.push(m.name.clone());
                println!("\n=== {} ({} prims, {} draws) ===", m.name, m.prims.len(), m.draws.len());
                for (pi, (io, ni)) in m.prims.iter().enumerate() {
                    let mine: Vec<u32> = m.draws.iter().filter(|d| d.0 as usize == pi).map(|d| d.1).collect();
                    let mut tex0: Vec<String> = Vec::new();
                    for mh in &mine {
                        let t = wsao.as_ref().and_then(|w| w.textures(*mh)).unwrap_or_default();
                        let s = t.first().map(|h| by_hash.get(h).cloned().unwrap_or(format!("{h:08X}?")))
                            .unwrap_or_else(|| "(none)".into());
                        if !tex0.contains(&s) { tex0.push(s); }
                    }
                    let real: Vec<&String> = tex0.iter().filter(|s| *s != "(none)").collect();
                    if real.len() <= 1 { agree += 1 } else { disagree += 1 }
                    println!("  prim {pi}: indices [{io}..{}) draws={} mats={:08X?} slot0={:?}",
                        io + ni, mine.len(), mine, tex0);
                }
            }
        }
    }
    println!("\n# prims whose drawcalls agree on the colour texture: {agree}; disagree: {disagree}");
}
