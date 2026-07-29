//! EditNodes.pack — The Saboteur's **dynamic-object database**: the NPCs, vehicles, fences,
//! spawners, sound/zone triggers and other per-scene objects that missions enable and spawn at
//! runtime (via Lua). One `EditNodes.pack` is embedded in `France/loosefiles_BinPC.pack` (the main
//! world, 767 entries) and small standalone ones ship per DLC slot (`DLC/NN/France/EditNodes/`).
//!
//! ```text
//! Pack:   "00ED", u32 count, count × { u32 hash, u32 size, u32 offset }   -- entries contiguous
//!         after the directory, in directory order, to EOF (offset absolute).
//! Entry root: u32 version==0  -> EditNode { u32 count, count × Node }      -- retail is 100% this
//!             else            -> Container { u32 tag(=version), u32 count, count × root }  (defensive)
//! Node:   u32 tag, u32 n, data
//!         - LEAF  : n is a BYTE count; data is n bytes
//!         - CONTAINER: n is a CHILD count; n Node children follow
//! ```
//!
//! **Tag = `pandemic_hash(paramName)` stored little-endian** (same convention as GameText's
//! `asset_id`), so reading a tag as an LE `u32` yields the hash directly. 250 of the 264 names in
//! [`TAG_NAMES`] hash-verify against their tag; the remaining 14 are community placeholders
//! (`Unknown_*`, `WTFPortal*`, `Root`) whose real names were never recovered. See the test
//! `tag_names_sorted_and_mostly_preimages`.
//!
//! The `NodeNN` / `FenceNodeNN` / `FenceWallFlagNN` families are **not** guesses: `Saboteur.exe`
//! builds them with `sprintf("Node%.2d")`, `"FenceNode%.2d"`, `"FenceNodeFlag%.2d"` and
//! `"FenceWallFlag%.2d"` (string block at `0x00c4e0b8`, owned by `WSStreamingManager.cpp`), and the
//! expansions reproduce 113 tags in the retail pack exactly.
//!
//! ## Object nodes carry an instance ID, not a name hash
//! A node whose children include a `ClassName` is an **object**. Its own tag is a per-instance
//! unique id — NOT `pandemic_hash` of anything stored in the file (13 426 objects, 13 397 distinct
//! ids; 640 names map to more than one id). Retail reuses an id across identical copy-pasted
//! objects, so cloning a node *with its tag* matches shipping practice. Note also that the two
//! name-ish fields are inverted from the intuitive reading: `Type` is the class (`"Spore"`,
//! `"Locator"`) and `ClassName` is the instance name (`"Spore_RS_Jules"`).
//!
//! ## Framing is positional, not value-inferred
//! `n` means different things by DEPTH, and nothing needs to inspect a payload to know which:
//! root count = objects, object count = params, **param count = BYTES**. This is the engine's own
//! rule — its reader `FUN_00a005a0` loops `param_count` times and advances `p = data + n`
//! unconditionally — and a depth census of the retail pack agrees exactly: 13 426 containers at
//! depth 0 (all objects), 138 850 leaves at depth 1, **zero containers below depth 0** in 152 276
//! nodes. The body is therefore not a recursive tree; it is one level deep.
//!
//! Earlier versions classified leaf-vs-container by value, which shortcut `n == 0|1|4` to "leaf" and
//! would have silently truncated any object with exactly 4 params (retail's minimum is 5, so it
//! never fired on shipped data — but authored data would). See
//! `object_with_four_params_is_not_mistaken_for_a_leaf`. Both retail packs round-trip
//! byte-identically. See `docs/formats/editnodes.md`.

use crate::pandemic_hash;

const MAGIC: &[u8; 4] = b"00ED";

// ------------------------------------------------------------------ model

/// A parsed `EditNodes.pack`: a directory of entries, each a node tree.
pub struct Pack {
    pub entries: Vec<Entry>,
}

pub struct Entry {
    /// Directory key. Its preimage is not yet pinned; kept verbatim and re-emitted.
    pub hash: u32,
    pub root: Root,
}

/// A pack entry's root. Retail packs are 100% [`Root::EditNode`]; [`Root::Container`] mirrors the
/// inspector's fallback for an entry whose first word is not 0 (not seen in retail data).
pub enum Root {
    EditNode { objects: Vec<Node> },
    Container { tag: u32, children: Vec<Root> },
}

/// A property node: a **leaf** (holds `n` bytes verbatim) or a **container** (holds `n` children).
pub enum Node {
    Leaf { tag: u32, data: Vec<u8> },
    Container { tag: u32, children: Vec<Node> },
}

/// A decoded leaf value, for display only — the round-trip keeps `Leaf::data` verbatim.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Bool(bool),
    U32(u32),
    Raw64([u8; 8]),
    Str(String),
    Vec3([f32; 3]),
    /// A `name\0` + 4-byte type marker + payload ("LuaParam"); we surface the name.
    Named(String),
    /// A leaf we did not decode further; the length.
    Bytes(usize),
}

// ------------------------------------------------------------------ reader

fn u32_at(b: &[u8], o: usize, end: usize) -> Result<u32, String> {
    if o + 4 > end {
        return Err(format!("EOF reading u32 at {o} (entry end {end})"));
    }
    Ok(u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]))
}

impl Pack {
    pub fn parse(b: &[u8]) -> Result<Pack, String> {
        if b.get(0..4) != Some(MAGIC.as_slice()) {
            return Err(format!("not an EditNodes.pack (magic {:?})", b.get(0..4)));
        }
        let count = u32_at(b, 4, b.len())? as usize;
        let mut dir = Vec::with_capacity(count);
        let mut o = 8;
        for i in 0..count {
            let hash = u32_at(b, o, b.len())?;
            let size = u32_at(b, o + 4, b.len())? as usize;
            let off = u32_at(b, o + 8, b.len())? as usize;
            let end = off.checked_add(size).ok_or_else(|| format!("entry {i} size overflow"))?;
            if end > b.len() {
                return Err(format!("entry {i} (hash {hash:08x}) runs past EOF"));
            }
            dir.push((hash, size, off, end));
            o += 12;
        }
        let mut entries = Vec::with_capacity(count);
        for (i, (hash, size, off, end)) in dir.into_iter().enumerate() {
            let (root, consumed) = parse_root(b, off, end)?;
            if consumed != end {
                return Err(format!(
                    "entry {i} (hash {hash:08x}): consumed {} of declared {size} bytes",
                    consumed - off
                ));
            }
            entries.push(Entry { hash, root });
        }
        Ok(Pack { entries })
    }

    /// Serialize back to bytes: `"00ED"`, count, directory (offsets/sizes recomputed), then each
    /// entry body in directory order. Unmodified input round-trips byte-identically.
    pub fn write(&self) -> Vec<u8> {
        let bodies: Vec<Vec<u8>> = self.entries.iter().map(|e| e.root.to_bytes()).collect();
        let dir_end = 8 + self.entries.len() * 12;
        let total: usize = dir_end + bodies.iter().map(|v| v.len()).sum::<usize>();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut off = dir_end;
        for (e, body) in self.entries.iter().zip(&bodies) {
            out.extend_from_slice(&e.hash.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(off as u32).to_le_bytes());
            off += body.len();
        }
        for body in &bodies {
            out.extend_from_slice(body);
        }
        out
    }
}

