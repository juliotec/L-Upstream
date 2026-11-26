//! The Windows game's resources, as `.res` files its link takes: the
//! manifest, Longshot's icon, and the game's name when the builder read it.

use build_core::res;
pub use build_core::res::{ico_images, without_icons};

/// The game's manifest: UTF-8 as the process's code page, the segment heap, and paths past MAX_PATH.
///
/// host/ and gpu/vk keep paths as UTF-8 and call Windows' A functions: without it a folder name the system code page
/// cannot spell came back as "?", and the game found nothing. The segment heap: the shader preparation's workers
/// allocate in the driver from all threads at once, and on the NT heap they queued on its one lock
/// (gpu/vk/prepare_workers.h has the measurements). longPathAware: where Windows' LongPathsEnabled is on, the A functions
/// take a path of more than 260 units (host/hos_paths.h's buffers hold HOS_PATH_CAP bytes).
pub const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Longshot.Game" version="1.0.0.0"/>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
      <heapType xmlns="http://schemas.microsoft.com/SMI/2020/WindowsSettings">SegmentHeap</heapType>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
</assembly>
"#;

/// The manifest's `.res`: the empty entry a `.res` starts with, then RT_MANIFEST 1 (what an exe's loader reads).
pub fn manifest_res() -> Vec<u8> {
    [res::start(0), res::entry(res::RT_MANIFEST, 1, MANIFEST.as_bytes(), 0x1030, res::US_ENGLISH)].concat()
}

/// A `.res` of an icon (group 1, which the window class loads: host/display.c).
pub fn icon_res(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    [res::start(res::US_ENGLISH), res::icon_entries(images)].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_s_records_come_out_and_the_rest_stay() {
        let png = b"\x89PNG\r\n\x1a\nxyz".to_vec();
        let icon = icon_res(&[(16, png.clone()), (256, png)]);
        let empty = res::start(res::US_ENGLISH);
        assert_eq!(without_icons(&icon).unwrap(), empty);
        let named = [icon.clone(), res::entry(16, 1, b"ver", 0x0030, 0x0409)].concat();
        assert_eq!(without_icons(&named).unwrap(), [empty, res::entry(16, 1, b"ver", 0x0030, 0x0409)].concat());
        assert_eq!(manifest_res().len() % 4, 0);
    }
}
