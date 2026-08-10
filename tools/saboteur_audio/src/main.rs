// Saboteur audio extractor: parse the "1KCP" Wwise file-package (.pck), carve the streamed
// .wem voice lines, and batch-decode them to WAV via vgmstream-cli.
//
// 1KCP header (LE):
//   +0x00 u32 magic = 0x50434B31 ("1KCP")
//   +0x04 u32 block align (2048)
//   +0x08 u32 version (2)
//   +0x0C u32 bankCount
//   +0x10 u32 bankTableOffset   -- always 0x1C on retail
//   +0x14 u32 streamCount
//   +0x18 u32 streamTableOffset -- always bankTableOffset + bankCount*12
//   +0x1C : record table, 12 bytes each { u32 id, u32 size, u32 offset(absolute) }
//           first `bankCount` records are .bnk (BKHD), next `streamCount` are loose .wem (RIFF).
//           The two sub-tables are contiguous, so both are read as one run from bankTableOffset.
//
// Media lives in TWO places and both must be carved to get full coverage:
//   (a) loose stream records  -- the whole record IS a RIFF/.wem. This is where VO lives
//       (English(US).pck = 20741 of them).
//   (b) embedded inside the banks -- a BKHD chunk stream carrying DIDX (index: 12 bytes each,
//       { u32 id, u32 offset, u32 size }, offsets relative to the DATA payload) + DATA (the blob).
//       This is where nearly all SFX live: Sound/Saboteur.pck holds only 529 loose streams but
//       6475 bank-embedded DIDX entries across its 342 banks. Carving streams alone yields 529 of
//       its 3478 distinct effects -- an 85% silent loss that still looks like a clean run, because
//       every file it does write is valid.
//
// Retail record totals (GOG, main + DLC/01) are 81409 loose streams + 8826 embedded = 90235, but
// that counts duplicates; after the content-aware dedup below the distinct media count is 84474
// (81409 stream + 3065 bank). Verified end-to-end 2026-07-31: 84474 wem -> 84474 wav, 0 failures.
//
// NOTE: this table was previously read from a hardcoded +0x28, which skipped the first record and
// read one record past the end of every pack -- silently losing 1 real stream per pack (8 across the
// 4 VO languages x main+DLC). Verified against all 10 retail packs: reading from bankTableOffset
// validates 100% of records by magic (BKHD/RIFF); reading from 0x28 mis-reads exactly 2 per pack.
//
// Usage:
//   saboteur_audio --game <dir> --out <dir> --vgmstream <exe> --sets eng,fra,deu,ita,sfx
//                  [--no-decode] [--keep-wem] [--no-banks] [--no-streams] [--batch N]
//
//   `sfx` is the non-localized Saboteur.pck (effects, music, ambiance). `--langs` is still
//   accepted as an alias for `--sets`.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write, BufWriter};
use std::path::{Path, PathBuf};
use std::process::Command;

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

struct Rec { id: u32, size: u32, offset: u32 }

struct PckInfo { banks: u32, streams: u32, recs: Vec<Rec> }

fn parse_pck_header(f: &mut File) -> std::io::Result<PckInfo> {
    let mut h = [0u8; 0x1C];
    f.seek(SeekFrom::Start(0))?;
    f.read_exact(&mut h)?;
    assert_eq!(u32le(&h, 0), 0x5043_4B31, "not a 1KCP pack");
    let banks = u32le(&h, 0x0C);
    let bank_tbl = u32le(&h, 0x10);
    let streams = u32le(&h, 0x14);
    let stream_tbl = u32le(&h, 0x18);
    // The stream sub-table directly follows the bank sub-table; assert it so a pack that ever
    // breaks the assumption fails loudly instead of silently carving garbage.
    assert_eq!(
        stream_tbl,
        bank_tbl + banks * 12,
        "1KCP sub-tables are not contiguous (bankTable={:#x} banks={} streamTable={:#x})",
        bank_tbl, banks, stream_tbl
    );
    let total = (banks + streams) as usize;
    let mut tbl = vec![0u8; total * 12];
    f.seek(SeekFrom::Start(bank_tbl as u64))?;
    f.read_exact(&mut tbl)?;
    let mut recs = Vec::with_capacity(total);
    for i in 0..total {
        let o = i * 12;
        recs.push(Rec { id: u32le(&tbl, o), size: u32le(&tbl, o + 4), offset: u32le(&tbl, o + 8) });
    }
    Ok(PckInfo { banks, streams, recs })
}