fn parse_root(b: &[u8], o: usize, end: usize) -> Result<(Root, usize), String> {
    let first = u32_at(b, o, end)?;
    let count = u32_at(b, o + 4, end)? as usize;
    let mut pos = o + 8;
    if first == 0 {
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            let (n, p) = parse_object(b, pos, end)?;
            objects.push(n);
            pos = p;
        }
        Ok((Root::EditNode { objects }, pos))
    } else {
        let mut children = Vec::with_capacity(count);
        for _ in 0..count {
            let (r, p) = parse_root(b, pos, end)?;
            children.push(r);
            pos = p;
        }
        Ok((Root::Container { tag: first, children }, pos))
    }
}

/// An **object**: `u32 instance_id, u32 param_count`, then exactly `param_count` params.
///
/// `n` is a CHILD COUNT here and a BYTE COUNT one level down — the split is positional, decided by
/// depth, never by inspecting the value. This mirrors the engine's reader `FUN_00a005a0`, which
/// loops `param_count` times and advances with `next = child + 8 + n` unconditionally.
fn parse_object(b: &[u8], o: usize, end: usize) -> Result<(Node, usize), String> {
    let tag = u32_at(b, o, end)?;
    let count = u32_at(b, o + 4, end)? as usize;
    let mut pos = o + 8;
    let mut children = Vec::with_capacity(count.min(256));
    for i in 0..count {
        let (c, p) = parse_param(b, pos, end).map_err(|e| format!("object {tag:08x} param {i}: {e}"))?;
        children.push(c);
        pos = p;
    }
    Ok((Node::Container { tag, children }, pos))
}

/// A **param**: `u32 tag, u32 byte_len`, then `byte_len` bytes kept verbatim. Always a leaf — the
/// retail packs contain 138 850 params and **not one container below an object** (depth census in
/// `docs/formats/editnodes.md`). Any nesting inside `data` is that tag's own private encoding, which
/// the engine hands to a per-tag sub-reader; it is not generic node framing.
fn parse_param(b: &[u8], o: usize, end: usize) -> Result<(Node, usize), String> {
    let tag = u32_at(b, o, end)?;
    let n = u32_at(b, o + 4, end)? as usize;
    let ds = o + 8;
    let de = ds.checked_add(n).ok_or_else(|| format!("tag {tag:08x}: length {n} overflows"))?;
    if de > end {
        return Err(format!("tag {tag:08x}: {n} bytes at {ds} runs past entry end {end}"));
    }
    Ok((Node::Leaf { tag, data: b[ds..de].to_vec() }, de))
}

impl Root {
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Root::EditNode { objects } => {
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&(objects.len() as u32).to_le_bytes());
                for n in objects {
                    n.write_into(&mut out);
                }
            }
            Root::Container { tag, children } => {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&(children.len() as u32).to_le_bytes());
                for c in children {
                    out.extend_from_slice(&c.to_bytes());
                }
            }
        }
        out
    }
}

impl Node {
    pub fn tag(&self) -> u32 {
        match self {
            Node::Leaf { tag, .. } | Node::Container { tag, .. } => *tag,
        }
    }

    fn write_into(&self, out: &mut Vec<u8>) {
        match self {
            Node::Leaf { tag, data } => {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(data);
            }
            Node::Container { tag, children } => {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&(children.len() as u32).to_le_bytes());
                for c in children {
                    c.write_into(out);
                }
            }
        }
    }

    /// Decode a leaf's bytes for display (never affects the round-trip). `None` for a container.
    pub fn value(&self) -> Option<Value> {
        let (tag, d) = match self {
            Node::Leaf { tag, data } => (*tag, data.as_slice()),
            Node::Container { .. } => return None,
        };
        Some(match d.len() {
            0 => Value::Empty,
            1 => Value::Bool(d[0] != 0),
            4 => Value::U32(u32::from_le_bytes([d[0], d[1], d[2], d[3]])),
            8 if RAW64_TAGS.contains(&tag) => Value::Raw64([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]]),
            _ => {
                if is_ascii_string_leaf(d) {
                    Value::Str(String::from_utf8_lossy(&d[..d.len() - 1]).into_owned())
                } else if let Some(name) = named_name(d) {
                    Value::Named(name)
                } else if d.len() == 12 && (VECTOR_TAGS.contains(&tag) || looks_like_vec3(d)) {
                    Value::Vec3([f32_at(d, 0), f32_at(d, 4), f32_at(d, 8)])
                } else {
                    Value::Bytes(d.len())
                }
            }
        })
    }
}

// ------------------------------------------------------------------ editing

/// `pandemic_hash` of the params an editor touches most. Pinned by `well_known_tags_are_preimages`.
pub mod tags {
    /// The object's **instance name** (`"Spore_RS_Jules"`) — despite the name.
    pub const CLASS_NAME: u32 = 0x5d78_ede2;
    /// The object's **class** (`"Spore"`, `"Locator"`) — despite the name.
    pub const TYPE: u32 = 0x28e1_0525;
    pub const POSITION: u32 = 0x05d7_fc60;
    pub const X_AXIS: u32 = 0xfbb4_bda8;
    pub const Y_AXIS: u32 = 0xd0c2_6e7d;
    pub const Z_AXIS: u32 = 0x4fb7_6002;
}

