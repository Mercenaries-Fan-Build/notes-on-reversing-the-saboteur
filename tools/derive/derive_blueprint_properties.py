#!/usr/bin/env python3
"""Derive the GameTemplates ("blueprint") property table: key -> name + type.

One-time archaeology, not durable tooling: it reads gitignored game files and the 54 MB decomp
and emits a committed artifact, `data/gametemplates_properties.tsv`. The part that must never
drift is enforced in Rust instead -- `sab_formats::gametemplates`'s table carries a test that
re-hashes every row, so a bad row cannot survive `cargo test`.

Acceptance rule for a NAME (docs/formats/editnodes.md's rule, tightened):
    pandemic_hash(name) == key, strictly little-endian.
docs/formats/gametemplates.md records LE as CONFIRMED (5 matches, 0 BE), and unlike the EditNodes
import we are generating our own preimages, so there is no mangled third-party byte order to
rescue. A name that does not hash is discarded, never labelled "probably".

Sources, in descending preference for SPELLING (all are hash-verified, so all are correct up to
case; this only decides which spelling to display):
  1. the community GameTemplates-inspector dictionary -- curated, and the only source that
     carries names containing SPACES (e.g. "Game Event Name"), which identifier-shaped
     harvesting structurally cannot produce
  2. retail Saboteur.exe printable strings, suffix-aware
  3. the 2008 prototype symbols (WildStar_d.pdb / WildStar_p.pdb / Saboteur.pdb / WildStar_d.map)

TYPE comes from the engine's own reader helpers in the per-class SetProperty dispatchers, and is
only emitted when it AGREES with the observed size histogram over all 10,761 retail templates.
Disagreement means the extraction is wrong, so the type is dropped rather than guessed.

Usage:  python tools/derive/derive_blueprint_properties.py [--out data/gametemplates_properties.tsv]
"""
from __future__ import annotations
import argparse, os, re, struct, sys, glob
from collections import Counter, defaultdict

# --------------------------------------------------------------------------- inputs
def find_install() -> str:
    for c in (os.environ.get("SABOTEUR_DIR"), r"C:\GOG Games\The Saboteur"):
        if c and os.path.isdir(c):
            return c
    sys.exit("set SABOTEUR_DIR to the game install")

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
DECOMP = os.path.join(REPO, "output", "_ghidra_saboteur", "saboteur_all_functions_decomp.txt")
SYMBOLS = os.path.join(REPO, "game-files", "symbols")
DICT_HTML = os.path.join(os.path.expanduser("~"), "Downloads", "GameTemplates-inspector_1.html")


def pandemic_hash(s: str) -> int:
    h = 0x811C9DC5
    for c in s.encode("latin-1", "replace"):
        h = ((h ^ (c | 0x20)) * 0x01000193) & 0xFFFFFFFF
    return ((h ^ 0x2A) * 0x01000193) & 0xFFFFFFFF


# --------------------------------------------------------------------------- retail corpus
def retail_corpus(game: str):
    """(uses, sizes, owning template types) per property key, from the real table."""
    path = os.path.join(game, "France", "loosefiles_BinPC.pack")
    d = open(path, "rb").read()
    cur, payload = 0, None
    while cur + 0x80 <= len(d):
        size = struct.unpack_from("<I", d, cur + 4)[0]
        nm = d[cur + 8 : cur + 0x80].split(b"\0")[0].decode("latin-1")
        if nm.lower().endswith("gametemplates.wsd"):
            payload = d[cur + 0x80 : cur + 0x80 + size]
            break
        cur += 0x80 + size
        if size & 0xF:
            cur += 0x10 - (size & 0xF)
    if not payload or payload[:4] != b"AULB":
        sys.exit("GameTemplates.wsd not found in loosefiles_BinPC.pack")

    uses, sizes, owners = Counter(), defaultdict(Counter), defaultdict(Counter)
    pos, ntpl, nmark = 8, 0, 0
    while pos + 4 <= len(payload):
        total = struct.unpack_from("<I", payload, pos)[0]
        body = pos + 4
        if total <= 0 or body + total > len(payload):
            break
        if total == 8:
            nmark += 1
            pos = body + total
            continue
        p = body + 8
        nl = struct.unpack_from("<I", payload, p)[0]; p += 4 + nl
        cl = struct.unpack_from("<I", payload, p)[0]; p += 4
        ttype = payload[p : p + cl].split(b"\0")[0].decode("latin-1"); p += cl
        pc = struct.unpack_from("<I", payload, p)[0]; p += 4
        for _ in range(pc):
            if p + 8 > body + total:
                break
            k = struct.unpack_from("<I", payload, p)[0]; p += 4
            fs = struct.unpack_from("<I", payload, p)[0]; p += 4
            uses[k] += 1
            sizes[k][fs] += 1
            owners[k][ttype] += 1
            p += fs
        ntpl += 1
        pos = body + total
    return uses, sizes, owners, ntpl, nmark


