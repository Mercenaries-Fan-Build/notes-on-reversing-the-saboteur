# 2008 hash namespace — chunked packlog blocks

**Status:** finding captured, awaiting merge of the parallel GUI + non-SBL4 worktrees before the ~5-line fix lands in `sources/prototype2008.rs`.

## The mismatch

`Prototype2008Source::open` sanity-checks that `pandemic_hash(packlog_stem) == block_hash_on_disk`, and reports ~161/200 sampled packlogs match on the current extract. The remaining ~20% were flagged as a mystery — "some entries may use a different scheme".

## Root cause

They use the exact same scheme. The mismatched packlogs are **chunk splits**: an asset too big for one `.Pack` gets emitted as `Barge0.Packlog` + `Barge1.Packlog` (etc.), and both packlogs record **the same block hash** — the hash of the base name `Barge`, without the trailing chunk digit.

Verified over every mismatch class in the extract:

| Packlog stem | on-disk block hash | `pandemic_hash(base)` | base name |
|---|---:|---:|---|
| `Barge0` / `Barge1` | -1066224316 | -1066224316 | `Barge` |
| `ArenesDeLutece0` / `…1` | -1080910608 | -1080910608 | `ArenesDeLutece` |
| `Bases_Statues0` / `…1` | -2104142233 | -2104142233 | `Bases_Statues` |
| `Bastile0` / `Bastile1` | 1169078689 | 1169078689 | `Bastile` |
| `BD_Alsace_A0` / `…1` | 672689901 | 672689901 | `BD_Alsace_A` |

Both chunks in a pair share the same base-name hash, because they're chunks of the ONE logical asset the engine looks up.

## The fix (apply after worktrees merge)

In `Prototype2008Source::open` where `block_name` is derived from the packlog stem:

```rust
// Chunk splits: `Barge0`/`Barge1` etc. share ONE block hash keyed on the base
// name (without trailing digits). If the stem itself hashes to the block hash,
// use it as-is; otherwise strip trailing ASCII digits and try again.
let stem = pl.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
let block_name = {
    // Peek the first packlog entry's hash to know what we should be matching.
    let first_hash = /* first entry.hash from parse_packlog */;
    if pandemic_hash(&stem) == first_hash {
        stem
    } else {
        let trimmed = stem.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
        if pandemic_hash(&trimmed) == first_hash { trimmed } else { stem }
    }
};
```

Where the raw stem hash matches, nothing changes. Where a chunk suffix broke the match, the stripped base name is used — so the `hash_names` inventory keys the shared block-hash to the shared logical name.

Expected effect on the fingerprint: `hash scheme confirmed IDENTICAL to retail: pandemic_hash matches all 200/200 sampled block names` (up from 161/200 partial).

## Also useful downstream

The base-name mapping is what the GUI browser will want to display — `Barge` (one row) is more useful than `Barge0` + `Barge1` (two rows for what a modder thinks of as one asset). Consider merging chunk twins into one browser entry.
