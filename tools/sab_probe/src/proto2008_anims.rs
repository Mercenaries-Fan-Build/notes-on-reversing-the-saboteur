//! `proto2008-anims <bin360>` — read every `ANM0` clip under the 2008 prototype's `Bin360` (by magic)
//! and `Animations.pack` with `sab_formats::proto2008::{anim, anim_pack}`, decode every clip, and
//! tally the facts `docs/2008_animations_format.md` cites: clips per directory and codec, the clips the
//! loose loader rejects, header value sets and ranges, the frame-count relation, rotation norms of the
//! stored poses, pack record flags and twins, and the pack's sections.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use sab_formats::pandemic_hash;
use sab_formats::proto2008::anim::{self, Body, Clip, Codec};
use sab_formats::proto2008::anim_pack::{self, TRANSITION_TYPES};

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
struct Range {
    min: f64,
    max: f64,
    n: usize,
}

impl Range {
    fn add(&mut self, v: f64) {
        if self.n == 0 || v < self.min {
            self.min = v;
        }
        if self.n == 0 || v > self.max {
            self.max = v;
        }
        self.n += 1;
    }
}

#[derive(Default)]
struct Stats {
    sets: BTreeMap<String, BTreeMap<String, usize>>,
    ranges: BTreeMap<String, Range>,
    /// codec -> (poses read, transforms, max ||q| - 1|, transforms with ||q| - 1| > 1e-2)
    norms: BTreeMap<&'static str, (usize, usize, f32, usize)>,
}

impl Stats {
    fn set(&mut self, key: &str, value: impl ToString) {
        *self.sets.entry(key.to_string()).or_default().entry(value.to_string()).or_default() += 1;
    }

    fn range(&mut self, key: &str, v: f64) {
        self.ranges.entry(key.to_string()).or_default().add(v);
    }

    fn clip(&mut self, from: &str, clip: &Clip) {
        let codec = clip.codec().name();
        self.set(&format!("{from} codec"), codec);
        self.set(&format!("{from} tracks == bone CRCs"), clip.tracks as usize == clip.bones.len());
        self.range(&format!("{from} {codec} duration"), clip.duration as f64);
        self.range(&format!("{from} {codec} tracks"), clip.tracks as f64);
        let (poses, extra) = match &clip.body {
            Body::Wavelet(w) => {
                self.set(&format!("{from} WAV0 (blockSize, maxBitWidth, preserved)"), format!("({}, {}, {})", w.block_size, w.quantization.max_bit_width, w.quantization.preserved));
                self.set(&format!("{from} WAV0 blockIndexSize == ceil(poses / blockSize)"), w.block_index_size as i32 == (w.poses + w.block_size - 1) / w.block_size);
                self.set(&format!("{from} WAV0 (maxBitWidth, numD == 0)"), format!("({}, {})", w.quantization.max_bit_width, w.quantization.num_d == 0));
                (w.poses as f64, w.data.len())
            }
            Body::Delta(d) => {
                self.set(&format!("{from} DEL0 (blockSize, maxBitWidth, preserved)"), format!("({}, {}, {})", d.block_size, d.quantization.max_bit_width, d.quantization.preserved));
                self.set(&format!("{from} DEL0 staticMaskSize"), d.static_mask_size);
                self.set(&format!("{from} DEL0 (maxBitWidth, numD == 0)"), format!("({}, {})", d.quantization.max_bit_width, d.quantization.num_d == 0));
                (d.poses as f64, d.data.len())
            }
            Body::Uncompressed(t) => {
                self.set(&format!("{from} NOC0 numTransforms % tracks == 0"), clip.tracks > 0 && t.len() % clip.tracks as usize == 0);
                ((t.len() / clip.tracks.max(1) as usize) as f64, t.len() * 0x30)
            }
        };
        self.range(&format!("{from} {codec} poses"), poses);
        self.range(&format!("{from} {codec} data bytes"), extra as f64);
        self.set(&format!("{from} poses == round(duration * 30) + 1"), poses == (clip.duration as f64 * 30.0).round() + 1.0);
    }

    fn poses(&mut self, codec: Codec, a: &anim::Animation) {
        let e = self.norms.entry(codec.name()).or_default();
        for f in 0..a.poses() {
            e.0 += 1;
            for x in a.pose(f).expect("pose") {
                let d = (x.rotation.iter().map(|v| v * v).sum::<f32>().sqrt() - 1.0).abs();
                e.1 += 1;
                e.2 = e.2.max(d);
                e.3 += (d > 1e-2) as usize;
            }
        }
    }
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-anims <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    let mut s = Stats::default();
    let mut rejected = Vec::new();
    let mut by_key: HashMap<u32, Vec<(String, Vec<u8>)>> = HashMap::new();
    for p in &all {
        let b = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        if b.get(..4) != Some(anim::MAGIC) {
            continue;
        }
        let rel = p.strip_prefix(&root).expect("under root").display().to_string();
        let clip = anim::parse(&b).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(clip.len, b.len(), "{rel}: clip length");
        let dir = rel.split(['/', '\\']).next().expect("first component").to_string();
        s.set("loose clips by directory and codec", format!("{dir} {}", clip.codec().name()));
        let stem = p.file_stem().expect("stem").to_string_lossy().into_owned();
        by_key.entry(pandemic_hash(&stem)).or_default().push((rel.clone(), b.clone()));
        if let Err(e) = clip.check_loose() {
            rejected.push(format!("{rel} ({}, duration {}, {} transforms): {e}", clip.codec().name(), clip.duration, match &clip.body {
                Body::Uncompressed(t) => t.len(),
                _ => 0,
            }));
            continue;
        }
        s.clip("loose", &clip);
        let a = clip.decode().unwrap_or_else(|e| panic!("{rel}: {e}"));
        s.poses(clip.codec(), &a);
    }
    let shared: Vec<_> = by_key.iter().filter(|(_, v)| v.len() > 1).map(|(k, v)| format!("{k:08X}: {:?}", v.iter().map(|x| &x.0).collect::<Vec<_>>())).collect();