# --------------------------------------------------------------------------- name sources
IDENT = re.compile(rb"[A-Za-z_][A-Za-z0-9_]{2,63}")
# Space-tolerant: property names really do contain spaces, so a second pass allows them.
PHRASE = re.compile(rb"[A-Za-z][A-Za-z0-9_ ]{2,63}")


def variants(s: str):
    yield s
    m = re.match(r"^(?:m_|s_|g_)([a-zA-Z])?([A-Z].*)$", s)
    if m:
        yield m.group(2)
    m = re.match(r"^[a-z]{1,2}([A-Z][A-Za-z0-9_]*)$", s)
    if m:
        yield m.group(1)


def harvest_strings(paths, keys, phrases=False):
    """key -> name, for every printable run (and suffix) that hashes to a live key."""
    out = {}
    pats = [IDENT, PHRASE] if phrases else [IDENT]
    for fn in paths:
        try:
            b = open(fn, "rb").read()
        except OSError:
            continue
        for pat in pats:
            for m in pat.finditer(b):
                raw = m.group(0).decode("latin-1").strip()
                if len(raw) < 3:
                    continue
                # a pointer into the middle of a C literal is a valid string, so try suffixes
                cands = {raw}
                for i in range(1, min(len(raw), 6)):
                    cands.add(raw[i:])
                for c in cands:
                    for v in variants(c):
                        h = pandemic_hash(v)
                        if h in keys and h not in out:
                            out[h] = v
    return out


def harvest_dictionary(keys):
    """The hash-verified subset of the community inspector's dictionary."""
    if not os.path.exists(DICT_HTML):
        return {}
    src = open(DICT_HTML, encoding="utf-8", errors="replace").read()
    try:
        blk = src[src.index("const FALLBACK_DICT") : src.index("// ===== Loading the external")]
    except ValueError:
        return {}
    out = {}
    for _, name in re.findall(r'"([0-9a-fA-F]{8})"\s*:\s*"([^"]+)"', blk):
        h = pandemic_hash(name)
        if h in keys and h not in out:
            out[h] = name          # key as written is IGNORED -- the hash decides
    return out


# --------------------------------------------------------------------------- type source
READER = {
    "00450f20": "scalar4",   # plain 4 bytes (int or float)
    "004511d0": "hash4",     # 4 bytes, byte-reversing -> CRC / blueprint reference
    "00451290": "vec3",      # 12 bytes
    "0045f440": "string",    # NUL-terminated
}
HDR = re.compile(r"^==== (FUN_[0-9a-f]+) @0x([0-9a-f]+)\s+size=(\d+)")
HEXLIT = re.compile(r"0x([0-9a-f]{7,8})\b")
CALL = re.compile(r"\bFUN_([0-9a-f]{8})\b")
# Only an EQUALITY test puts the key's own body in the following lines. Ghidra also emits inverted
# guards -- `if (param_2 != 0xKEY) { <other key's body> }` with the key's real reader *after* the
# closing brace -- so a forward window from a `!=` attributes the wrong helper. That is not
# hypothetical: it mis-typed 0x246DB06C ("Game Event Name") as a plain scalar when its actual
# reader is the byte-reversing FUN_004511d0. The size histogram cannot catch it, because both
# helpers consume 4 bytes. So `!=` sites count only as "this key is handled in this function".
EQ_HEX = re.compile(r"==\s*(?:\([^)]*\)\s*)?0x([0-9a-f]{7,8})\b")


