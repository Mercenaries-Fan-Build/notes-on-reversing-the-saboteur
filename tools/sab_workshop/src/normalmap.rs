//! Normal-map helpers — grayscale-relief detection + height→tangent-normal conversion.
//!
//! Ported from `mercs2_workshop`'s `texpng.rs` (`mean_blue` and `height_to_normal`). The
//! Mercenaries 2 team discovered that its `_nm` slot is NOT a tangent-space normal map — it is a
//! grayscale height/relief with R==G==B, so binding it directly gives near-sideways normals (Z≈0)
//! and mottled shading. `mean_blue < 200` is the tell, and a central-difference gradient recovers a
//! proper OpenGL-convention tangent normal (R=X, G=+Y up, B=Z out).
//!
//! Saboteur may or may not ship the same convention. Applying the gate is cheap and only triggers
//! when the source really is grayscale — a genuine tangent normal has B ≈ 255 and passes through
//! unchanged.

/// Mean of the blue channel of an RGBA8 buffer. A real tangent-space normal map is blue-dominant
/// (Z≈+1 → B≈255); a grayscale relief map hovers around B≈128.
pub fn mean_blue(rgba: &[u8]) -> f32 {
    if rgba.len() < 4 {
        return 0.0;
    }
    let n = rgba.len() / 4;
    let sum: u64 = (0..n).map(|i| rgba[i * 4 + 2] as u64).sum();
    sum as f32 / n as f32
}

/// A grayscale-relief normal source is called out by a low mean-blue. Anything under 200 is
/// treated as height — a genuine tangent-space normal sits comfortably above.
pub const HEIGHT_MEAN_BLUE: f32 = 200.0;

/// The default `strength` for [`height_to_normal`] — matches Mercs 2's `NORMAL_STRENGTH`.
pub const NORMAL_STRENGTH: f32 = 6.0;

