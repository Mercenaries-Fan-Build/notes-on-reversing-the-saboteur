//! `proto2008-binaries <bin360>` — which container and asset magics each 2008 prototype binary loads.
//! Every `.xex` is unwrapped with `sab_formats::xex`; every `.exe` is read as is. In each PowerPC image
//! the probe collects the 32-bit constants built by `lis rD,hi` followed (within 16 instructions) by
//! `ori rX,rD,lo` or `addi rX,rD,lo`, keeps those that are four printable characters with a high half
//! from the asset-magic families below, and prints them with the binary's original PE name and PE
//! link timestamp. `docs/2008_subpack_format.md` cites the result.

use std::collections::BTreeMap;
use std::path::PathBuf;

use sab_formats::{bytes, xex};

/// High halves of the magics the probe looks for: `SB`, `MS`, `MP`, `AP`, `AN`, `WS`, `MT`.
const FAMILIES: [u16; 7] = [0x5342, 0x4D53, 0x4D50, 0x4150, 0x414E, 0x5753, 0x4D54];

fn fourccs(img: &[u8]) -> BTreeMap<u32, usize> {
    let w = |i: usize| u32::from_be_bytes(img[i..i + 4].try_into().expect("4 bytes"));
    let mut found = BTreeMap::new();
    let mut i = 0;
    while i + 4 <= img.len() {
        let ins = w(i);
        let (op, rd, ra, hi) = (ins >> 26, (ins >> 21) & 31, (ins >> 16) & 31, ins & 0xFFFF);
        let near = FAMILIES.iter().any(|f| *f as u32 == hi || (*f as u32 + 1) == hi);
        if op == 15 && ra == 0 && near {
            for k in 1..16 {
                let j = i + 4 * k;
                if j + 4 > img.len() {
                    break;
                }
                let n = w(j);
                let (op2, rs, ra2, lo) = (n >> 26, (n >> 21) & 31, (n >> 16) & 31, n & 0xFFFF);
                let v = match op2 {
                    24 if rs == rd => Some((hi << 16) | lo),
                    14 if ra2 == rd => Some((hi << 16).wrapping_add(lo as u16 as i16 as i32 as u32)),
                    _ => None,
                };
                if let Some(v) = v {
                    let printable = v.to_be_bytes().iter().all(|c| c.is_ascii_alphanumeric());
                    if printable && FAMILIES.contains(&((v >> 16) as u16)) {
                        *found.entry(v).or_default() += 1;
                    }
                    break;
                }
            }
        }
        i += 4;
    }
    found
}

fn pe_timestamp(img: &[u8]) -> Result<u32, String> {
    let pe = bytes::u32_le(img, 0x3C, "e_lfanew")? as usize;
    bytes::u32_le(img, pe + 8, "PE TimeDateStamp")
}

pub fn run(args: &[String]) {
    let [bin360] = args else {
        eprintln!("usage: sab_probe proto2008-binaries <Bin360 dir>");
        std::process::exit(2);
    };
    let root = PathBuf::from(bin360);
    let mut names: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| matches!(p.extension().and_then(|x| x.to_str()), Some("xex") | Some("exe")))
        .collect();
    names.sort();
    for p in &names {
        let file = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let buf = std::fs::read(p).unwrap_or_else(|e| panic!("{file}: {e}"));
        let (img, original) = if buf.starts_with(xex::MAGIC) {
            let h = xex::parse(&buf).unwrap_or_else(|e| panic!("{file}: {e}"));
            let name = xex::original_pe_name(&buf, &h).unwrap_or_else(|e| panic!("{file}: {e}"));
            let img = xex::unwrap(&buf, &h, &xex::DEVKIT_KEK).unwrap_or_else(|e| panic!("{file}: {e}"));
            (img, name.unwrap_or_else(|| "-".into()))
        } else if buf.starts_with(b"MZ") {
            (buf, "-".into())
        } else {
            println!("{file}: not an XEX or PE image, head {:02X?}", &buf[..buf.len().min(4)]);
            continue;
        };
        let ts = pe_timestamp(&img).unwrap_or_else(|e| panic!("{file}: {e}"));
        let found = fourccs(&img);
        let list: Vec<String> = found
            .iter()
            .map(|(v, n)| format!("{}x{n}", String::from_utf8_lossy(&v.to_be_bytes())))
            .collect();
        println!("{file:<24} original {original:<26} PE timestamp 0x{ts:08X}  {}", list.join(" "));
    }
}