def harvest_types(keys):
    """key -> (Counter of reader kinds, set of dispatcher functions), from the decomp."""
    if not os.path.exists(DECOMP):
        print("  (decomp absent -- no types)", file=sys.stderr)
        return {}, {}
    blob = open(DECOMP, encoding="utf-8", errors="replace").read()
    kinds, funcs = defaultdict(Counter), defaultdict(set)
    for blk in blob.split("==== ")[1:]:
        nl = blk.find("\n")
        m = HDR.match("==== " + blk[:nl])
        if not m:
            continue
        fname = m.group(1)
        lines = blk.split("\n")
        for i, line in enumerate(lines):
            # every mention attributes the key to this dispatcher ...
            for hm in HEXLIT.finditer(line):
                k = int(hm.group(1), 16)
                if k in keys:
                    funcs[k].add(fname)
            # ... but only an `== 0xKEY` site may claim the reader that follows it
            for hm in EQ_HEX.finditer(line):
                k = int(hm.group(1), 16)
                if k not in keys:
                    continue
                window = "\n".join(lines[i : i + 6])
                for cm in CALL.finditer(window):
                    if cm.group(1) in READER:
                        kinds[k][READER[cm.group(1)]] += 1
                        break
                else:
                    if re.search(r"\+ 1;|\+= 1", window) and re.search(
                        r"\*\(char \*\)|\*\(byte \*\)|cVar", window
                    ):
                        kinds[k]["byte1"] += 1
    return kinds, funcs


# ---- the size histogram is the oracle: a reader kind is only accepted if it agrees with it
EXPECT = {"byte1": {1}, "scalar4": {4}, "hash4": {4}, "vec3": {12}}
TYPE_OF = {"byte1": "Bool", "scalar4": "Scalar4", "hash4": "Ref", "vec3": "Vec3", "string": "Str"}


def classify(key, sizes, kinds):
    obs = set(sizes[key])
    best = None
    for kind, _ in kinds.get(key, Counter()).most_common():
        if kind == "string":
            best = "Str"
            break
        if kind in EXPECT and obs <= EXPECT[kind]:
            best = TYPE_OF[kind]
            break
    if best:
        return best, "decomp+size"
    # fall back to the corpus shape alone
    if obs == {1}:
        return "Bool", "size"
    if obs == {12}:
        return "Vec3", "size"
    if obs == {4}:
        return "Scalar4", "size"
    return "Bytes", "size"


