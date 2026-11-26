//! The VMA the build takes is new enough for the renderer: what gpu/vk/vk_vma.cpp calls of VMA 3.1 is what
//! [`VK_HEADERS`]' VMA needs (tests/test_target_reuse_memory.py checks the renderer's side).

use super::*;

#[test]
fn the_vma_taken_declares_what_the_renderer_calls() {
    let vma =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gpu/vk/vk_vma.cpp")).unwrap();
    let package = VK_HEADERS.iter().find(|p| p.label == "VMA").unwrap();
    if vma.contains("vmaGetAllocationInfo2(") {
        assert_eq!(package.needs, Some("vmaGetAllocationInfo2"), "a VMA 3.0 would fail the renderer's build");
    }
}
