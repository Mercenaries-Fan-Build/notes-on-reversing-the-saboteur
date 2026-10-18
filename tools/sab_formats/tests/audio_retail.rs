//! Game gate for retail audio decoding over the install named by `retail_dir` in the repo-root
//! `.sab-local.toml`: every 25th streamed `.wem` of every `Sound/*.pck` decodes with
//! `wwise::decode_wem`, to exactly the sample count its `vorb` chunk declares.

use std::path::{Path, PathBuf};

use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::wwise::{decode_wem, SoundPack};

fn retail() -> PathBuf {
    local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
}

/// The `vorb` chunk's first word: the stream's sample count per channel.
fn vorb_samples(wem: &[u8]) -> Result<u32, String> {
    if wem.len() < 12 || &wem[0..4] != b"RIFF" || &wem[8..12] != b"WAVE" {
        return Err("not a RIFF WAVE".into());
    }
    let mut o = 12;
    while o + 8 <= wem.len() {
        let size = u32::from_le_bytes(wem[o + 4..o + 8].try_into().expect("4 bytes")) as usize;
        if &wem[o..o + 4] == b"vorb" {
            return Ok(u32::from_le_bytes(wem[o + 8..o + 12].try_into().expect("4 bytes")));
        }
        o += 8 + size + (size & 1);
    }
    Err("no vorb chunk".into())
}

#[test]
fn retail_wem_streams_decode_to_their_declared_length() {
    let sound = retail().join("Sound");
    let mut packs: Vec<PathBuf> = std::fs::read_dir(&sound)
        .unwrap_or_else(|e| panic!("{}: {e}", sound.display()))
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")))
        .collect();
    packs.sort();
    assert!(!packs.is_empty(), "no .pck under {}", sound.display());
    let (mut decoded, mut frames) = (0usize, 0usize);
    let mut failures = Vec::new();
    for pck in &packs {
        let pack = SoundPack::open(pck).unwrap_or_else(|e| panic!("{}: {e}", pck.display()));
        for id in pack.stream_ids().into_iter().step_by(25) {
            let at = format!("{} wem {id:08X}", pck.display());
            let wem = pack.wem_bytes(id).unwrap_or_else(|| panic!("{at}: unreadable"));
            let want = match vorb_samples(&wem) {
                Ok(n) => n as usize,
                Err(e) => {
                    failures.push(format!("{at}: {e}"));
                    continue;
                }
            };
            match decode_wem(&wem) {
                Ok(p) if p.frames() == want && (1..=2).contains(&p.channels) => {
                    decoded += 1;
                    frames += p.frames();
                }
                Ok(p) => failures.push(format!("{at}: {} frames x {} channels, vorb says {want}", p.frames(), p.channels)),
                Err(e) => failures.push(format!("{at}: {e}")),
            }
        }
    }
    eprintln!("{} packs: {decoded} streams decoded, {frames} frames", packs.len());
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(decoded > 0);
}
