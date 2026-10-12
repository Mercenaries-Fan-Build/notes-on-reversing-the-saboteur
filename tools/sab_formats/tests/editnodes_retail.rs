//! Retail gate for `editnodes`: the main `EditNodes.pack` (inside `France/loosefiles_BinPC.pack`) and
//! the DLC one parse, write back byte for byte, and every leaf tagged `tags::LUA_PARAM` decodes with
//! `lua_param`.
//!
//! Game-gated: built by the `retail` feature, reads the install named by the repo-root
//! `.sab-local.toml`, and fails if it is absent.

use std::path::{Path, PathBuf};

use sab_formats::editnodes::{lua_param, tags, LuaValue, Node, Pack, Root};
use sab_formats::game_paths::local_config_retail_dir;
use sab_formats::loosefiles;

fn retail() -> PathBuf {
    local_config_retail_dir(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| panic!("{e}"))
}

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

/// (LuaParam leaves, LuaCrcList items) in a pack that must round-trip.
fn check(label: &str, bytes: &[u8]) -> (usize, usize) {
    let pack = Pack::parse(bytes).unwrap_or_else(|e| panic!("{label}: {e}"));
    assert!(pack.write() == bytes, "{label}: rebuild differs");
    let (mut params, mut items) = (0, 0);
    for e in &pack.entries {
        let mut ls = Vec::new();
        leaves(&e.root, &mut ls);
        for n in ls {
            let Node::Leaf { tag, data } = n else { continue };
            if *tag == tags::LUA_PARAM {
                let p = lua_param(data).unwrap_or_else(|err| panic!("{label} entry 0x{:08x}: {err}", e.hash));
                params += 1;
                if let LuaValue::CrcList(v) = &p.value {
                    items += v.len();
                }
            }
        }
    }
    (params, items)
}

#[test]
fn retail_editnodes_round_trip_and_every_lua_param_decodes() {
    let retail = retail();
    let lf = std::fs::read(retail.join("France/loosefiles_BinPC.pack")).expect("loosefiles_BinPC.pack");
    let es = loosefiles::entries(&lf).expect("loosefiles walk");
    let main = loosefiles::find(&es, r"France\EditNodes\EditNodes.pack").expect("main EditNodes.pack");
    assert_eq!(check("main", &lf[main.data.clone()]), (4548, 775));
    let dlc = std::fs::read(retail.join("DLC/01/France/EditNodes/EditNodes.pack")).expect("DLC EditNodes.pack");
    assert_eq!(check("DLC 01", &dlc), (5, 0));
}