fn vec3_bytes(v: [f32; 3]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// The prefix the engine prepends to a container path before hashing it. Lua never writes it.
pub const CONTAINER_PREFIX: &str = r"France\EditNodes\";

fn strip_wsd(path: &str) -> &str {
    path.strip_suffix(".wsd").or_else(|| path.strip_suffix(".WSD")).unwrap_or(path)
}

/// Directory hash of a container, from the path Lua spawns it with.
///
/// ```text
/// Util.SpawnEditNode(r"Missions\freeplay\country\mis_giselle_rescue\main.wsd")
///   -> pandemic_hash(r"France\EditNodes\Missions\freeplay\country\mis_giselle_rescue\main.wsd")
/// ```
///
/// Lua omits the `France\EditNodes\` prefix; the engine prepends it. Accepts the path with or
/// without `.wsd` and always hashes the `.wsd` form. Verified: **97 of the 106** `.wsd` path literals
/// in `LuaScripts.luap` reproduce a directory hash in the retail pack (the rest are DLC or cut).
pub fn container_hash(path: &str) -> u32 {
    pandemic_hash(&format!("{CONTAINER_PREFIX}{}.wsd", strip_wsd(path)))
}

/// An object's tag, from its container's path and its **editor name**.
///
/// ```text
/// Util.GetHandleByName(r"Missions\freeplay\country\mis_giselle_rescue\main\Giselle")
///   -> pandemic_hash(that same string)
/// ```
///
/// Note the two are hashed **differently**: a container carries the `France\EditNodes\` prefix and
/// the `.wsd` extension, an object carries **neither** — it is the container path with `.wsd`
/// dropped, a separator, then the name. Verified: 1 632 path literals in `LuaScripts.luap` reproduce
/// an object tag, and **1 620 / 1 620** of those whose container is in the base pack are found inside
/// exactly the container their path predicts (the other 12 name DLC/cut containers).
///
/// ⚠️ The editor name is **not stored in the pack** — only ~6.7% of objects happen to have a
/// `ClassName` equal to it. The name exists only in this hash and in Lua, so it cannot be recovered
/// from an EditNodes file alone; but it *can* be recomputed, which is what makes a clone addressable.
pub fn object_tag(container_path: &str, object_name: &str) -> u32 {
    pandemic_hash(&format!("{}\\{object_name}", strip_wsd(container_path)))
}

impl Pack {
    pub fn entry(&self, hash: u32) -> Option<&Entry> {
        self.entries.iter().find(|e| e.hash == hash)
    }

    pub fn entry_mut(&mut self, hash: u32) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.hash == hash)
    }

    /// Build the **sparse DLC override pack**: a new pack holding only `hashes`, copied from `self`.
    ///
    /// This is the publish step. A DLC-slot pack shadows the base pack **per directory hash** and a
    /// shadowed entry is replaced *wholesale* (`docs/formats/editnodes.md`), so the recipe is: take
    /// the base entry entire, edit it, and ship only the entries you touched. Entries you leave out
    /// keep falling through to the base pack — copying them in would be harmless but pointless, and
    /// would freeze them against future patches.
    ///
    /// Errors if a hash is not present, since silently shipping a smaller pack than intended is
    /// exactly the failure that is invisible until you are in-game.
    pub fn override_pack(&self, hashes: &[u32]) -> Result<Pack, String> {
        let mut entries = Vec::with_capacity(hashes.len());
        for &h in hashes {
            let src = self.entry(h).ok_or_else(|| format!("entry {h:08x} is not in this pack"))?;
            entries.push(Entry { hash: h, root: src.root.deep_clone() });
        }
        Ok(Pack { entries })
    }
}

impl Root {
    fn deep_clone(&self) -> Root {
        match self {
            Root::EditNode { objects } => Root::EditNode { objects: objects.iter().map(Node::deep_clone).collect() },
            Root::Container { tag, children } => {
                Root::Container { tag: *tag, children: children.iter().map(Root::deep_clone).collect() }
            }
        }
    }
}

impl Entry {
    /// The entry's objects, or `&[]` for the defensive `Root::Container` shape (absent from retail).
    pub fn objects(&self) -> &[Node] {
        match &self.root {
            Root::EditNode { objects } => objects,
            Root::Container { .. } => &[],
        }
    }

    pub fn objects_mut(&mut self) -> Option<&mut Vec<Node>> {
        match &mut self.root {
            Root::EditNode { objects } => Some(objects),
            Root::Container { .. } => None,
        }
    }

    /// Index of the first object whose `ClassName` (the instance name) matches, case-sensitively.
    pub fn find_object(&self, instance_name: &str) -> Option<usize> {
        self.objects().iter().position(|o| o.class_name().as_deref() == Some(instance_name))
    }

    /// Clone object `index` and insert the copy directly after it, returning the new index.
    ///
    /// **The tag is copied verbatim**, so the copy has the same Lua-addressable name as its
    /// template — `Util.GetHandleByName` cannot tell them apart. Retail does contain such pairs (13
    /// tags are used more than once, always same-container and same-`ClassName`: objects copied in
    /// the editor and never renamed), so this is legal, but it is rarely what you want.
    ///
    /// Prefer [`Entry::duplicate_object_as`], which gives the copy its own name.
    pub fn duplicate_object(&mut self, index: usize) -> Result<usize, String> {
        let objects = self.objects_mut().ok_or("entry has no object list")?;
        let copy = objects.get(index).ok_or_else(|| format!("no object at index {index}"))?.deep_clone();
        objects.insert(index + 1, copy);
        Ok(index + 1)
    }

    /// Clone object `index` and give the copy its **own name**, so Lua can address it individually.
    ///
    /// The name is not written into the object — it exists only as the tag,
    /// `object_tag(container_path, new_name)` — so after this the copy answers to
    /// `Util.GetHandleByName(r"<container_path without .wsd>\<new_name>")`.
    ///
    /// `container_path` must be the path of the entry this object lives in, as Lua spells it (no
    /// `France\EditNodes\` prefix). Errors if the resulting tag already exists in this entry, which
    /// would make both objects unaddressable.
    pub fn duplicate_object_as(
        &mut self,
        index: usize,
        container_path: &str,
        new_name: &str,
    ) -> Result<usize, String> {
        let tag = object_tag(container_path, new_name);
        if self.objects().iter().any(|o| o.tag() == tag) {
            return Err(format!("{container_path}\\{new_name} already exists in this entry ({tag:08x})"));
        }
        let j = self.duplicate_object(index)?;
        self.objects_mut().expect("checked above")[j].set_tag(tag);
        Ok(j)
    }

    pub fn remove_object(&mut self, index: usize) -> Result<Node, String> {
        let objects = self.objects_mut().ok_or("entry has no object list")?;
        if index >= objects.len() {
            return Err(format!("no object at index {index}"));
        }
        Ok(objects.remove(index))
    }
}

impl Node {
    fn deep_clone(&self) -> Node {
        match self {
            Node::Leaf { tag, data } => Node::Leaf { tag: *tag, data: data.clone() },
            Node::Container { tag, children } => {
                Node::Container { tag: *tag, children: children.iter().map(Node::deep_clone).collect() }
            }
        }
    }

    /// Retag a node. For an object the tag **is** its Lua-addressable name (see [`object_tag`]), so
    /// this is how an object is renamed — there is no name field to edit.
    pub fn set_tag(&mut self, tag: u32) {
        match self {
            Node::Leaf { tag: t, .. } | Node::Container { tag: t, .. } => *t = tag,
        }
    }

    /// An object's params. Empty for a leaf.
    pub fn params(&self) -> &[Node] {
        match self {
            Node::Container { children, .. } => children,
            Node::Leaf { .. } => &[],
        }
    }

    /// Raw bytes of the first param with `tag`.
    pub fn param(&self, tag: u32) -> Option<&[u8]> {
        self.params().iter().find_map(|p| match p {
            Node::Leaf { tag: t, data } if *t == tag => Some(data.as_slice()),
            _ => None,
        })
    }

    /// Set (or append) a param. Returns `true` if an existing param was replaced.
    ///
    /// Appending grows the object's param count, which the writer re-emits — there are no offsets to
    /// fix, because framing is positional.
    pub fn set_param(&mut self, tag: u32, data: Vec<u8>) -> Result<bool, String> {
        let Node::Container { children, .. } = self else {
            return Err("set_param on a leaf; params live on objects".into());
        };
        for p in children.iter_mut() {
            if let Node::Leaf { tag: t, data: d } = p {
                if *t == tag {
                    *d = data;
                    return Ok(true);
                }
            }
        }
        children.push(Node::Leaf { tag, data });
        Ok(false)
    }

    /// A NUL-terminated ASCII param, without the terminator.
    pub fn string_param(&self, tag: u32) -> Option<String> {
        let d = self.param(tag)?;
        let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
        Some(String::from_utf8_lossy(&d[..end]).into_owned())
    }

