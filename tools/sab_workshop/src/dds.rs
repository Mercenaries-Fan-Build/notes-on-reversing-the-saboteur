//! Standard PC-style `DDS ` texture files. Only the 2008 Xbox 360 prototype ships these as
//! loose assets under `Bin360/` — retail packs everything into DTEX records with no DDS wrapper
//! at all — but the header layout is the D3D9 documented one, byte-for-byte.
//!
//! **Empirically measured** (see `docs/2008_texture_format.md`) the on-disk layout is
//! plain-linear, little-endian DXT blocks — NOT the Xbox-tiled 128×128-macrotile-with-word-swap
//! that a native GPU texture would use. So we can hand mip 0 straight to
//! `dtex::decode_surface`, which already covers BC1/BC2/BC3 and the uncompressed formats we
//! care about. No un-tiling step is needed; if a future prototype dumps its GPU-resident texture
//! memory (rather than the authoring cache we see here) that will be a separate reader.
//!
//! DXT4 (pre-multiplied alpha DXT5) turns up in a handful of these loose textures. The block
//! layout is byte-identical to DXT5; the difference is a semantic one about pre-multiplication
//! that only matters at composite time. We normalise it to DXT5 so the shared decoder can eat
//! it — the alpha channel comes back as-authored either way.
//!
//! Kept intentionally tiny — one entry point (`decode_bytes`) that returns the same
//! `dtex::CpuTexture` the rest of the workshop consumes. Face 0, mip 0 of a cubemap is
//! sufficient for a viewer's first-look (the browser doesn't yet render cubemaps as such).

use crate::dtex::{self, CpuTexture};

// D3DFMT ints — same encoding `dtex.rs` uses so we can call straight into `decode_surface`.
const DXT1: u32 = 0x3154_5844;
const DXT2: u32 = 0x3254_5844; // BC2, pre-multiplied alpha semantic
const DXT3: u32 = 0x3354_5844; // BC2
const DXT4: u32 = 0x3454_5844; // BC3, pre-multiplied alpha semantic
const DXT5: u32 = 0x3554_5844; // BC3

/// DDS header + first mip decode → straight-alpha RGBA8, ready to upload as `Rgba8Unorm`.
///
/// `name` is what the caller prefers as a friendly label; the DDS format has no name field so
/// we take one rather than fabricating from an offset.
pub fn decode_bytes(name: &str, buf: &[u8]) -> Result<CpuTexture, String> {
    if buf.len() < 128 || &buf[0..4] != b"DDS " {
        return Err("not a DDS file (missing `DDS ` magic)".into());
    }
    // Standard D3D9 DDS_HEADER, laid out little-endian on disk. We only need the fields the
    // block decoders can't infer from block-count math.
    let header_size = u32_at(buf, 4);
    if header_size != 124 {
        return Err(format!("bad DDS header size: {header_size} (want 124)"));
    }
    let height = u32_at(buf, 12);
    let width = u32_at(buf, 16);
    // DDS_PIXELFORMAT starts at +76 (0x4C); its `flags` (+80) and `fourCC` (+84) drive format
    // selection. We only handle the fourCC-coded compressed formats today — the loose Bin360
    // library is all DXT (n=615, 100% coverage across DXT1/3/4/5). Uncompressed DDS variants
    // would need the RGB masks at +92 to be parsed; not required yet.
    let pf_flags = u32_at(buf, 80);
    if pf_flags & 0x04 == 0 {
        return Err("uncompressed DDS pixel formats aren't supported yet — no such loose \
                    files exist in the current 2008 dump"
            .into());
    }
    let fourcc = u32_at(buf, 84);
    // DXT2/DXT4 are the pre-multiplied-alpha twins of DXT3/DXT5; the on-disk block layout is
    // identical (BC2/BC3). We normalise up front so `dtex::decode_surface`'s match doesn't
    // have to know about the pre-mult variant. Straight-alpha RGBA8 is what the viewer wants
    // regardless — the pre-mult designation only matters at composite time, which we don't do.
    let fmt = match fourcc {
        DXT2 => DXT3,
        DXT4 => DXT5,
        other => other,
    };

    // Just mip 0, face 0 — the browser thumbnail path never asks for more, and cubemaps here
    // are 128×128 max (the only 9 cubemaps in Bin360 are the diffuse probes), so decoding a
    // single face is plenty for a preview. Higher-mip picking follows the same shape as
    // `dtex::decode_preview` when we need it — this reader stays deliberately minimal.
    let pixel_data_start = 128usize;
    let block_bytes = match fmt {
        DXT1 => 8usize,
        DXT3 | DXT5 => 16usize,
        _ => {
            return Err(format!(
                "unhandled DDS fourCC 0x{fourcc:08X} — only DXT1/2/3/4/5 wired today"
            ))
        }
    };
    let bw = (width as usize).div_ceil(4);
    let bh = (height as usize).div_ceil(4);
    let mip0_len = bw * bh * block_bytes;
    if buf.len() < pixel_data_start + mip0_len {
        return Err(format!(
            "DDS payload truncated: need {mip0_len} bytes for mip 0 at {}x{}, have {}",
            width,
            height,
            buf.len().saturating_sub(pixel_data_start)
        ));
    }
    let data = &buf[pixel_data_start..pixel_data_start + mip0_len];
    let rgba = dtex::decode_surface(fmt, width, height, data)?;

    Ok(CpuTexture { name: name.to_string(), width, height, format: fmt, rgba })
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one that gave us the "linear, not tiled" verdict during probing: 512×512 DXT1 cloud
    /// texture. If the decoder ever regresses to tiled interpretation, the pixel-count math is
    /// unchanged (blocks in vs pixels out) but the image is garbage — the human eyeball probe
    /// is documented in `docs/2008_texture_format.md`, this test asserts the mechanical shape.
    #[test]
    fn decodes_cloud_proj_512x512_dxt1() {
        let Some(root) = crate::testenv::prototype2008_bin360() else {
            eprintln!("skip: SAB_2008_DIR not set to a 2008 prototype");
            return;
        };
        let path = root.join("CloudProj.dds");
        if !path.exists() {
            eprintln!("skip: no CloudProj.dds at expected 2008 dump");
            return;
        }
        let bytes = std::fs::read(&path).expect("read");
        let tex = decode_bytes("CloudProj", &bytes).expect("decode");
        assert_eq!(tex.width, 512);
        assert_eq!(tex.height, 512);
        assert_eq!(tex.rgba.len(), 512 * 512 * 4);
        // A DXT1 cloud texture is mostly white-with-holes — the mean R channel should be well
        // above 128. This same assertion would fail with the wrong endianness / a tiled
        // interpretation (mean drifts either way; see the probe output in the docs).
        let mean_r: u64 =
            tex.rgba.chunks_exact(4).map(|p| p[0] as u64).sum::<u64>() / (512 * 512);
        assert!(
            mean_r > 150,
            "mean R = {mean_r}; cloud texture decoded wrong (tiled? swapped?)"
        );
    }

    #[test]
    fn rejects_non_dds() {
        assert!(decode_bytes("junk", b"not a dds file").is_err());
        assert!(decode_bytes("empty", &[]).is_err());
    }
}
