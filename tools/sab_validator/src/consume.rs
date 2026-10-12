//! Asset consumers — each mirrors what the engine's loader for that layer actually reads, and
//! turns any deviation into an [`Issue`]. The walk follows the engine mount path:
//! `.megapack` (mounter `FUN_00e428c0`) → `ALBS` sub-pack (loader `FUN_00658870`) → sub-assets.
//!
//! An ALBS sub-pack can wrap several resource kinds (mesh/texture object directory, particle
//! `CFX`, empty stub, named config, terrain, …). The mesh/texture *directory* model is only one
//! of them, so — like the real extractors — we don't gate on it. Directory integrity is a soft
//! insight; the load-bearing validation is content-scanning for DTEX textures and MSHA meshes and
//! checking each the way the engine would consume it.

use crate::report::{Issue, Report, Severity};
use sab_formats::{dtex, megapack, mesh, sbla};

/// Walk a whole `.megapack`: validate the index, then descend into every sub-pack.
pub fn consume_megapack(buf: &[u8], name: &str, report: &mut Report, limit: Option<usize>) {
    let mp = match megapack::parse(buf) {
        Ok(mp) => mp,
        Err(e) => {
            report.failed("megapack");
            report.push(Issue {
                severity: Severity::Fatal,
                code: "megapack.bad-header",
                format: "megapack",
                location: name.to_string(),
                message: format!("{e} — the mounter FUN_00e428c0 rejects the archive"),
                engine_ref: Some("FUN_00e428c0 @0x00e428c0"),
            });
            return;
        }
    };
    report.scanned("megapack");

    let file_end = buf.len() as u64;
    let n = limit.map(|l| l.min(mp.entries.len())).unwrap_or(mp.entries.len());
    for (i, e) in mp.entries.iter().take(n).enumerate() {
        let loc = format!("{name}[{i}] crc=0x{:08X}", e.crc);

        // --- entry-level integrity (what the mounter needs before it can seek) ---
        let end = e.offset.saturating_add(e.size as u64);
        if end > file_end {
            report.push(Issue {
                severity: Severity::Fatal,
                code: "megapack.entry-out-of-range",
                format: "megapack",
                location: loc.clone(),
                message: format!(
                    "sub-pack [0x{:X}..0x{:X}] runs past EOF 0x{file_end:X} — the mount seek/read faults",
                    e.offset, end
                ),
                engine_ref: Some("FUN_00e428c0 @0x00e428c0"),
            });
            continue;
        }
        if e.offset % megapack::SECTOR != 0 {
            report.push(Issue {
                severity: Severity::Advisory,
                code: "megapack.unaligned-offset",
                format: "megapack",
                location: loc.clone(),
                message: format!(
                    "offset 0x{:X} is not {}-aligned (retail always is)",
                    e.offset,
                    megapack::SECTOR
                ),
                engine_ref: None,
            });
        }
        if e.size == 0 {
            report.push(Issue {
                severity: Severity::Warning,
                code: "megapack.empty-entry",
                format: "megapack",
                location: loc.clone(),
                message: "entry declares zero-length sub-pack".into(),
                engine_ref: None,
            });
            continue;
        }

        match megapack::entry_slice(buf, e) {
            Ok(sub) => consume_sbla(sub, &loc, report),
            Err(err) => report.push(Issue {
                severity: Severity::Fatal,
                code: "megapack.entry-unreadable",
                format: "megapack",
                location: loc,
                message: err,
                engine_ref: Some("FUN_00e428c0 @0x00e428c0"),
            }),
        }
    }
}