    /// Write a NUL-terminated ASCII param.
    pub fn set_string_param(&mut self, tag: u32, s: &str) -> Result<bool, String> {
        let mut data = s.as_bytes().to_vec();
        data.push(0);
        self.set_param(tag, data)
    }

    /// The object's **instance name** (`ClassName`).
    pub fn class_name(&self) -> Option<String> {
        self.string_param(tags::CLASS_NAME)
    }

    pub fn set_class_name(&mut self, name: &str) -> Result<bool, String> {
        self.set_string_param(tags::CLASS_NAME, name)
    }

    /// The object's **class** (`Type`) — `"Spore"`, `"Locator"`, ...
    pub fn type_name(&self) -> Option<String> {
        self.string_param(tags::TYPE)
    }

    pub fn vec3_param(&self, tag: u32) -> Option<[f32; 3]> {
        let d = self.param(tag)?;
        if d.len() != 12 {
            return None;
        }
        Some([f32_at(d, 0), f32_at(d, 4), f32_at(d, 8)])
    }

    pub fn position(&self) -> Option<[f32; 3]> {
        self.vec3_param(tags::POSITION)
    }

    pub fn set_position(&mut self, p: [f32; 3]) -> Result<bool, String> {
        self.set_param(tags::POSITION, vec3_bytes(p))
    }

    /// Set the rotation basis to a yaw about the up axis, in degrees.
    ///
    /// Y is up and yaw turns in the XZ plane: `X = (cos, 0, -sin)`, `Y = (0, 1, 0)`,
    /// `Z = (sin, 0, cos)`. The engine expands `Position` with `w = 1` and these with `w = 0`.
    pub fn set_yaw_degrees(&mut self, deg: f32) -> Result<(), String> {
        let (s, c) = deg.to_radians().sin_cos();
        self.set_param(tags::X_AXIS, vec3_bytes([c, 0.0, -s]))?;
        self.set_param(tags::Y_AXIS, vec3_bytes([0.0, 1.0, 0.0]))?;
        self.set_param(tags::Z_AXIS, vec3_bytes([s, 0.0, c]))?;
        Ok(())
    }
}

/// The human name for a tag, if known ([`TAG_NAMES`], mostly `pandemic_hash` preimages).
pub fn tag_name(tag: u32) -> Option<&'static str> {
    TAG_NAMES.binary_search_by_key(&tag, |&(t, _)| t).ok().map(|i| TAG_NAMES[i].1)
}

// ------------------------------------------------------------------ classification heuristic

fn is_printable(b: u8) -> bool {
    b == 9 || b == 10 || b == 13 || (32..127).contains(&b)
}

fn is_ascii_string_leaf(d: &[u8]) -> bool {
    !d.is_empty() && d[d.len() - 1] == 0 && d[..d.len() - 1].iter().all(|&b| is_printable(b))
}

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn looks_like_vec3(d: &[u8]) -> bool {
    d.len() == 12
        && (0..3).all(|i| {
            let v = f32_at(d, i * 4);
            v.is_finite() && v.abs() < 1e8
        })
}

/// True if `d` is a `name\0` + 4-byte type marker + payload ("LuaParam") leaf.
fn named_name(d: &[u8]) -> Option<String> {
    if d.len() < 9 {
        return None;
    }
    let name_end = d.iter().position(|&b| b == 0)?;
    if name_end == 0 || !d[..name_end].iter().all(|&b| is_printable(b)) {
        return None;
    }
    let marker_end = name_end + 1 + 4;
    if marker_end > d.len() {
        return None;
    }
    let marker = &d[name_end + 1..marker_end];
    let rest = &d[marker_end..];
    let float_m = [0xa9, 0xbd, 0x93, 0x2b];
    let bool_m1 = [0x31, 0x4f, 0x44, 0x1b];
    let bool_m2 = [0x0f, 0x00, 0x00, 0x00];
    let list_m = [0xf9, 0x9d, 0x05, 0xb9];
    let str_m = [0xc2, 0x99, 0xb7, 0xf5];
    let ends_printable_str =
        !rest.is_empty() && rest[rest.len() - 1] == 0 && rest[..rest.len() - 1].iter().all(|&b| is_printable(b));
    let ok = (marker == float_m && rest.len() == 4)
        || ((marker == bool_m1 || marker == bool_m2) && rest.len() == 1)
        || (marker == list_m && rest.len() >= 4)
        || (marker == str_m && ends_printable_str)
        || ends_printable_str
        || rest.len() == 4;
    if ok {
        Some(String::from_utf8_lossy(&d[..name_end]).into_owned())
    } else {
        None
    }
}

