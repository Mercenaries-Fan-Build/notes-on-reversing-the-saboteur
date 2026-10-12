//! `sab_probe editnodes` — read-only inspection of an `EditNodes.pack` (the dynamic-object DB).
//! Parsing/writing lives in `sab_formats::editnodes`; this only asks questions.

use sab_formats::editnodes::{Node, Pack, Root, Value};

pub fn run(args: &[String]) {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    if sub == "lua-params" {
        let Some(retail) = args.get(1) else { return usage() };
        return lua_params(std::path::Path::new(retail));
    }
    let path = args.get(1);
    let rest = &args[args.len().min(2)..];
    let Some(path) = path else { return usage() };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {path}: {e}");
            std::process::exit(1);
        }
    };
    let pack = match Pack::parse(&bytes) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse {path}: {e}");
            std::process::exit(1);
        }
    };
    match sub {
        "info" => info(&pack, &bytes),
        "list" => list(&pack, flag(rest, "--limit").and_then(|s| s.parse().ok()).unwrap_or(40)),
        "tree" => {
            let entry = flag(rest, "--entry").and_then(|s| s.parse::<usize>().ok());
            let depth = flag(rest, "--depth").and_then(|s| s.parse::<usize>().ok()).unwrap_or(usize::MAX);
            tree(&pack, entry, depth);
        }
        _ => usage(),
    }
}

fn usage() {
    eprintln!("sab_probe editnodes <info|list|tree> <EditNodes.pack> [opts]");
    eprintln!("  info                       entry/node/leaf counts, known-tag coverage, round-trip check");
    eprintln!("  list  [--limit N]          per-entry: hash, byte size, object count");
    eprintln!("  tree  [--entry N] [--depth D]   dump the node tree (tag names + decoded values)");
    eprintln!("sab_probe editnodes lua-params <retail_dir>");
    eprintln!("  every leaf tagged pandemic_hash(\"LuaParam\") in the main and DLC packs, decoded with");
    eprintln!("  sab_formats::editnodes::lua_param: counts by type, CrcList items, decode errors");
    eprintln!("note: the main pack is embedded in France/loosefiles_BinPC.pack; DLC slots ship");
    eprintln!("      standalone ones at DLC/NN/France/EditNodes/EditNodes.pack");
}

/// The main pack (inside `France/loosefiles_BinPC.pack`) and every DLC pack, by label.
fn retail_packs(retail: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    use sab_formats::loosefiles;
    let lf = std::fs::read(retail.join("France/loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let name = r"France\EditNodes\EditNodes.pack";
    let e = loosefiles::find(&es, name).unwrap_or_else(|| panic!("{name} not in loosefiles_BinPC.pack"));
    let mut out = vec![("main".to_string(), lf[e.data.clone()].to_vec())];
    let dlc = retail.join("DLC");
    let mut slots: Vec<_> = std::fs::read_dir(&dlc).expect("DLC dir").map(|d| d.expect("DLC entry").path()).collect();
    slots.sort();
    for s in slots {
        let p = s.join("France/EditNodes/EditNodes.pack");
        if p.exists() {
            out.push((p.display().to_string(), std::fs::read(&p).expect("DLC EditNodes.pack")));
        }
    }
    out
}

/// Every leaf in an entry root, depth first.
fn leaves<'a>(r: &'a Root, out: &mut Vec<&'a Node>) {
    fn node<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
        match n {
            Node::Leaf { .. } => out.push(n),
            Node::Container { children, .. } => children.iter().for_each(|c| node(c, out)),
        }
    }
    match r {
        Root::EditNode { objects } => objects.iter().for_each(|o| node(o, out)),
        Root::Container { children, .. } => children.iter().for_each(|c| leaves(c, out)),
    }
}

