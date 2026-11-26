//! The libraries and headers the build takes from outside the tree: the Vulkan, SPIR-V and VMA headers, FFmpeg, dav1d,
//! NVIDIA's NGX SDK. Each from a CPM package cache (`RX_CPM`: the builder's tools/cpm, build-driver's tools/mod.rs; then
//! the tree's `cpm/`, older checkouts' caches), a developer's `LONGSHOT_*` variable, or the system (`/usr/local`,
//! `/usr`, pkg-config).

use crate::options::Settings;
use std::path::{Path, PathBuf};

/// The CPM package caches, in the order they are looked in.
pub fn cpm_dirs(s: &Settings, root: &Path, work: &Path) -> Vec<PathBuf> {
    let parent = root.parent().unwrap_or(root);
    [
        s.cpm.clone(),
        Some(root.join("cpm")),
        Some(work.join("eg").join(".cache").join("cpm")),
        Some(parent.join("mkw-recomp").join("work").join("eg").join(".cache").join("cpm")),
        Some(parent.join("Eglantine").join(".cache").join("cpm")),
    ]
    .into_iter()
    .flatten()
    .filter(|d| d.is_dir())
    .collect()
}

fn sorted_dir(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The newest version of a package in the first cache that has it.
pub fn cpm_package(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find_map(|root| {
        let d = root.join(name);
        sorted_dir(&d).last().map(|v| d.join(v))
    })
}

/// A header package: (what it is, CPM name, the header that shows it, what it must declare to be new enough).
pub struct HeaderPackage {
    /// Its name in messages.
    pub label: &'static str,
    /// Its CPM package.
    pub cpm: &'static str,
    /// The header under `include/` that shows the package is there.
    pub probe: &'static str,
    /// Text the header must hold: gpu/vk/vk_vma.cpp calls vmaGetAllocationInfo2 (VMA 3.1); a distribution's VMA 3.0
    /// (Ubuntu 24.04's), taken, failed the renderer's build.
    pub needs: Option<&'static str>,
}

/// The renderer's header packages, in [`Settings::header_dirs`]' order.
pub const VK_HEADERS: [HeaderPackage; 3] = [
    HeaderPackage { label: "Vulkan", cpm: "vulkanheaders", probe: "vulkan/vulkan.h", needs: None },
    HeaderPackage { label: "SPIR-V", cpm: "spirv-headers", probe: "spirv/unified1/spirv.h", needs: None },
    HeaderPackage {
        label: "VMA",
        cpm: "vulkanmemoryallocator",
        probe: "vk_mem_alloc.h",
        needs: Some("vmaGetAllocationInfo2"),
    },
];

/// A package root with `include/<probe>`: the CPM cache's, then the variable's (the root or its `include/`), then
/// `/usr/local` and `/usr` (a distribution's vulkan-headers).
pub fn header_package(p: &HeaderPackage, env: Option<&Path>, dirs: &[PathBuf]) -> Option<PathBuf> {
    let ok = |root: &Path| {
        let path = root.join("include").join(p.probe);
        path.is_file()
            && p.needs.is_none_or(|n| std::fs::read(&path).is_ok_and(|b| String::from_utf8_lossy(&b).contains(n)))
    };
    if let Some(pkg) = cpm_package(p.cpm, dirs).filter(|r| ok(r)) {
        return Some(pkg);
    }
    if let Some(e) = env {
        let e = std::path::absolute(e).unwrap_or_else(|_| e.to_path_buf());
        if ok(&e) {
            return Some(e);
        }
        if e.file_name().is_some_and(|n| n == "include")
            && let Some(parent) = e.parent().filter(|p| ok(p))
        {
            return Some(parent.to_path_buf());
        }
    }
    ["/usr/local", "/usr"].iter().map(PathBuf::from).find(|r| ok(r))
}

/// FFmpeg's libraries the game links.
pub const FFMPEG_LIBS: [&str; 4] = ["avcodec", "swscale", "swresample", "avutil"];

/// (Windows) FFmpeg's libraries in `ff`: llvm-mingw's build (`lib/lib<name>.a`, builder/tools/windows-x64/cpm.sh), else an MSVC
/// build's (`lib/<name>.lib`, the CPM cache Hookshot's builder had).
pub fn ffmpeg_windows_libs(ff: &Path) -> Vec<PathBuf> {
    let mingw = ff.join("lib").join("libavcodec.a").is_file();
    FFMPEG_LIBS
        .iter()
        .map(|l| ff.join("lib").join(if mingw { format!("lib{l}.a") } else { format!("{l}.lib") }))
        .collect()
}

/// (Windows) FFmpeg in the CPM caches: a version with `lib/` that is not a Linux build (a cache may have both).
pub fn ffmpeg_windows(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|r| r.join("ffmpeg")).filter(|d| d.is_dir()).find_map(|d| {
        sorted_dir(&d).into_iter().map(|n| d.join(&n)).find(|p| {
            p.join("lib").is_dir()
                && !p.file_name().unwrap_or_default().to_string_lossy().to_lowercase().contains("linux")
        })
    })
}