// The inspector's `parseProperty` value test used to decide leaf-vs-container here. It is gone:
// framing is positional (see `parse_object`/`parse_param`), so nothing about the file's structure
// depends on what a payload looks like. The surviving value tests live in `Node::value` and only
// choose how to DISPLAY a leaf's bytes.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pandemic_hash;

    #[test]
    fn tag_names_sorted_and_mostly_preimages() {
        assert!(TAG_NAMES.windows(2).all(|w| w[0].0 < w[1].0), "TAG_NAMES must be sorted, unique");
        let verified = TAG_NAMES.iter().filter(|&&(t, n)| pandemic_hash(n) == t).count();
        // 250/264 hash-verify. The remaining 14 are community placeholders whose real names were
        // never recovered (`Unknown_*`, `WTFPortal*`, `UnknownTag`, `Root`, `Raw64_*`); every
        // engine-derived name now verifies. See `docs/formats/editnodes.md`.
        assert_eq!(verified, 250, "expected 250 hash-verified tag names");
    }

    #[test]
    fn tag_lookup() {
        assert_eq!(tag_name(pandemic_hash("ClassName")), Some("ClassName"));
        assert_eq!(tag_name(pandemic_hash("Position")), Some("Position"));
        assert_eq!(tag_name(0xdead_beef), None);
    }

    fn vec3(f: [f32; 3]) -> Vec<u8> {
        f.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A minimal pack: one entry, one object (instance id + params), round-trips byte-identically.
    /// The root's children are OBJECTS; params live one level down.
    #[test]
    fn synthetic_roundtrip() {
        let object = Node::Container {
            tag: 0xdead_1234, // per-instance id, not a name hash
            children: vec![
                Node::Leaf { tag: pandemic_hash("Type"), data: b"Spore\0".to_vec() },
                Node::Leaf { tag: pandemic_hash("ClassName"), data: b"Human_GS_Grunt\0".to_vec() },
                Node::Leaf { tag: pandemic_hash("Position"), data: vec3([95.5, 234.3, 110.9]) },
            ],
        };
        let pack =
            Pack { entries: vec![Entry { hash: 0x1234_5678, root: Root::EditNode { objects: vec![object] } }] };
        let bytes = pack.write();
        let re = Pack::parse(&bytes).unwrap();
        assert_eq!(re.entries.len(), 1);
        assert_eq!(re.write(), bytes, "byte-identical round-trip");
        let Root::EditNode { objects } = &re.entries[0].root else { panic!("expected EditNode root") };
        let Node::Container { tag, children } = &objects[0] else { panic!("expected an object") };
        assert_eq!(*tag, 0xdead_1234);
        assert_eq!(children[1].value(), Some(Value::Str("Human_GS_Grunt".into())));
        assert_eq!(tag_name(children[1].tag()), Some("ClassName"));
        assert!(matches!(children[2].value(), Some(Value::Vec3(_))));
    }

    /// ★ The regression the old value-based classifier could not survive: an object with exactly
    /// 4 params. `n == 4` used to be shortcut to "leaf, 4 bytes", silently truncating the object and
    /// desyncing everything after it. Retail never trips this (min param count is 5) — authored data
    /// easily would. Positional parsing makes it a non-event.
    #[test]
    fn object_with_four_params_is_not_mistaken_for_a_leaf() {
        let object = Node::Container {
            tag: 0x0000_00ff,
            children: vec![
                Node::Leaf { tag: pandemic_hash("Type"), data: b"Locator\0".to_vec() },
                Node::Leaf { tag: pandemic_hash("ClassName"), data: b"L\0".to_vec() },
                Node::Leaf { tag: pandemic_hash("Position"), data: vec3([1.0, 2.0, 3.0]) },
                Node::Leaf { tag: pandemic_hash("XAxis"), data: vec3([1.0, 0.0, 0.0]) },
            ],
        };
        let pack = Pack { entries: vec![Entry { hash: 7, root: Root::EditNode { objects: vec![object] } }] };
        let bytes = pack.write();
        let re = Pack::parse(&bytes).expect("a 4-param object must parse");
        assert_eq!(re.write(), bytes, "byte-identical round-trip");
        let Root::EditNode { objects } = &re.entries[0].root else { panic!("expected EditNode root") };
        let Node::Container { children, .. } = &objects[0] else { panic!("expected an object, got a leaf") };
        assert_eq!(children.len(), 4, "all four params must survive");
    }

    /// Build a Locator-ish object the way a caller would.
    fn sample_object(tag: u32, name: &str) -> Node {
        Node::Container {
            tag,
            children: vec![
                Node::Leaf { tag: tags::TYPE, data: b"Locator\0".to_vec() },
                Node::Leaf { tag: tags::CLASS_NAME, data: format!("{name}\0").into_bytes() },
                Node::Leaf { tag: tags::POSITION, data: vec3([1.0, 2.0, 3.0]) },
                Node::Leaf { tag: tags::X_AXIS, data: vec3([1.0, 0.0, 0.0]) },
                Node::Leaf { tag: tags::Y_AXIS, data: vec3([0.0, 1.0, 0.0]) },
                Node::Leaf { tag: tags::Z_AXIS, data: vec3([0.0, 0.0, 1.0]) },
            ],
        }
    }

    fn sample_pack() -> Pack {
        Pack {
            entries: vec![
                Entry {
                    hash: 0xaaaa_0001,
                    root: Root::EditNode {
                        objects: vec![sample_object(0x1111, "Guard_A"), sample_object(0x2222, "Guard_B")],
                    },
                },
                Entry { hash: 0xbbbb_0002, root: Root::EditNode { objects: vec![sample_object(0x3333, "Car_A")] } },
            ],
        }
    }

    /// The tag constants an editor writes against must really be their names' hashes.
    #[test]
    fn well_known_tags_are_preimages() {
        assert_eq!(tags::CLASS_NAME, pandemic_hash("ClassName"));
        assert_eq!(tags::TYPE, pandemic_hash("Type"));
        assert_eq!(tags::POSITION, pandemic_hash("Position"));
        assert_eq!(tags::X_AXIS, pandemic_hash("XAxis"));
        assert_eq!(tags::Y_AXIS, pandemic_hash("YAxis"));
        assert_eq!(tags::Z_AXIS, pandemic_hash("ZAxis"));
    }

    #[test]
    fn reads_and_writes_object_params() {
        let mut o = sample_object(7, "Guard_A");
        assert_eq!(o.class_name().as_deref(), Some("Guard_A"));
        assert_eq!(o.type_name().as_deref(), Some("Locator"));
        assert_eq!(o.position(), Some([1.0, 2.0, 3.0]));

        assert!(o.set_position([10.0, 20.0, 30.0]).unwrap(), "replaces the existing param");
        assert_eq!(o.position(), Some([10.0, 20.0, 30.0]));
        assert_eq!(o.params().len(), 6, "replacing must not add a param");

        assert!(!o.set_string_param(pandemic_hash("Script"), "MyScript").unwrap(), "appends a new one");
        assert_eq!(o.params().len(), 7);
        assert_eq!(o.string_param(pandemic_hash("Script")).as_deref(), Some("MyScript"));
    }

    #[test]
    fn yaw_writes_the_engines_basis() {
        let mut o = sample_object(7, "Guard_A");
        o.set_yaw_degrees(90.0).unwrap();
        let approx = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert!(approx(o.vec3_param(tags::X_AXIS).unwrap(), [0.0, 0.0, -1.0]));
        assert!(approx(o.vec3_param(tags::Y_AXIS).unwrap(), [0.0, 1.0, 0.0]));
        assert!(approx(o.vec3_param(tags::Z_AXIS).unwrap(), [1.0, 0.0, 0.0]));
    }

    /// The clone-a-template workflow: duplicate, rename, move — and keep the instance id, which is
    /// what retail does for copy-pasted objects.
    #[test]
    fn duplicate_keeps_the_instance_id_and_round_trips() {
        let mut pack = sample_pack();
        let entry = pack.entry_mut(0xaaaa_0001).unwrap();
        let i = entry.find_object("Guard_A").unwrap();
        let j = entry.duplicate_object(i).unwrap();
        assert_eq!(j, i + 1, "the copy lands directly after the template");

        let copy = &mut entry.objects_mut().unwrap()[j];
        assert_eq!(copy.tag(), 0x1111, "instance id is carried over, as retail does");
        copy.set_class_name("Guard_A_Copy").unwrap();
        copy.set_position([50.0, 0.0, 50.0]).unwrap();
        copy.set_yaw_degrees(180.0).unwrap();

        assert_eq!(entry.objects().len(), 3);
        let bytes = pack.write();
        let re = Pack::parse(&bytes).expect("an edited pack must re-parse");
        assert_eq!(re.write(), bytes, "byte-identical after a round-trip");
        let objs = re.entry(0xaaaa_0001).unwrap().objects();
        assert_eq!(objs[1].class_name().as_deref(), Some("Guard_A_Copy"));
        assert_eq!(objs[1].position(), Some([50.0, 0.0, 50.0]));
        assert_eq!(objs[0].class_name().as_deref(), Some("Guard_A"), "the template is untouched");
    }

    /// Container and object hashes, pinned against values recovered from the retail pack by hashing
    /// the path literals in `LuaScripts.luap`.
    #[test]
    fn container_and_object_hashes_reproduce_retail() {
        // `Util.SpawnEditNode(r"Missions\Act_3\Mission_2\main.wsd")`
        assert_eq!(container_hash(r"Missions\Act_3\Mission_2\main.wsd"), 0xffff_6df6);
        assert_eq!(container_hash(r"Missions\Act_3\Mission_2\main"), 0xffff_6df6, ".wsd is optional");
        assert_eq!(container_hash(r"CountrySide\centre\chateaudeisenbourg\aurora.wsd"), 0xa100_b035);

        // `Util.GetHandleByName(...)` — the object path is the SAME string that is hashed.
        assert_eq!(object_tag(r"Missions\act_1\barfight\main.wsd", "LOC_EnterSpot"), 0x32f8_35cb);
        assert_eq!(
            object_tag(r"CountrySide\centre\chateaudeisenbourg\aurora.wsd", "VH_CV_CR_Aurora_01"),
            0x1d7c_ae7e
        );
        // The two schemes differ: a container gets the prefix AND the extension, an object neither.
        assert_ne!(
            object_tag(r"Missions\act_1\barfight\main", "x"),
            container_hash(r"Missions\act_1\barfight\main\x")
        );
    }

    /// Renaming an object means retagging it — there is no name field.
    #[test]
    fn duplicate_as_gives_the_copy_its_own_lua_name() {
        const PATH: &str = r"Missions\test\main.wsd";
        let mut pack = Pack {
            entries: vec![Entry {
                hash: container_hash(PATH),
                root: Root::EditNode { objects: vec![sample_object(object_tag(PATH, "Guard"), "Guard_A")] },
            }],
        };
        let e = pack.entry_mut(container_hash(PATH)).unwrap();
        let j = e.duplicate_object_as(0, PATH, "Guard_Clone").unwrap();
        assert_eq!(e.objects()[j].tag(), object_tag(PATH, "Guard_Clone"));
        assert_ne!(e.objects()[j].tag(), e.objects()[0].tag(), "the copy is addressable on its own");

        // A second copy under the same name would make both unreachable.
        let Err(err) = e.duplicate_object_as(0, PATH, "Guard_Clone") else { panic!("must reject a dupe") };
        assert!(err.contains("already exists"), "{err}");

        let bytes = pack.write();
        assert_eq!(Pack::parse(&bytes).unwrap().write(), bytes);
    }

    /// The publish step: ship only the touched entries, so the rest fall through to base.
    #[test]
    fn override_pack_holds_only_the_touched_entries() {
        let base = sample_pack();
        let dlc = base.override_pack(&[0xbbbb_0002]).unwrap();
        assert_eq!(dlc.entries.len(), 1);
        assert_eq!(dlc.entries[0].hash, 0xbbbb_0002);
        assert_eq!(dlc.entry(0xbbbb_0002).unwrap().objects()[0].class_name().as_deref(), Some("Car_A"));
        assert!(dlc.entry(0xaaaa_0001).is_none(), "untouched entries must fall through, not be copied");
        let bytes = dlc.write();
        assert_eq!(Pack::parse(&bytes).unwrap().write(), bytes);
    }

    /// A deep copy, so editing the override cannot reach back into the base pack.
    #[test]
    fn override_pack_is_independent_of_the_base() {
        let base = sample_pack();
        let mut dlc = base.override_pack(&[0xaaaa_0001]).unwrap();
        dlc.entry_mut(0xaaaa_0001).unwrap().objects_mut().unwrap()[0].set_class_name("Renamed").unwrap();
        assert_eq!(base.entry(0xaaaa_0001).unwrap().objects()[0].class_name().as_deref(), Some("Guard_A"));
    }

    #[test]
    fn override_pack_rejects_an_unknown_hash() {
        let base = sample_pack();
        let Err(err) = base.override_pack(&[0xdead_beef]) else { panic!("must reject an unknown hash") };
        assert!(err.contains("deadbeef"), "{err}");
    }

    /// A param's `n` is a byte count even when its payload looks like a node sequence — the engine
    /// advances `next = child + 8 + n` regardless. `DynamicFenceNodes` is the retail example: a
    /// 4-byte leaf holding a COUNT, whose entries are flat siblings (`FenceNode00`, …).
    #[test]
    fn param_payload_is_never_reparsed_as_children() {
        let object = Node::Container {
            tag: 1,
            children: vec![
                Node::Leaf { tag: pandemic_hash("ClassName"), data: b"Fence\0".to_vec() },
                Node::Leaf { tag: pandemic_hash("DynamicFenceNodes"), data: 4u32.to_le_bytes().to_vec() },
                Node::Leaf { tag: pandemic_hash("FenceNode00"), data: vec3([0.0, 0.0, 0.0]) },
            ],
        };
        let pack = Pack { entries: vec![Entry { hash: 9, root: Root::EditNode { objects: vec![object] } }] };
        let bytes = pack.write();
        let re = Pack::parse(&bytes).unwrap();
        let Root::EditNode { objects } = &re.entries[0].root else { panic!() };
        let Node::Container { children, .. } = &objects[0] else { panic!() };
        assert_eq!(children.len(), 3);
        assert_eq!(children[1].value(), Some(Value::U32(4)), "the count stays a 4-byte leaf");
    }
}


