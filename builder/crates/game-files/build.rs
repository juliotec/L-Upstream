//! `ROMFS_SOURCES`: a hash of the sources that decide what a romfs index holds (recipe_hash.rs names them), in
//! romfs::RECIPE, so an index made by other code is made again without anyone remembering to say so.

mod recipe_hash {
    include!("recipe_hash.rs");
}

fn main() {
    let src = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it")).join("src");
    println!("cargo:rerun-if-changed=recipe_hash.rs");
    for f in recipe_hash::FILES {
        println!("cargo:rerun-if-changed={}", src.join(f).display());
    }
    println!("cargo:rustc-env=ROMFS_SOURCES={}", recipe_hash::hash(&src));
}