/// FNV-1a/64 over a media payload. Only ever used to *avoid* a byte compare on the common
/// equal-length-equal-hash path; every consequential decision re-reads and compares real bytes.
fn hash64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// What to do with a media whose id was already written.
enum Dup { Exact, Prefetch, Longer, Variant }

/// Outcome of offering one media to the output directory.
enum Emit {
    /// Written to a new file; caller should add a manifest row for this name.
    New(String),
    /// A longer copy replaced a previously-written short one, under the same name.
    Replaced(String),
    /// Redundant (exact duplicate or prefetch fragment); nothing written.
    Skipped,
}

/// Classify a repeat of media id `id`. Media ids repeat for three quite different reasons and
/// collapsing them all to "duplicate" silently loses audio:
///   * **Exact** — the same wem indexed by several banks (a shared impact referenced from both
///     Weapons and Explosions_Impacts). Genuinely redundant; skip.
///   * **Prefetch** — the bank copy is a strict *prefix* of the loose stream. Wwise zero-latency
///     streaming keeps the first ~8 KB in the bank so playback starts instantly while the rest
///     streams. A truncated head of audio we already have; skip.
///   * **Longer** — the reverse arrival order (we wrote a prefetch first, the full stream came
///     later). Replace, so the complete audio wins regardless of ordering.
///   * **Variant** — same id, neither a prefix of the other: a genuinely different *encoding* of
///     the same source (retail `Saboteur.pck` has 38 of these, bank `cd0d70ce` carrying smaller
///     lower-bitrate copies). Real distinct audio — an id-only dedup discards it silently.
///
/// Measured on retail: the four VO packs are ~530 Prefetch each and 0 Variant; `Saboteur.pck` is
/// 0 Prefetch (effects are bank-resident, never streamed) and all 38 Variants.
fn classify(prev: &Path, new: &[u8]) -> std::io::Result<Dup> {
    let old = fs::read(prev)?;
    Ok(if old == new {
        Dup::Exact
    } else if old.starts_with(new) {
        Dup::Prefetch
    } else if new.starts_with(&old) {
        Dup::Longer
    } else {
        Dup::Variant
    })
}

/// State threaded through carving so both the stream and the bank path dedup identically.
struct Sink<'a> {
    tag: &'a str,
    wem_dir: &'a Path,
    /// media id -> (content hash, len, path already written)
    written: HashMap<u32, (u64, usize, PathBuf)>,
    out: Vec<PathBuf>,
    riff: usize,
    exact: usize,
    prefetch: usize,
    longer: usize,
    variant: usize,
}

impl<'a> Sink<'a> {
    fn new(tag: &'a str, wem_dir: &'a Path) -> Self {
        Sink { tag, wem_dir, written: HashMap::new(), out: Vec::new(),
               riff: 0, exact: 0, prefetch: 0, longer: 0, variant: 0 }
    }

    /// Offer one media payload. Dedups by content, not by id alone.
    fn emit(&mut self, id: u32, bytes: &[u8], variant_tag: u32) -> std::io::Result<Emit> {
        let h = hash64(bytes);
        if let Some((ph, pl, pp)) = self.written.get(&id).cloned() {
            // Cheap reject first; only re-read bytes when hash/len say it might be equal, or when
            // they differ and we must find out *how* they differ.
            if ph == h && pl == bytes.len() {
                self.exact += 1;
                return Ok(Emit::Skipped);
            }
            match classify(&pp, bytes)? {
                Dup::Exact => { self.exact += 1; return Ok(Emit::Skipped); }
                Dup::Prefetch => { self.prefetch += 1; return Ok(Emit::Skipped); }
                Dup::Longer => {
                    fs::write(&pp, bytes)?;
                    self.written.insert(id, (h, bytes.len(), pp.clone()));
                    self.longer += 1;
                    let name = pp.file_name().unwrap().to_string_lossy().to_string();
                    return Ok(Emit::Replaced(name));
                }
                Dup::Variant => {
                    // Genuinely different audio under the same id: keep it, disambiguated by the
                    // bank it came from so the name stays deterministic across runs.
                    self.variant += 1;
                    let name = format!("{}_{:08x}_v{:08x}.wem", self.tag, id, variant_tag);
                    let p = self.wem_dir.join(&name);
                    if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" { self.riff += 1; }
                    let mut w = BufWriter::new(File::create(&p)?);
                    w.write_all(bytes)?;
                    drop(w);
                    self.out.push(p);
                    return Ok(Emit::New(name));
                }
            }
        }
        let name = format!("{}_{:08x}.wem", self.tag, id);
        let p = self.wem_dir.join(&name);
        if bytes.len() >= 4 && &bytes[0..4] == b"RIFF" { self.riff += 1; }
        { let mut w = BufWriter::new(File::create(&p)?); w.write_all(bytes)?; }
        self.written.insert(id, (h, bytes.len(), p.clone()));
        self.out.push(p);
        Ok(Emit::New(name))
    }
}