// ------------------------------------------------------------------ generated tag tables
// pandemic_hash(name); placeholder byte-order resolved against the real pack.
// 265 names, 242 hash-verified. Do not edit by hand.

pub(crate) static TAG_NAMES: &[(u32, &str)] = &[
    (0x00000000, "Root"),
    (0x0012bdc3, "BaseName"),
    (0x03330f5c, "Node30"),
    (0x03f4a784, "FenceWallFlag19"),
    (0x05d7fc60, "Position"),
    (0x05e0ed37, "FenceNodeFlag27"),
    (0x0852d5ef, "SoundFake3DSettings"),
    (0x08c714d2, "Node10"),
    (0x08da9c8a, "Node18"),
    (0x08e1d248, "FenceNode26"),
    (0x0a6c5a61, "FenceNodeFlag36"),
    (0x0a95de23, "FenceWallFlag27"),
    (0x0f30c53e, "FenceNodeFlag17"),
    (0x0f535780, "FenceNodeFlag19"),
    (0x0fc275db, "WTFPortal_Float3"),
    (0x117a9fd1, "FenceWallFlag03"),
    (0x125d5c6f, "FenceNodeFlag38"),
    (0x1270e427, "FenceNodeFlag30"),
    (0x12dfa36f, "FenceNode25"),
    (0x16944042, "Node03"),
    (0x17786aac, "FenceWallFlag02"),
    (0x189208a6, "CommonSeatSettings"),
    (0x19421e1d, "FenceNodeFlag10"),
    (0x1a61709c, "PassengerSettings"),
    (0x1acf73a2, "DynamicFenceNodes"),
    (0x1be34e70, "FenceNodeFlag24"),
    (0x1df70f09, "FenceWallFlag18"),
    (0x1e0a96c1, "FenceWallFlag10"),
    (0x1e25164d, "AmbiancePriority"),
    (0x1f317606, "SoundAreaSettings"),
    (0x20cbb7c8, "Node12"),
    (0x22b00b38, "FenceNodeFlag02"),
    (0x23deddda, "FenceNodeFlag26"),
    (0x2408619c, "FenceWallFlag11"),
    (0x24a3426b, "FenceNodeFlag05"),
    (0x28aaccae, "ID0"),
    (0x28e10525, "Type"),
    (0x2ac988ef, "Node11"),
    (0x2add10a7, "Node19"),
    (0x2c40a12b, "CheckVisibility"),
    (0x2cc16417, "FenceNodeFlag09"),
    (0x2f400228, "FenceNodeFlag11"),
    (0x3133395b, "FenceNodeFlag16"),
    (0x31dbbab1, "FenceNode12"),
    (0x34e32c64, "WTFPortal_Float1"),
    (0x361a4769, "Unknown_Container_69471a36"),
    (0x377d1a3a, "FenceWallFlag04"),
    (0x3955d835, "FenceNodeFlag18"),
    (0x39e04477, "FenceNode14"),
    (0x3ce51772, "Flags"),
    (0x404d1343, "LuaTable"),
    (0x42498680, "Script"),
    (0x42a1330e, "FenceNodeFlag04"),
    (0x43301c7e, "FenceNode06"),
    (0x4352aec0, "FenceNode08"),
    (0x43b9013b, "SpawnBlueprint"),
    (0x442cf8ba, "Unknown_Container_baf82c44"),
    (0x46678af8, "Unknown_Container_f88a6746"),
    (0x49c5a031, "Node26"),
    (0x4aa5bcd4, "FenceNodeFlag06"),
    (0x4bb87d44, "AttachedID2"),
    (0x4cb28bed, "FenceNodeFlag03"),
    (0x4d41755d, "FenceNode01"),
    (0x4ee116bb, "WTFPortal_Float2"),
    (0x4fb76002, "ZAxis"),
    (0x4fc0136e, "FenceNode19"),
    (0x4fe2a5b0, "FenceNode17"),
    (0x51b6a23f, "Node28"),
    (0x51ca29f7, "Node20"),
    (0x52645ce5, "PrevID2"),
    (0x52856dba, "Volumes"),
    (0x56340cf8, "FenceWallFlag37"),
    (0x56388a26, "FenceWallFlag39"),
    (0x566b3f52, "Group"),
    (0x56af6278, "FenceNode35"),
    (0x57de351a, "FenceNode15"),
    (0x5827442b, "FenceWallFlag30"),
    (0x58a299ab, "FenceNode32"),
    (0x5b3c9440, "Node34"),
    (0x5c131421, "Node40"),
    (0x5d2923df, "GunnerSettings"),
    (0x5d611e32, "NextID1"),
    (0x5d78ede2, "ClassName"),
    (0x5e58eac8, "Unknown_c8ea585e"),
    (0x60f3b263, "SoundTriggerName"),
    (0x633823aa, "Node36"),
    (0x633f5968, "FenceNode00"),
    (0x6350df48, "DriverSettings"),
    (0x6486e117, "SearcherSettings"),
    (0x6532909b, "FenceNode07"),
    (0x65bae4c9, "AttachedID3"),
    (0x65f01872, "AttractionPt"),
    (0x66ac7e16, "SoundTimeSettings"),
    (0x67cc8b30, "Node23"),
    (0x67ed39c8, "UnknownTag"),
    (0x6873aa20, "FenceNodeFlag33"),
    (0x688e2358, "FenceWallFlag28"),
    (0x68d35ede, "FenceNode20"),
    (0x6a66e153, "FenceNodeFlag34"),
    (0x6a9af271, "FenceWallFlag25"),
    (0x6ae92853, "FenceNode29"),
    (0x6b9391d0, "DynamicPathNodes"),
    (0x6d0c6486, "MaxNumAlive"),
    (0x6d552f75, "FenceNode09"),
    (0x6e9947f8, "Node01"),
    (0x6fc81a9a, "Node21"),
    (0x70464ebf, "Volume"),
    (0x7076575c, "Sound3DSettings"),
    (0x708c7f2b, "Node06"),
    (0x70d7e8a4, "FenceNode22"),
    (0x729f7c37, "FenceWallFlag23"),
    (0x72b61f7c, "WTFPortal_Next"),
    (0x72e4b7bd, "FenceNode27"),
    (0x762534ce, "FenceWallFlag31"),
    (0x76a08a4e, "FenceNode33"),
    (0x783afe43, "FenceWallFlag38"),
    (0x78a10702, "CustomExportTag"),
    (0x78af1e05, "Node08"),
    (0x79450675, "Parent"),
    (0x798a195e, "Persistent"),
    (0x7b2dbc16, "Node32"),
    (0x7bd4db06, "FenceNodeFlag22"),
    (0x7c11e680, "FenceWallFlag15"),
    (0x7c541819, "SoundDistSettings"),
    (0x7dc81239, "FenceNodeFlag29"),
    (0x7e1f64c8, "MinTimeBetweenSpawns"),
    (0x7e29be94, "FenceWallFlag33"),
    (0x7ea51414, "FenceNode31"),
    (0x7ee2f763, "ScriptTriggered"),
    (0x7f451191, "Position1"),
    (0x7f46515c, "XSize"),
    (0x7f63924f, "NextID0"),
    (0x80368dad, "FenceWallFlag36"),
    (0x80b1e32d, "FenceNode34"),
    (0x80bd445e, "Node14"),
    (0x83d964cc, "FenceNodeFlag20"),
    (0x840d75ea, "FenceWallFlag17"),
    (0x853a97c7, "Node37"),
    (0x853f14f5, "Node35"),
    (0x85e633e5, "FenceNodeFlag25"),
    (0x8839711a, "SeatSpawnObject"),
    (0x8864d1f6, "FenceNodeFlag35"),
    (0x88a1dd70, "FenceWallFlag20"),
    (0x88c1ce24, "Node16"),
    (0x88e718f6, "FenceNode28"),
    (0x8aada58b, "ID1"),
    (0x8ab222b9, "ID3"),
    (0x8ace9d3d, "Node13"),
    (0x8ad5d2fb, "FenceNode21"),
    (0x8ada5029, "FenceNode23"),
    (0x8d2eedb7, "Honkable"),
    (0x8e224ef6, "FenceNodeFlag40"),
    (0x8e8a6fce, "Node07"),
    (0x8ead0210, "Node09"),
    (0x8f731766, "FenceWallFlag00"),
    (0x90695bbc, "FenceNodeFlag37"),
    (0x909d6cda, "FenceWallFlag22"),
    (0x9161d16b, "FenceWallFlag09"),
    (0x91f044cf, "CheckPoint"),
    (0x92762ad5, "FenceNodeFlag32"),
    (0x9290a40d, "FenceWallFlag29"),
    (0x92d61cfd, "SoundAmbianceSettings"),
    (0x968ef994, "Node05"),
    (0x97361884, "FenceNodeFlag15"),
    (0x973a95b2, "FenceNodeFlag13"),
    (0x989bc8ad, "Node00"),
    (0x99327d6f, "Width"),
    (0x997ff317, "FenceWallFlag05"),
    (0x99847045, "FenceWallFlag07"),
    (0x9c030e56, "FenceWallFlag13"),
    (0x9dd74f23, "FenceNodeFlag23"),
    (0x9e8ad974, "FenceNode40"),
    (0x9eb3ee85, "YSize"),
    (0xa09a4dfd, "Reverse"),
    (0xa2bfb87b, "Node15"),
    (0xa2c435a9, "Node17"),
    (0xa3c60f74, "FenceNodeFlag28"),
    (0xa4a88919, "FenceNodeFlag07"),
    (0xa50fa6af, "TargetObject"),
    (0xa60fea07, "FenceWallFlag16"),
    (0xa6146735, "FenceWallFlag14"),
    (0xa90ecbbb, "MaxNumSpawns"),
    (0xacad12df, "FenceNodeFlag01"),
    (0xaf5fc20e, "FenceWallFlag08"),
    (0xaf825450, "FenceWallFlag06"),
    (0xafd43246, "FenceNode11"),
    (0xb0b01ff4, "ID2"),
    (0xb1388009, "FenceNodeFlag14"),
    (0xb14f2f93, "RadiusOfEffect"),
    (0xb1524043, "CollisionModule"),
    (0xb1758b83, "FenceWallFlag01"),
    (0xb1bd8bda, "SeatNameRef"),
    (0xb1c2ec4b, "FenceNode18"),
    (0xb2e588d7, "Raw64_d788e5b2"),
    (0xb7d8bc0c, "FenceNode13"),
    (0xb93d09cf, "FenceNodeFlag12"),
    (0xb9e58b25, "FenceNode16"),
    (0xc3b329fe, "AttachedID0"),
    (0xc4197ff4, "LaneCount"),
    (0xc7a270c0, "LowWTFMult"),
    (0xc7be17c6, "Node25"),
    (0xcaab0382, "FenceNodeFlag00"),
    (0xcabe8b3a, "FenceNodeFlag08"),
    (0xcabe9772, "FenceWallFlag40"),
    (0xcb289cd9, "Tangent1"),
    (0xcb356fc4, "FenceNode04"),
    (0xcb39ecf2, "FenceNode02"),
    (0xcbe8ed58, "Distance"),
    (0xce5b1183, "Talkable"),
    (0xcfc2a18c, "Node27"),
    (0xd0c26e7d, "YAxis"),
    (0xd0e81ab8, "InstanceData"),
    (0xd1cf70a5, "Node22"),
    (0xd1d6a663, "FenceNode10"),
    (0xd25f1637, "PrevID0"),
    (0xd3443e4a, "ZSize"),
    (0xd80a2bcb, "WTFPortal"),
    (0xd82c8ad9, "FenceWallFlag32"),
    (0xd8a7e059, "FenceNode30"),
    (0xd8bb6811, "FenceNode38"),
    (0xdb0f705c, "LuaParam"),
    (0xdbaa9984, "Traffic"),
    (0xdc679240, "OuterRadius"),
    (0xdd3094f3, "Node33"),
    (0xdde4ef82, "CustomEnabledDefault"),
    (0xdeb932ec, "FenceNode39"),
    (0xdf68a69d, "NextID2"),
    (0xe031149f, "FenceWallFlag34"),
    (0xe0ac6a1f, "FenceNode36"),
    (0xe1ae04b8, "Height"),
    (0xe31f5544, "Node38"),
    (0xe537d749, "FenceNode05"),
    (0xe5b59e1b, "AttachedID1"),
    (0xe6f43618, "SoundScriptSettings"),
    (0xe728749b, "DistanceTriggered"),
    (0xe85f4cfc, "DynamicFenceFlags"),
    (0xe8617770, "PrevID3"),
    (0xe8936a06, "FenceWallFlag26"),
    (0xe9c08be3, "Node24"),
    (0xed3c610f, "FenceNode03"),
    (0xefb3c962, "Node29"),
    (0xf05ae852, "FenceNodeFlag39"),
    (0xf05d06da, "PrevID1"),
    (0xf06e700a, "FenceNodeFlag31"),
    (0xf091c5d9, "Node04"),
    (0xf097f3cc, "FenceWallFlag24"),
    (0xf0dd2f52, "FenceNode24"),
    (0xf103cebd, "SoundArea"),
    (0xf2a4c2e5, "FenceWallFlag21"),
    (0xf33eac7d, "SoundTriggerSettings"),
    (0xf5668aa8, "NextID3"),
    (0xf7a1ff8c, "SoundWTFSettings"),
    (0xf8964f9f, "Node02"),
    (0xf930d873, "UnknownBool"),
    (0xfa4e0765, "Node"),
    (0xfb8065aa, "AttachedCount"),
    (0xfbb4bda8, "XAxis"),
    (0xfd21bcc9, "Node39"),
    (0xfd354481, "Node31"),
    (0xfd3d8926, "WTFPortal_Vector2"),
    (0xfddc6371, "FenceNodeFlag21"),
    (0xfe05e733, "FenceWallFlag12"),
    (0xfe2f0542, "FenceWallFlag35"),
    (0xfeaa5ac2, "FenceNode37"),
];