fn lua_params(retail: &std::path::Path) {
    use sab_formats::editnodes::{lua_param, tags, LuaValue};
    for (label, bytes) in retail_packs(retail) {
        let pack = Pack::parse(&bytes).unwrap_or_else(|e| panic!("{label}: {e}"));
        let mut by_type: std::collections::BTreeMap<&str, usize> = Default::default();
        let (mut total, mut items) = (0usize, 0usize);
        let mut errors: Vec<String> = Vec::new();
        for e in &pack.entries {
            let mut ls = Vec::new();
            leaves(&e.root, &mut ls);
            for n in ls {
                let Node::Leaf { tag, data } = n else { continue };
                if *tag != tags::LUA_PARAM {
                    continue;
                }
                total += 1;
                match lua_param(data) {
                    Ok(p) => {
                        let t = match &p.value {
                            LuaValue::Crc(_) => "LuaCrc",
                            LuaValue::Bool(_) => "LuaBool",
                            LuaValue::Float(_) => "LuaFloat",
                            LuaValue::Int(_) => "LuaInt",
                            LuaValue::Str(_) => "LuaString",
                            LuaValue::CrcList(v) => {
                                items += v.len();
                                "LuaCrcList"
                            }
                        };
                        *by_type.entry(t).or_default() += 1;
                    }
                    Err(err) => errors.push(err),
                }
            }
        }
        println!("{label}: {total} LuaParam leaves, {} decode, {} errors", total - errors.len(), errors.len());
        for (t, n) in &by_type {
            println!("  {t:<12} {n}");
        }
        println!("  LuaCrcList items: {items}");
        for e in errors.iter().take(20) {
            println!("  error: {e}");
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

struct Stat {
    nodes: usize,
    leaves: usize,
    containers: usize,
    named: usize,
    unknown_tags: usize,
}

fn walk_nodes(n: &Node, st: &mut Stat) {
    st.nodes += 1;
    if sab_formats::editnodes::tag_name(n.tag()).is_none() {
        st.unknown_tags += 1;
    }
    match n {
        Node::Leaf { .. } => {
            st.leaves += 1;
            if matches!(n.value(), Some(Value::Lua(_) | Value::BadLua(_))) {
                st.named += 1;
            }
        }
        Node::Container { children, .. } => {
            st.containers += 1;
            for c in children {
                walk_nodes(c, st);
            }
        }
    }
}

fn walk_root(r: &Root, st: &mut Stat) {
    match r {
        Root::EditNode { objects } => objects.iter().for_each(|n| walk_nodes(n, st)),
        Root::Container { children, .. } => children.iter().for_each(|r| walk_root(r, st)),
    }
}

fn info(pack: &Pack, raw: &[u8]) {
    let mut st = Stat { nodes: 0, leaves: 0, containers: 0, named: 0, unknown_tags: 0 };
    for e in &pack.entries {
        walk_root(&e.root, &mut st);
    }
    let bytes = raw.len();
    println!("EditNodes.pack: {} entries, {bytes} bytes", pack.entries.len());
    // Parsing already proves every entry consumes to its declared size; this proves the writer
    // reproduces the input exactly, which is the precondition for any edit-and-repack workflow.
    let re = pack.write();
    if re == raw {
        println!("round-trip: byte-identical ({bytes} bytes)");
    } else {
        let diff = re.iter().zip(raw).position(|(a, b)| a != b);
        println!(
            "round-trip: MISMATCH (wrote {} bytes vs {bytes}; first differing byte at {:?})",
            re.len(),
            diff
        );
    }
    println!(
        "nodes={} (containers={}, leaves={}; LuaParam={})",
        st.nodes, st.containers, st.leaves, st.named
    );
    println!(
        "tags: {} unknown of {} ({} named by the tag dictionary)",
        st.unknown_tags,
        st.nodes,
        st.nodes - st.unknown_tags
    );
}

fn root_obj_count(r: &Root) -> usize {
    match r {
        Root::EditNode { objects } => objects.len(),
        Root::Container { children, .. } => children.len(),
    }
}

fn list(pack: &Pack, limit: usize) {
    for (i, e) in pack.entries.iter().enumerate().take(limit) {
        let kind = match &e.root {
            Root::EditNode { .. } => "EditNode",
            Root::Container { .. } => "Container",
        };
        println!("[{i:>4}] hash={:08x}  {kind}  objects={}", e.hash, root_obj_count(&e.root));
    }
    if pack.entries.len() > limit {
        println!("... ({} more; --limit {} shown)", pack.entries.len() - limit, limit);
    }
}

fn tree(pack: &Pack, entry: Option<usize>, depth: usize) {
    for (i, e) in pack.entries.iter().enumerate() {
        if let Some(want) = entry {
            if want != i {
                continue;
            }
        }
        println!("entry [{i}] hash={:08x}", e.hash);
        print_root(&e.root, 1, depth);
    }
}

fn indent(d: usize) -> String {
    "  ".repeat(d)
}

fn tagname(tag: u32) -> String {
    sab_formats::editnodes::tag_name(tag).map(|s| s.to_string()).unwrap_or_else(|| format!("tag_{tag:08x}"))
}

fn print_root(r: &Root, d: usize, max: usize) {
    if d > max {
        return;
    }
    match r {
        Root::EditNode { objects } => {
            println!("{}EditNode ({} objects)", indent(d), objects.len());
            for n in objects {
                print_node(n, d + 1, max);
            }
        }
        Root::Container { tag, children } => {
            println!("{}{} (container, {} children)", indent(d), tagname(*tag), children.len());
            for c in children {
                print_root(c, d + 1, max);
            }
        }
    }
}

fn print_node(n: &Node, d: usize, max: usize) {
    if d > max {
        return;
    }
    match n {
        Node::Container { tag, children } => {
            println!("{}{} ({} children)", indent(d), tagname(*tag), children.len());
            for c in children {
                print_node(c, d + 1, max);
            }
        }
        Node::Leaf { tag, .. } => {
            println!("{}{} = {}", indent(d), tagname(*tag), fmt_value(n.value()));
        }
    }
}

fn fmt_value(v: Option<Value>) -> String {
    match v {
        Some(Value::Empty) => "<empty>".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::U32(u)) => format!("0x{u:08x} ({u})"),
        Some(Value::Raw64(b)) => format!("raw64 {}", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        Some(Value::Str(s)) => format!("{s:?}"),
        Some(Value::Vec3(v)) => format!("({:.3}, {:.3}, {:.3})", v[0], v[1], v[2]),
        Some(Value::Lua(p)) => format!("LuaParam {:?} = {:?}", p.name, p.value),
        Some(Value::BadLua(e)) => format!("bad LuaParam: {e}"),
        Some(Value::Bytes(n)) => format!("<{n} bytes>"),
        None => "<container>".into(),
    }
}