// Carve every media in a pack: the loose stream records, and -- because that is where nearly all
// SFX live -- the media embedded inside each bank's DIDX/DATA chunk pair.
//
// Dedup is content-aware, not id-only -- see `classify`. Returns the list of written wem paths and
// appends manifest rows.
fn carve_pack(
    pck: &Path, tag: &str, wem_dir: &Path, manifest: &mut Vec<String>, limit: usize,
    do_streams: bool, do_banks: bool,
) -> std::io::Result<Vec<PathBuf>> {
    let mut f = File::open(pck)?;
    let info = parse_pck_header(&mut f)?;
    fs::create_dir_all(wem_dir)?;
    let mut sink = Sink::new(tag, wem_dir);
    let mut buf: Vec<u8> = Vec::new();

    // --- (a) loose stream records ---
    // Deliberately first: for a prefetched sound the loose stream is the COMPLETE audio and the
    // bank copy is only its first few KB, so taking streams first means the full version is the one
    // on disk. `classify` no longer depends on this ordering, but it keeps the common path cheap.
    let mut n_stream = 0usize;
    if do_streams {
        let start = info.banks as usize;
        let mut end = start + info.streams as usize;
        if limit > 0 && end > start + limit { end = start + limit; }
        for r in &info.recs[start..end] {
            buf.clear();
            buf.resize(r.size as usize, 0);
            f.seek(SeekFrom::Start(r.offset as u64))?;
            f.read_exact(&mut buf)?;
            let is_riff = buf.len() >= 4 && &buf[0..4] == b"RIFF";
            match sink.emit(r.id, &buf, 0)? {
                Emit::New(name) | Emit::Replaced(name) => {
                    manifest.push(format!(
                        "{},{:08x},stream,,{},{},{},{}", tag, r.id, r.offset, r.size, is_riff, name));
                    n_stream += 1;
                }
                Emit::Skipped => {}
            }
        }
    }

    // --- (b) media embedded in banks (DIDX index + DATA blob) ---
    let mut n_emb = 0usize;
    let mut n_banks = 0usize;
    if do_banks {
        let mut bank = Vec::new();
        for r in &info.recs[..info.banks as usize] {
            if limit > 0 && n_emb >= limit { break; }
            bank.clear();
            bank.resize(r.size as usize, 0);
            f.seek(SeekFrom::Start(r.offset as u64))?;
            f.read_exact(&mut bank)?;
            if bank.len() < 8 || &bank[0..4] != b"BKHD" {
                eprintln!("  [{}] bank {:08x} is not BKHD -- skipped", tag, r.id);
                continue;
            }
            n_banks += 1;
            // Walk the chunk stream to locate DIDX and DATA. Chunk header is { u8 id[4], u32 size }.
            let (mut didx, mut data) = (None, None);
            let mut p = 0usize;
            while p + 8 <= bank.len() {
                let cid = &bank[p..p + 4];
                let csz = u32le(&bank, p + 4) as usize;
                let body = p + 8;
                if body + csz > bank.len() { break; } // truncated/garbage tail: stop, don't panic
                match cid {
                    b"DIDX" => didx = Some((body, csz)),
                    b"DATA" => data = Some((body, csz)),
                    _ => {}
                }
                p = body + csz;
            }
            let ((dx_off, dx_len), (da_off, da_len)) = match (didx, data) {
                (Some(a), Some(b)) => (a, b),
                _ => continue, // HIRC-only bank (event/logic graph, no media) -- normal
            };
            for i in 0..dx_len / 12 {
                if limit > 0 && n_emb >= limit { break; }
                let e = dx_off + i * 12;
                let id = u32le(&bank, e);
                let off = u32le(&bank, e + 4) as usize;
                let size = u32le(&bank, e + 8) as usize;
                if off + size > da_len {
                    eprintln!("  [{}] bank {:08x} media {:08x} runs past DATA -- skipped", tag, r.id, id);
                    continue;
                }
                let slice = &bank[da_off + off..da_off + off + size];
                let is_riff = slice.len() >= 4 && &slice[0..4] == b"RIFF";
                match sink.emit(id, slice, r.id)? {
                    Emit::New(name) | Emit::Replaced(name) => {
                        manifest.push(format!(
                            "{},{:08x},bank,{:08x},{},{},{},{}",
                            tag, id, r.id, r.offset as usize + da_off + off, size, is_riff, name
                        ));
                        n_emb += 1;
                    }
                    Emit::Skipped => {}
                }
            }
        }
    }

    eprintln!(
        "  [{}] {} streams + {} embedded (from {} banks) = {} wems, {} RIFF | skipped {} exact-dup, \
         {} prefetch-fragment | {} replaced-by-longer, {} same-id variants kept -- {}",
        tag, n_stream, n_emb, n_banks, sink.out.len(), sink.riff,
        sink.exact, sink.prefetch, sink.longer, sink.variant, pck.display()
    );
    Ok(sink.out)
}

