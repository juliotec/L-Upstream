//! The source fingerprint and the shader stamp of a tree unpacked in a deep folder, its files past Windows' 260
//! characters (MAX_PATH): a portable launcher in Downloads put the builder's tools 176 characters deep (#1115). Both
//! hash each file's path below their root, so the same tree gives the same answer at any depth, and no file past 260 is
//! left out (a source left out of the stamp would drop every player's shader caches). On Windows with the long-paths
//! setting off this is the case that failed; elsewhere it checks that the depth does not matter.

use build_compile::generated::{shader_stamp, version};
use std::path::{Path, PathBuf};

const MAX_PATH: usize = 260;
const DEEP: &str = "gpu/dddddddddddddddddddddddddddddddddddddddd/dddddddddddddddddddddddddddddddddddddddd/b.cpp";

fn tree(root: &Path) -> Vec<PathBuf> {
    let deep_stamped = format!("gpu/vk/eden/{}/emit_spirv_folder.cpp", vec!["d".repeat(40); 3].join("/"));
    let rels = [
        "host/a.c",
        "gpu/third_party/suyu/shader_recompiler/frontend/maxwell/indirect_branch_table_track.cpp",
        DEEP,
        &deep_stamped,
        "gpu/vk/eden/shader_recompiler/frontend/maxwell/translate/impl/floating_point_conversion_integer.cpp",
        "gpu/vk/sirit/src/sirit.cpp",
        "gpu/vk/compat/compat.h",
        "gpu/vk/vk_shader.cpp",
        "gpu/vk/vk_bank.cpp",
        "gpu/vk/vk_state.h",
    ];
    rels.iter()
        .map(|rel| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, format!("int f() {{ return {}; }}\n", rel.len())).unwrap();
            p
        })
        .collect()
}

fn root(name: &str, depth: usize) -> PathBuf {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR")).join("long_paths").join(name);
    let _ = std::fs::remove_dir_all(&base);
    let mut r = base;
    while r.as_os_str().len() < depth {
        r = r.join("p".repeat(50));
    }
    std::fs::create_dir_all(&r).unwrap();
    r
}

#[test]
fn a_deep_tree_has_the_shallow_tree_s_fingerprint_and_stamp() {
    let shallow = root("shallow", 0);
    let deep = root("deep", MAX_PATH);
    tree(&shallow);
    let files = tree(&deep);
    assert!(files.iter().all(|f| f.as_os_str().len() > MAX_PATH), "every file is past MAX_PATH");

    assert_eq!(version::fingerprint(&deep), version::fingerprint(&shallow));
    let (vk_s, vk_d) = (shallow.join("gpu/vk"), deep.join("gpu/vk"));
    assert_eq!(shader_stamp::files(&vk_d).len(), 7, "no stamped file is left out");
    assert_eq!(shader_stamp::stamp(&vk_d).unwrap(), shader_stamp::stamp(&vk_s).unwrap());

    std::fs::write(deep.join(DEEP), "changed\n").unwrap();
    assert_ne!(version::fingerprint(&deep), version::fingerprint(&shallow), "a source past 260 is read");
}