/// Validate one `ALBS` sub-pack: soft directory-integrity insight, then the contained sub-assets.
pub fn consume_sbla(buf: &[u8], parent: &str, report: &mut Report) {
    if buf.get(0..4) != Some(sbla::MAGIC) {
        report.push(Issue {
            severity: Severity::Advisory,
            code: "sbla.not-albs",
            format: "sbla",
            location: parent.to_string(),
            message: format!(
                "sub-pack magic {:02X?} is not \"ALBS\" — non-descended resource kind",
                buf.get(0..4).unwrap_or(&[])
            ),
            engine_ref: None,
        });
        return;
    }
    report.scanned("sbla");
    let loc = format!("{parent} / ALBS");

    // Soft directory check: only when the mesh/texture *object* directory model cleanly applies do
    // we assert its blob ranges. A parse refusal (empty/CFX/stub/other variant) is NOT an error —
    // those are legitimate retail sub-pack kinds the directory model simply doesn't cover.
    if let Ok(s) = sbla::parse(buf) {
        for (r, (start, end)) in s.recs.iter().zip(sbla::blob_ranges(&s).iter()) {
            if r.real() && *end > buf.len() {
                report.push(Issue {
                    severity: Severity::Fatal,
                    code: "sbla.blob-out-of-range",
                    format: "sbla",
                    location: format!("{loc} rec 0x{:08X}", r.hash),
                    message: format!(
                        "directory blob [0x{start:X}..0x{end:X}] exceeds sub-pack size 0x{:X} — the loader over-reads",
                        buf.len()
                    ),
                    engine_ref: Some("FUN_00658870 @0x00658870"),
                });
            }
        }
    }

    // Load-bearing validation: content-scan for the assets the engine actually consumes.
    for m in mesh::scan_msha(buf) {
        consume_mesh(buf, &m, &loc, report);
    }
    for start in dtex::find_record_starts(buf) {
        consume_dtex(&buf[start..], &loc, report);
    }
}

/// Validate a MSHA-wrapped MESH: the two blobs must inflate to their declared sizes (the engine
/// trusts `unc0`/`unc1` when it allocates the body and VB/IB buffers).
fn consume_mesh(buf: &[u8], m: &mesh::Msha, parent: &str, report: &mut Report) {
    report.scanned("mesh");
    let body = match mesh::read_body_and_dat(buf, m) {
        Ok((body, _dat)) => body,
        Err(e) => {
            report.failed("mesh");
            report.push(Issue {
                severity: Severity::Fatal,
                code: "mesh.blob-inflate-mismatch",
                format: "mesh",
                location: format!("{parent} / MSHA '{}'", m.name),
                message: format!("{e} — the engine's mesh loader allocates the declared size and faults"),
                engine_ref: Some("FUN_00658870 @0x00658870"),
            });
            return;
        }
    };
    check_prim_tricount(&body, m, parent, report);
}

/// Every MESH primitive stores BOTH its index count (@96) and its triangle count / draw primCount
/// (@92). The engine builds one render segment per prim and uses @92 as the DrawIndexedPrimitive
/// primCount; a value inconsistent with the index count makes the loader reject the prim, so the
/// OdinMesh ends up with 0 LOD segments, `OdinMesh::IsFullyLoaded` returns false, and the engine
/// SKIPS the draw entirely (the part renders as a hole / patchy "dither"). Synth encoders that
/// template a prim from a donor and forget to recompute @92 hit this. Every retail prim has
/// `@92 == numIndices/3`.
fn check_prim_tricount(body: &[u8], m: &mesh::Msha, parent: &str, report: &mut Report) {
    let u32at = |o: usize| -> Option<u32> {
        body.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let u16at = |o: usize| -> Option<u16> { body.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])) };
    // header counts / skeleton walk (offsets into the decompressed MESH body)
    let (Some(n_remaps), Some(n_streams), Some(n_prims), Some(n_unk0), Some(n_bones), Some(n_unk1)) =
        (u32at(208), u16at(216), u16at(218), u32at(244), u32at(256), u32at(260))
    else {
        return; // too short to be a skinned MESH; other checks handle malformed bodies
    };
    let (n_remaps, n_streams, n_prims, n_unk0, n_bones) =
        (n_remaps as usize, n_streams as usize, n_prims as usize, n_unk0 as usize, n_bones as usize);
    if n_bones <= 1 || n_prims == 0 {
        return;
    }
    let mut p = 288 + n_bones + n_unk0 + n_bones * 64 + n_bones * 64 + n_bones * 48 + n_bones * 2 + n_bones * 4;
    if n_unk1 != 0 {
        p += 2;
    }
    if n_remaps > 0 {
        p += 8 + n_remaps * 68;
    }
    p += n_streams * 152;
    for i in 0..n_prims {
        let po = p + i * 100;
        let (Some(tri_count), Some(num_idx)) = (u32at(po + 92), u32at(po + 96)) else {
            return; // prim table overruns body — malformed; not this check's concern
        };
        if tri_count != num_idx / 3 {
            report.failed("mesh");
            report.push(Issue {
                severity: Severity::Fatal,
                code: "mesh.prim-tricount-mismatch",
                format: "mesh",
                location: format!("{parent} / MSHA '{}' prim[{i}]", m.name),
                message: format!(
                    "prim triangle count @92={tri_count} != numIndices/3 ({num_idx}/3={}); the loader \
                     rejects the prim -> 0 LOD segments -> OdinMesh::IsFullyLoaded false -> the mesh is \
                     not drawn (renders as a hole)",
                    num_idx / 3
                ),
                engine_ref: Some("OdinMesh::SetupLodSegmentCount / IsFullyLoaded @0x00e0c460"),
            });
        }
    }
}