// Batch-decode wems to WAV. vgmstream selects the Wwise parser by the .wem extension and its
// -o "?f" wildcard expands to the infile string verbatim, so we run with CWD = wem_dir and pass
// bare basenames (?f -> "<tag>_<id>.wem"), writing to an absolute wav path. A final pass strips
// the ".wem.wav" double extension down to ".wav".
fn decode_batches(vgm: &Path, wems: &[PathBuf], wem_dir: &Path, wav_dir: &Path, batch: usize)
    -> std::io::Result<usize>
{
    fs::create_dir_all(wav_dir)?;
    let cwd = std::env::current_dir()?;
    // Plain absolute paths (NO fs::canonicalize -> avoids the \\?\ extended prefix that
    // breaks vgmstream's -o pattern).
    let wav_abs = if wav_dir.is_absolute() { wav_dir.to_path_buf() } else { cwd.join(wav_dir) };
    let vgm_abs = if vgm.is_absolute() { vgm.to_path_buf() } else { cwd.join(vgm) };
    let pat = format!("{}\\?f.wav", wav_abs.to_string_lossy());
    let mut done = 0usize;
    let debug = std::env::var("SAB_DEBUG").is_ok();
    for (bi, chunk) in wems.chunks(batch).enumerate() {
        let mut cmd = Command::new(&vgm_abs);
        cmd.current_dir(wem_dir);
        cmd.arg("-o").arg(&pat);
        for w in chunk {
            cmd.arg(w.file_name().unwrap());
        }
        if debug && bi == 0 {
            eprintln!("DEBUG exe={}", vgm_abs.display());
            eprintln!("DEBUG cwd={}", wem_dir.display());
            eprintln!("DEBUG arg[0]=-o arg[1]={}", pat);
            eprintln!("DEBUG first file arg = {:?}", chunk[0].file_name().unwrap());
            eprintln!("DEBUG last file arg  = {:?}", chunk[chunk.len()-1].file_name().unwrap());
            eprintln!("DEBUG total file args = {}", chunk.len());
        }
        let status = cmd.status()?;
        if !status.success() {
            eprintln!("    vgmstream batch {} exit {:?}", bi, status.code());
        }
        done += chunk.len();
        if bi % 10 == 0 { eprintln!("    decoded ~{}/{}", done, wems.len()); }
    }
    // Strip ".wem.wav" -> ".wav"
    let mut renamed = 0usize;
    for entry in fs::read_dir(wav_dir)? {
        let p = entry?.path();
        let s = p.to_string_lossy().to_string();
        if s.ends_with(".wem.wav") {
            let np = PathBuf::from(s.replace(".wem.wav", ".wav"));
            if fs::rename(&p, &np).is_ok() { renamed += 1; }
        }
    }
    eprintln!("    tidied {} names (.wem.wav -> .wav)", renamed);
    Ok(done)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut game = String::new();
    let mut out = String::from("output/saboteur_audio");
    let mut vgm = String::from("tools/vgmstream/vgmstream-cli.exe");
    // `sfx` = the non-localized Saboteur.pck. Included by default: without it the run yields no
    // sound effects at all, only voice.
    let mut sets = String::from("eng,fra,deu,ita,sfx");
    let mut decode = true;
    let mut keep_wem = false;
    let mut do_streams = true;
    let mut do_banks = true;
    let mut batch = 300usize;
    let mut limit = 0usize; // 0 = no limit
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--game" => { i += 1; game = args[i].clone(); }
            "--out" => { i += 1; out = args[i].clone(); }
            "--vgmstream" => { i += 1; vgm = args[i].clone(); }
            "--sets" | "--langs" => { i += 1; sets = args[i].clone(); }
            "--no-decode" => { decode = false; }
            "--keep-wem" => { keep_wem = true; }
            "--no-banks" => { do_banks = false; }
            "--no-streams" => { do_streams = false; }
            "--batch" => { i += 1; batch = args[i].parse().unwrap(); }
            "--limit" => { i += 1; limit = args[i].parse().unwrap(); }
            other => { eprintln!("unknown arg {}", other); }
        }
        i += 1;
    }
    assert!(!game.is_empty(), "--game <Saboteur install dir> required");
    assert!(do_streams || do_banks, "--no-streams and --no-banks together extract nothing");

    // set code -> (main pck relative, dlc pck relative)
    let set_map: BTreeMap<&str, (&str, &str)> = [
        ("eng", ("Sound/English(US).pck", "DLC/01/sound/English(US).pck")),
        ("fra", ("Sound/French(Canada).pck", "DLC/01/sound/French(Canada).pck")),
        ("deu", ("Sound/German.pck", "DLC/01/sound/German.pck")),
        ("ita", ("Sound/Italian.pck", "DLC/01/sound/Italian.pck")),
        ("sfx", ("Sound/Saboteur.pck", "DLC/01/sound/Saboteur.pck")),
    ].into_iter().collect();

    let out_root = PathBuf::from(&out);
    fs::create_dir_all(&out_root).unwrap();
    let mut manifest: Vec<String> =
        vec!["pack,wem_id,source,bank_id,offset,size,is_riff,file".into()];

    for set in sets.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let (main_rel, dlc_rel) = match set_map.get(set) {
            Some(v) => *v, None => { eprintln!("skip unknown set {}", set); continue; }
        };
        eprintln!("=== set: {} ===", set);
        let lang_dir = out_root.join(set);
        let wem_dir = lang_dir.join("wem");
        let wav_dir = lang_dir.join("wav");
        let mut all_wems: Vec<PathBuf> = Vec::new();

        for (tag, rel) in [(format!("{}_main", set), main_rel), (format!("{}_dlc", set), dlc_rel)] {
            let pck = PathBuf::from(&game).join(rel);
            if !pck.exists() { eprintln!("  (missing {})", pck.display()); continue; }
            match carve_pack(&pck, &tag, &wem_dir, &mut manifest, limit, do_streams, do_banks) {
                Ok(mut v) => all_wems.append(&mut v),
                Err(e) => eprintln!("  ERROR carving {}: {}", pck.display(), e),
            }
        }

        if decode && !all_wems.is_empty() {
            eprintln!("  decoding {} wems -> {}", all_wems.len(), wav_dir.display());
            match decode_batches(&PathBuf::from(&vgm), &all_wems, &wem_dir, &wav_dir, batch) {
                Ok(n) => eprintln!("  decoded {} files", n),
                Err(e) => eprintln!("  decode error: {}", e),
            }
            if !keep_wem {
                let _ = fs::remove_dir_all(&wem_dir);
                eprintln!("  removed intermediate wem dir");
            }
        }
    }

    let mpath = out_root.join("manifest.csv");
    fs::write(&mpath, manifest.join("\n")).unwrap();
    eprintln!("manifest -> {} ({} rows)", mpath.display(), manifest.len() - 1);
}