pub(crate) static RAW64_TAGS: &[u32] = &[0xb2e588d7];

pub(crate) static VECTOR_TAGS: &[u32] = &[
    0x05d7fc60, 0xfbb4bda8, 0xd0c26e7d, 0x4fb76002, 0x7f451191, 0xfd3d8926,
    0xfa4e0765, 0xcb289cd9, 0x989bc8ad, 0x6e9947f8, 0xf8964f9f, 0x16944042,
    0xf091c5d9, 0x968ef994, 0x708c7f2b, 0x8e8a6fce, 0x78af1e05, 0x8ead0210,
    0x08c714d2, 0x2ac988ef, 0x20cbb7c8, 0x8ace9d3d, 0x80bd445e, 0xa2bfb87b,
    0x88c1ce24, 0xa2c435a9, 0x08d89c8a, 0x2add10a7, 0x51ca29f7, 0x6fc81a9a,
    0xd1cf70a5, 0x67cc8b30, 0xe9c08be3, 0xc7be17c6, 0x49c5a031, 0xcfc2a18c,
    0x51b6a23f, 0xefb3c962, 0x03330f5c, 0xfd354481, 0x7b2dbc16, 0xdd3094f3,
    0x5b3c9440, 0x853f14f5, 0x633823aa, 0x853a97c7, 0xe31f5544, 0xfd21bcc9,
    0x5c131421, 0x633f5968, 0x4d41755d, 0xcb39ecf2, 0xed3c610f, 0xcb356fc4,
    0xe537d749, 0x43301c7e, 0x6532909b, 0x4352aec0, 0x6d552f75, 0xd1d6a663,
    0xafd43246, 0x31dbbab1, 0xb7b8bc0c, 0x39e04477, 0x57de351a, 0xb9e58b25,
    0x4fe2a5b0, 0xb1b2ec4b, 0x4fc0136e, 0x68d35ede, 0x8ad5d2fb, 0x70d7e8a4,
    0x8ada5029, 0xf0dd2f52, 0x12dfa36f, 0x08e1d248, 0x72e4b7bd, 0x88e718f6,
    0x6ae92853, 0xd8a7e059, 0x7ea51414, 0x58a299ab, 0x76a08a4e, 0x80b1e32d,
    0x56af6278, 0xe0ac6a1f, 0xfeaa5ac2, 0xd8bb6811, 0xdeb932ec, 0x9e8ad974,
    0xb7d8bc0c, 0xd260f3a7,
];
