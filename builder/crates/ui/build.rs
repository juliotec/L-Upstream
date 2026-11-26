//! The windows (ui/windows.slint) compiled to Rust, the font in it embedded.

fn main() {
    let config = slint_build::CompilerConfiguration::new().embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    if let Err(e) = slint_build::compile_with_config("ui/windows.slint", config) {
        panic!("ui/windows.slint: {e}");
    }
}