/// Convert a grayscale HEIGHT image into an OpenGL-convention tangent-space normal map
/// (R=X, G=+Y up, B=Z-out) — the encoding glTF, and so Blender's importer, expect.
///
/// Height is read from the red channel (R==G==B on these). `strength` scales the bump, edges
/// clamp, output alpha is opaque. The Y term is `+dy` (image V axis runs downward); flipping the
/// sign would give a DirectX-convention normal.
pub fn height_to_normal(rgba: &[u8], w: u32, h: u32, strength: f32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return rgba.to_vec();
    }
    let height_at = |x: usize, y: usize| -> f32 { rgba[(y * w + x) * 4] as f32 / 255.0 };
    let mut out = vec![0u8; w * h * 4];
    let enc = |v: f32, inv: f32| ((v * inv * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    for y in 0..h {
        for x in 0..w {
            let xl = x.saturating_sub(1);
            let xr = (x + 1).min(w - 1);
            let yu = y.saturating_sub(1);
            let yd = (y + 1).min(h - 1);
            let dx = (height_at(xr, y) - height_at(xl, y)) * 0.5;
            let dy = (height_at(x, yd) - height_at(x, yu)) * 0.5;
            let nx = -dx * strength;
            let ny = dy * strength;
            let nz = 1.0f32;
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            let d = (y * w + x) * 4;
            out[d] = enc(nx, inv);
            out[d + 1] = enc(ny, inv);
            out[d + 2] = enc(nz, inv);
            out[d + 3] = 255;
        }
    }
    out
}

/// A single flat-normal texel `[128, 128, 255, 255]` — an OpenGL-tangent "no perturbation" normal.
/// Handy as a wgpu bind-group fallback for a submesh that has no normal map.
pub const FLAT_NORMAL_RGBA: [u8; 4] = [128, 128, 255, 255];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_blue_flat_grey_below_gate() {
        let rgba: Vec<u8> = (0..16).flat_map(|_| [128, 128, 128, 255]).collect();
        assert!(mean_blue(&rgba) < HEIGHT_MEAN_BLUE);
    }

    #[test]
    fn mean_blue_flat_tangent_normal_above_gate() {
        let rgba: Vec<u8> = (0..16).flat_map(|_| FLAT_NORMAL_RGBA).collect();
        assert!(mean_blue(&rgba) >= HEIGHT_MEAN_BLUE);
    }

    #[test]
    fn height_to_normal_flat_is_pointing_out() {
        // A flat grey height map (zero gradient) should yield a flat +Z normal everywhere.
        let rgba: Vec<u8> = (0..64).flat_map(|_| [128, 128, 128, 255]).collect();
        let n = height_to_normal(&rgba, 8, 8, NORMAL_STRENGTH);
        for i in 0..64 {
            assert_eq!(n[i * 4], 128, "R (X) ~0");
            assert_eq!(n[i * 4 + 1], 128, "G (Y) ~0");
            assert_eq!(n[i * 4 + 2], 255, "B (Z) ~+1");
            assert_eq!(n[i * 4 + 3], 255, "opaque");
        }
    }

    /// Guard the CONVENTION: Saboteur's `_NM` textures are genuine DXT1 tangent-space normals
    /// (Z ≈ +1 → B ≈ 250), NOT the grayscale relief that Mercs 2 ships. This is why the bundle
    /// exporter decodes them straight through instead of running [`height_to_normal`] — running
    /// that on real tangent normals would treat the red channel as a height field and produce
    /// garbage. If a future patch changes the convention (perhaps a modder ships DXT5nm), this
    /// test fails and the exporter needs to grow a per-texture gate. Skips when no install.
    ///
    /// Measured over Sean / Bishop / Georgia's `_NM` textures on the retail build: every sampled
    /// record showed meanB > 240 and zero B==G==R pixels — pure tangent-space normals.
    #[test]
    fn saboteur_normalmaps_are_tangent_space() {
        use crate::{pack::Megapack, resolve};
        let Some(s) = crate::settings::detected_retail() else {
            eprintln!("skip: no Saboteur install detected");
            return;
        };
        let Ok(mp) = Megapack::open(&s.megapack()) else {
            eprintln!("skip: megapack unavailable");
            return;
        };
        let assets = resolve::textures_in(&mp, "SeanDevlin").expect("scan the megapack");
        let normals: Vec<&resolve::TexAsset> =
            assets.iter().filter(|a| a.role == resolve::Role::Normal).take(6).collect();
        if normals.is_empty() {
            eprintln!("skip: no `_NM` textures found for token 'SeanDevlin'");
            return;
        }
        let mut checked = 0;
        for a in &normals {
            let Ok(t) = a.decode() else { continue };
            let mb = mean_blue(&t.rgba);
            assert!(
                mb >= HEIGHT_MEAN_BLUE,
                "{} mean_blue = {:.1} — below the grayscale-relief gate ({}). Saboteur's normal \
                 convention may have changed; the exporter needs a per-texture height gate.",
                a.name,
                mb,
                HEIGHT_MEAN_BLUE
            );
            checked += 1;
        }
        assert!(checked > 0, "no `_NM` records actually decoded");
    }

    #[test]
    fn height_to_normal_ramp_leans_the_right_way() {
        // A left-to-right height ramp — the surface slopes down as X increases, so the outward
        // normal leans in the -X direction (R < 128).
        let w = 8u32;
        let h = 4u32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = (x * 32) as u8;
                let d = ((y * w + x) * 4) as usize;
                rgba[d] = v;
                rgba[d + 1] = v;
                rgba[d + 2] = v;
                rgba[d + 3] = 255;
            }
        }
        let n = height_to_normal(&rgba, w, h, NORMAL_STRENGTH);
        // Middle column, middle row — clear of the clamp edges.
        let mid = ((1 * w + 4) * 4) as usize;
        assert!(n[mid] < 128, "R should be negative (nx < 0), got {}", n[mid]);
    }
}