# --------------------------------------------------------------------------- main
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(REPO, "data", "gametemplates_properties.tsv"))
    ap.add_argument(
        "--rs",
        default=os.path.join(REPO, "tools", "sab_formats", "src", "gametemplates_props.rs"),
        help="generated Rust tables",
    )
    args = ap.parse_args()

    game = find_install()
    uses, sizes, owners, ntpl, nmark = retail_corpus(game)
    keys = set(uses)
    tot = sum(uses.values())
    print(f"retail: {ntpl} templates + {nmark} markers, {len(keys)} keys, {tot} pairs")

    print("harvesting types from the decomp ...", file=sys.stderr)
    kinds, funcs = harvest_types(keys)

    print("harvesting names ...", file=sys.stderr)
    d_dict = harvest_dictionary(keys)
    d_exe = harvest_strings([os.path.join(game, "Saboteur.exe")], keys, phrases=True)
    d_pdb = harvest_strings(sorted(glob.glob(os.path.join(SYMBOLS, "*"))), keys, phrases=True)

    names, source = {}, {}
    for label, table in (("dict", d_dict), ("exe", d_exe), ("sym", d_pdb)):
        for k, n in table.items():
            if k not in names:
                names[k], source[k] = n, label

    # every row must hash. belt and braces -- the harvesters already enforce it.
    bad = [k for k, n in names.items() if pandemic_hash(n) != k]
    assert not bad, f"{len(bad)} rows do not hash: {bad[:4]}"

    named_uses = sum(uses[k] for k in names)
    print(f"\nNAMES  {len(names)}/{len(keys)} keys = {100*named_uses/tot:.1f}% of pairs")
    for label in ("dict", "exe", "sym"):
        n = sum(1 for k in names if source[k] == label)
        u = sum(uses[k] for k in names if source[k] == label)
        print(f"   {label:5} {n:5} keys  {100*u/tot:5.1f}% of pairs")

    typed = {k: classify(k, sizes, kinds) for k in keys}
    strong = sum(1 for k in keys if typed[k][1] == "decomp+size")
    print(f"\nTYPES  {strong} keys corroborated by decomp+size; "
          f"{len(keys)-strong} from the size histogram alone")
    tcount = Counter(t for t, _ in typed.values())
    print("   " + "  ".join(f"{t}={c}" for t, c in tcount.most_common()))

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("# GameTemplates blueprint property table -- DERIVED, do not hand-edit.\n")
        f.write("# Regenerate: python tools/derive/derive_blueprint_properties.py\n")
        f.write("# Every `name` satisfies pandemic_hash(name) == key, little-endian.\n")
        f.write("# type_src=decomp+size means the engine's reader helper agrees with the observed\n")
        f.write("# size histogram; type_src=size means only the corpus shape supports it.\n")
        f.write("key\tname\tname_src\ttype\ttype_src\tuses\tsizes\ttop_types\tdispatchers\n")
        for k in sorted(keys, key=lambda x: (-uses[x], x)):
            t, tsrc = typed[k]
            f.write(
                f"{k:08x}\t{names.get(k,'')}\t{source.get(k,'')}\t{t}\t{tsrc}\t{uses[k]}\t"
                f"{','.join(f'{s}x{c}' for s, c in sizes[k].most_common())}\t"
                f"{','.join(t for t, _ in owners[k].most_common(3))}\t"
                f"{','.join(sorted(funcs.get(k, ())))} \n".replace(" \n", "\n")
            )
    print(f"\nwrote {args.out}")

    # ---- generated Rust: sorted tables + binary search in sab_formats ----
    with open(args.rs, "w", encoding="utf-8", newline="\n") as f:
        f.write(
            "//! GENERATED by `tools/derive/derive_blueprint_properties.py` -- do not hand-edit.\n"
            "//!\n"
            "//! Blueprint property names and value types, derived from the retail table, the\n"
            "//! per-class `SetProperty` dispatchers in the decomp, `Saboteur.exe`'s own string\n"
            "//! literals, and the 2008 prototype symbols. See `docs/formats/gametemplates.md`.\n"
            "//!\n"
            "//! Every name here satisfies `pandemic_hash(name) == key` little-endian; the test\n"
            "//! `every_property_name_is_a_preimage` re-checks all of them, so a bad row cannot\n"
            "//! survive `cargo test`.\n\n"
            "use super::PropType;\n\n"
        )
        f.write(
            f"/// {len(names)} of the {len(keys)} property keys in retail have a recovered name,\n"
            f"/// covering {100*named_uses/tot:.1f}% of the {tot} property uses. Sorted by key.\n"
            "pub(super) static NAMES: &[(u32, &str)] = &[\n"
        )
        for k in sorted(names):
            f.write(f'    (0x{k:08X}, "{names[k]}"),\n')
        f.write("];\n\n")
        f.write(
            f"/// Value type for all {len(keys)} keys. {strong} are corroborated by the engine's\n"
            "/// reader helper agreeing with the observed size histogram; the rest rest on the\n"
            "/// corpus shape alone. Sorted by key.\n"
            "pub(super) static TYPES: &[(u32, PropType)] = &[\n"
        )
        for k in sorted(keys):
            f.write(f"    (0x{k:08X}, PropType::{typed[k][0]}),\n")
        f.write("];\n")
    print(f"wrote {args.rs}")


if __name__ == "__main__":
    main()
