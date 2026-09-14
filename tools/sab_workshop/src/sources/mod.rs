//! Per-version implementations of `crate::source::GameSource`. One file per version. Nothing in
//! this tree cross-references — a new version = a new file here + a new fingerprint arm in
//! `source::detect_version`.

pub mod prototype2008;
pub mod prototype2008_anim;
pub mod prototype2008_material;
pub mod prototype2008_mesh;
pub mod prototype2008_subpack;
pub mod retail_pc;