    let pack_path = root.join("Animations.pack");
    let buf = std::fs::read(&pack_path).unwrap_or_else(|e| panic!("{}: {e}", pack_path.display()));
    let pack = anim_pack::parse(&buf).unwrap_or_else(|e| panic!("{}: {e}", pack_path.display()));
    let mut differs = Vec::new();
    for r in &pack.records {
        s.clip("pack", &r.clip);
        s.set("pack object flag", r.object);
        s.set("pack turn positive", r.turn_positive);
        let body = &buf[r.clip_at..r.clip_at + r.clip.len];
        let twin = match by_key.get(&r.key) {
            None => "no loose clip of the key",
            Some(v) if v.iter().any(|(_, b)| b == body) => "byte-identical to a loose clip",
            Some(v) => {
                differs.push(format!("{:08X} ({} bytes) vs {:?}", r.key, body.len(), v.iter().map(|(p, b)| format!("{p} ({} bytes)", b.len())).collect::<Vec<_>>()));
                "differs from the loose clip"
            }
        };
        s.set("pack clip vs loose clip of the same key", twin);
        for e in &r.events {
            s.set("pack event frame >= 0", e.frame >= 0);
        }
        for _ in &r.offsets {
            s.set("pack offsets", "count");
        }
    }
    let keys: BTreeSet<u32> = pack.records.iter().map(|r| r.key).collect();
    let sequence_names: BTreeSet<u32> = pack.sequences.iter().map(|q| q.name).collect();
    for q in &pack.sequences {
        s.set("SEQC sequence type", q.body.kind);
        for i in &q.instructions {
            s.set("SEQC instruction word 0", i.word0);
            for a in &i.animations {
                s.set("SEQC instruction animation is a record key", keys.contains(a));
            }
        }
    }
    for c in &pack.transitions {
        for t in &c.transitions {
            let name = |v: u32| TRANSITION_TYPES.get(v as usize).map(|n| n.to_string()).unwrap_or(v.to_string());
            s.set("TRAN type", name(t.kind));
            s.set("TRAN inline sequence", t.inline.is_some());
            if let Some(i) = &t.inline {
                s.set("TRAN inline type", name(t.inline_type));
                s.set("TRAN inline animation is a record key", keys.contains(&i.animation));
            }
            s.set("TRAN label ends in NUL", t.label.last() == Some(&0));
        }
    }
    for b in &pack.banks {
        let text = String::from_utf8_lossy(b.text.split(|&c| c == 0).next().expect("split yields one part"));
        s.set("BANK name CRC == pandemic_hash(text)", pandemic_hash(&text) == b.name);
        for (first, second) in &b.overrides {
            s.set("BANK override (first, second) are record keys", format!("({}, {})", keys.contains(first), keys.contains(second)));
        }
    }
    for k in &pack.list_88 {
        s.set("ALPH list +0x88 entry is (sequence name, record key)", format!("({}, {})", sequence_names.contains(k), keys.contains(k)));
    }
    let list_7c: BTreeSet<u32> = pack.list_7c.iter().copied().collect();
    s.set("ALPH list +0x7C: (distinct entries, equals the record key set, in record order)", format!("({}, {}, {})", list_7c.len(), list_7c == keys, pack.list_7c == pack.records.iter().map(|r| r.key).collect::<Vec<_>>()));

    println!("Animations.pack: {} bytes read of {}", pack.len, buf.len());
    println!(
        "pack: {} records, blend bone exception {}, {} intervals, {} sequences, {} transition containers ({} transitions), {} banks, {} additive pairs, ALPH {} + {}",
        pack.records.len(),
        pack.blend_bone_exception,
        pack.intervals.len(),
        pack.sequences.len(),
        pack.transitions.len(),
        pack.transitions.iter().map(|c| c.transitions.len()).sum::<usize>(),
        pack.banks.len(),
        pack.additives.len(),
        pack.list_88.len(),
        pack.list_7c.len()
    );
    for i in &pack.intervals {
        println!("  INTV type {:08X} values {:?} animations {}", i.kind, i.values, i.animations.len());
    }
    println!("loose clips LoadAnimFile rejects: {}", rejected.len());
    for r in &rejected {
        println!("  {r}");
    }
    println!("loose stems sharing a key: {}", shared.len());
    for r in &shared {
        println!("  {r}");
    }
    println!("pack clips that differ from their loose clip: {}", differs.len());
    for r in &differs {
        println!("  {r}");
    }
    for (k, v) in &s.sets {
        println!("{k}: {v:?}");
    }
    for (k, r) in &s.ranges {
        println!("{k}: {} .. {} ({})", r.min, r.max, r.n);
    }
    for (codec, (poses, transforms, max, over)) in &s.norms {
        println!("{codec} stored poses: {poses} poses, {transforms} transforms, max ||q| - 1| {max:.4}, {over} over 1e-2");
    }
}