/// (Linux) FFmpeg's compile and link flags: `LONGSHOT_FFMPEG`'s static libraries (tools.lock's `ffmpeg`, with dav1d
/// for the AV1 movies), else the system's through pkg-config. `Err`: the prerequisites missing, as sentences.
pub fn ffmpeg_linux(s: &Settings) -> Result<(Vec<String>, Vec<String>), Vec<String>> {
    if let Some(pre) = s.ffmpeg.as_ref().filter(|p| p.join("lib").join("libavcodec.a").exists()) {
        let mut missing = Vec::new();
        let mut libs: Vec<String> = Vec::new();
        for l in FFMPEG_LIBS {
            let lib = pre.join("lib").join(format!("lib{l}.a"));
            let head = pre.join("include").join(format!("lib{l}")).join(format!("{l}.h"));
            for p in [&lib, &head] {
                if !p.is_file() {
                    missing.push(format!("FFmpeg: {} (LONGSHOT_FFMPEG's selected static prefix)", p.display()));
                }
            }
            libs.push(lib.display().to_string());
        }
        if !missing.is_empty() {
            return Err(missing);
        }
        let dav1d = pre.join("lib").join("libdav1d.a");
        if dav1d.exists() {
            libs.push(dav1d.display().to_string());
        }
        libs.extend(["-lm".to_string(), "-lpthread".to_string()]);
        return Ok((vec![format!("-I{}", pre.join("include").display())], libs));
    }
    let names: Vec<String> = FFMPEG_LIBS.iter().map(|l| format!("lib{l}")).collect();
    let ask = |what: &str| -> Option<Vec<String>> {
        let out = build_core::child::output(build_core::child::command("pkg-config").arg(what).args(&names)).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect())
    };
    match (ask("--cflags"), ask("--libs")) {
        (Some(c), Some(l)) => Ok((c, l)),
        _ => {
            let absent: Vec<&String> = names
                .iter()
                .filter(|n| {
                    !build_core::child::status(build_core::child::command("pkg-config").args(["--exists", n]))
                        .is_ok_and(|s| s.success())
                })
                .collect();
            let absent = if absent.is_empty() { names.iter().collect() } else { absent };
            Err(vec![format!(
                "FFmpeg: {} (LONGSHOT_FFMPEG with lib/libavcodec.a first, else pkg-config --cflags/--libs for {}; \
                 install the development libraries or set LONGSHOT_FFMPEG).",
                absent.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                names.join(" ")
            )])
        }
    }
}

/// (Windows, llvm-mingw) dav1d in the CPM cache, the AV1 movies' decoder (the cache's FFmpeg has none); `None` on the
/// MSVC route (a MinGW library: Media Foundation decodes them there).
pub fn dav1d_windows(s: &Settings, dirs: &[PathBuf]) -> Option<PathBuf> {
    if !s.windows || s.mingw.is_none() {
        return None;
    }
    cpm_package("dav1d", dirs).filter(|d| d.join("lib/libdav1d.a").exists() && d.join("include/dav1d/dav1d.h").exists())
}

/// NVIDIA's NGX library in the SDK folder for this build's platform: the static one.
pub fn ngx_library(sdk: &Path, linux: bool) -> PathBuf {
    sdk.join("lib").join(if linux { "libnvsdk_ngx.a" } else { "nvsdk_ngx_s.lib" })
}

/// `LONGSHOT_NGX_SDK` as the build takes it: the folder when it has this platform's library; else none, and why.
pub fn ngx_sdk(s: &Settings) -> (Option<PathBuf>, Option<String>) {
    let Some(sdk) = &s.ngx_sdk else { return (None, None) };
    let lib = ngx_library(sdk, !s.windows);
    if lib.is_file() {
        return (Some(sdk.clone()), None);
    }
    let why = format!(
        "LONGSHOT_NGX_SDK={} has no lib/{} (scripts/fetch.py dlss --platform {} fetches it): built without NVIDIA DLSS",
        sdk.display(),
        lib.file_name().unwrap_or_default().to_string_lossy(),
        if s.windows { "windows" } else { "linux" }
    );
    (None, Some(why))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_package_too_old_is_no_package() {
        let d = crate::testdir("packages");
        let cache = d.join("cpm");
        let vma = cache.join("vulkanmemoryallocator").join("3.0");
        std::fs::create_dir_all(vma.join("include")).unwrap();
        std::fs::write(vma.join("include/vk_mem_alloc.h"), "old").unwrap();
        let dirs = vec![cache.clone()];
        assert_eq!(header_package(&VK_HEADERS[2], None, &dirs), None.or(header_package(&VK_HEADERS[2], None, &[])));
        std::fs::write(vma.join("include/vk_mem_alloc.h"), "vmaGetAllocationInfo2").unwrap();
        assert_eq!(header_package(&VK_HEADERS[2], None, &dirs), Some(vma.clone()));
        // (the variable naming the include folder itself)
        let own = d.join("own");
        std::fs::create_dir_all(own.join("include/vulkan")).unwrap();
        std::fs::write(own.join("include/vulkan/vulkan.h"), "").unwrap();
        assert_eq!(header_package(&VK_HEADERS[0], Some(&own.join("include")), &[]), Some(own));
    }
}

#[cfg(test)]
#[path = "packages_tests.rs"]
mod packages_tests;
