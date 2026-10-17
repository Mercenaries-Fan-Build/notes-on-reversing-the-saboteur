//! The June 2008 Xbox 360 prototype (`Bin360/`, `Saboteur.xex`, the `WildStar_d` debug build).
//!
//! Every multi-byte field in this build's assets is big-endian unless a module says otherwise.
//! The containers, outermost first:
//!
//! * [`mp00`] — `mega*.pack`: an archive of SBL sub-packs.
//! * [`sbl`]  — `.Pack` / an MP00 entry: the `SBL*` sub-pack, a directory of typed slots.
//! * [`slot_dir`] — the `meshes` and `textures` slots: a directory of per-asset records.
//! * [`mesh`] — a `meshes` record: an `MSH4`-layout mesh body, or a variant record wrapping one.
//! * [`texture`] — a `textures` record: header and the tiled GPU memory image.

pub mod mesh;
pub mod mp00;
pub mod sbl;
pub mod slot_dir;
pub mod texture;