/// Validate a DTEX texture record: parse, inflate every stream, confirm the payload matches the
/// declared uncompressed size, and confirm the mip descriptor chain tiles the payload exactly.
fn consume_dtex(rec: &[u8], parent: &str, report: &mut Report) {
    report.scanned("dtex");
    let d = match dtex::parse(rec) {
        Ok(d) => d,
        Err(e) => {
            report.failed("dtex");
            report.push(Issue {
                severity: Severity::Fatal,
                code: "dtex.malformed-record",
                format: "dtex",
                location: parent.to_string(),
                message: format!("{e} — the texture loader FUN_009bb910 cannot read the record"),
                engine_ref: Some("FUN_009bb910 @0x009bb910"),
            });
            return;
        }
    };
    let loc = format!("{parent} / DTEX '{}'", d.name);

    let payload = match dtex::payload(&d) {
        Ok(p) => p,
        Err(e) => {
            report.failed("dtex");
            report.push(Issue {
                severity: Severity::Fatal,
                code: "dtex.stream-inflate-failed",
                format: "dtex",
                location: loc,
                message: format!("{e} — a mip stream does not decompress"),
                engine_ref: None,
            });
            return;
        }
    };
    if payload.len() != d.unc as usize {
        report.failed("dtex");
        report.push(Issue {
            severity: Severity::Fatal,
            code: "dtex.payload-size-mismatch",
            format: "dtex",
            location: loc.clone(),
            message: format!(
                "inflated mip payload is {} bytes but the header declares uncompressedSize {} — the engine's mip buffer under/over-runs",
                payload.len(),
                d.unc
            ),
            engine_ref: Some("FUN_009bb910 @0x009bb910"),
        });
        return;
    }

    // Mip descriptor chain: must yield exactly mipCount descriptors that tile the whole payload.
    let mips = dtex::mips_of(&d, &payload);
    if mips.len() != d.mips as usize {
        report.failed("dtex");
        report.push(Issue {
            severity: Severity::Fatal,
            code: "dtex.mip-chain-truncated",
            format: "dtex",
            location: loc.clone(),
            message: format!(
                "walked {} mip descriptors but header declares mipCount={} — a descriptor or its data overruns the payload",
                mips.len(),
                d.mips
            ),
            engine_ref: Some("FUN_009bb910 @0x009bb910"),
        });
        return;
    }
    // Each mip's declared data size should match the format+dimensions.
    for mp in &mips {
        if let Some(expect) = dtex::mip_bytes(d.format, mp.width as usize, mp.height as usize) {
            if mp.data_size != expect {
                report.push(Issue {
                    severity: Severity::Warning,
                    code: "dtex.mip-size-inconsistent",
                    format: "dtex",
                    location: loc.clone(),
                    message: format!(
                        "mip {} ({}x{} {}) stores {} bytes but the format implies {expect}",
                        mp.index,
                        mp.width,
                        mp.height,
                        dtex::fmt_name(d.format),
                        mp.data_size
                    ),
                    engine_ref: None,
                });
            }
        }
    }
}
