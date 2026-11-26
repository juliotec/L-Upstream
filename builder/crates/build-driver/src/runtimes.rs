//! The upscalers' vendor files a build uses and the game folder gets, which Longshot never commits or ships: NVIDIA's
//! DLSS SDK (the headers and library DLSS's build reads, and DLSS's and DLSS Frame Generation's runtimes, which go in
//! the game folder's `ngx/`), AMD's FSR 4 runtime for Vulkan and Intel's XeSS library (Windows: beside the game). Each
//! file comes from its vendor's GitHub repository at one pinned commit, checked against its SHA-256, and is kept in the
//! per-user cache (`<cache>/runtimes/<sha256>-<name>`) for every later build; nothing is downloaded until the vendor's
//! license is accepted (`--accept-licenses nvidia,amd,intel`, which a launcher passes once the player said yes to the
//! terms [`Vendor::notice`] names). A build for a profile keeps what was accepted in its profile.json (`"licenses"`:
//! each vendor's name and the pin it was accepted for, crate::play::Profile); a build for no profile takes them for
//! that run only. A pin that moves asks again. The pins are the ones the launchers show the licenses of
//! (LongshotLauncher's SettingsStepViewModel.cs names the same commits).
//!
//! A developer's `LONGSHOT_NGX_SDK` is used as it is, and nothing of NVIDIA's is downloaded. FSR 4 on Linux needs no
//! download: the player's own Vulkan FSR 4 library is loaded (gpu/vk/vk_fsr4.cpp); Intel publishes no XeSS library for
//! Linux. A vendor file in the game folder that the builder did not put there (the installed note says which it did,
//! crate::folder) is the player's own, maybe newer: it is left alone with the rest of that vendor's files. One the
//! builder put there under an earlier pin is replaced by this pin's, and goes when a build no longer asks for its
//! upscaler.

use crate::options::Upscaler;
use build_core::report::Reporter;
use build_core::{Error, IoContext, Result, fsx};
use std::path::{Path, PathBuf};

/// A vendor whose license a download needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    /// NVIDIA (DLSS).
    Nvidia,
    /// AMD (FSR 4).
    Amd,
    /// Intel (XeSS).
    Intel,
}

/// NVIDIA's DLSS SDK, v310.9.16.
const DLSS_REPO: (&str, &str) = ("NVIDIA/DLSS", "9981c6350dc58c0ce21f024fd65e31e851c31689");
/// AMD's FidelityFX SDK v2.4.0: FSR 4.1.2.
const FSR_REPO: (&str, &str) = ("GPUOpen-LibrariesAndSDKs/FidelityFX-SDK", "7539ae8b6568ad29ed78a753404351daf3b5cca7");
/// Intel's XeSS, main after v3.0.2: libxess.dll 2.0.2.68 (third_party/xess's headers' commit).
const XESS_REPO: (&str, &str) = ("intel/xess", "de0fb9c1c510661c571164e1418ceca8101dab69");

impl Vendor {
    /// Every vendor.
    pub const ALL: [Vendor; 3] = [Vendor::Nvidia, Vendor::Amd, Vendor::Intel];

    /// The vendor of a name ([`Vendor::name`]), `None` for one this builder does not know.
    pub fn of_name(name: &str) -> Option<Vendor> {
        Vendor::ALL.into_iter().find(|v| v.name() == name)
    }

    /// Its name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Vendor::Nvidia => "nvidia",
            Vendor::Amd => "amd",
            Vendor::Intel => "intel",
        }
    }

    /// `--accept-licenses`' value: vendors' names separated by commas.
    pub fn parse_list(text: &str) -> Result<Vec<Vendor>> {
        let mut out = Vec::new();
        for n in text.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            out.push(match n.to_ascii_lowercase().as_str() {
                "nvidia" => Vendor::Nvidia,
                "amd" => Vendor::Amd,
                "intel" => Vendor::Intel,
                _ => {
                    return Err(Error::player(
                        "bad_license",
                        format!("--accept-licenses: {n:?} is not one of nvidia, amd or intel."),
                    ));
                }
            });
        }
        out.dedup();
        Ok(out)
    }

    /// The pinned commit its files come from: a license accepted for one pin is asked again for the next.
    pub fn pin(self) -> &'static str {
        match self {
            Vendor::Nvidia => DLSS_REPO.1,
            Vendor::Amd => FSR_REPO.1,
            Vendor::Intel => XESS_REPO.1,
        }
    }

    /// The license's page at the pinned commit.
    pub fn license_url(self) -> String {
        let (repo, commit, path) = match self {
            Vendor::Nvidia => (DLSS_REPO.0, DLSS_REPO.1, "LICENSE.txt"),
            Vendor::Amd => (FSR_REPO.0, FSR_REPO.1, "Kits/FidelityFX/docs/license.md"),
            Vendor::Intel => (XESS_REPO.0, XESS_REPO.1, "LICENSE.txt"),
        };
        format!("https://github.com/{repo}/blob/{commit}/{path}")
    }

    /// What the player accepts: the vendor's files, their license.
    pub fn notice(self) -> String {
        let what = match self {
            Vendor::Nvidia => "NVIDIA DLSS needs NVIDIA's DLSS SDK (v310.9.16), under the NVIDIA RTX SDKs license",
            Vendor::Amd => "AMD FSR 4 needs AMD's FSR runtime for Vulkan (FSR SDK v2.4.0), under AMD's FSR SDK license",
            Vendor::Intel => {
                "Intel XeSS needs Intel's XeSS library (2.0.2.68), under the Intel Simplified Software License"
            }
        };
        format!(
            "{what}: {}. The files are the vendor's, not Longshot's; they stay on this PC, in the builder's cache and the \
             game folder. Never share a game folder built with them.",
            self.license_url()
        )
    }
}

/// One file of a vendor's repository: its path there, its SHA-256, its name or path where it goes.
#[derive(Clone, Copy, Debug)]
struct File {
    path: &'static str,
    sha256: &'static str,
    dest: &'static str,
}

const fn f(path: &'static str, sha256: &'static str, dest: &'static str) -> File {
    File { path, sha256, dest }
}

/// What every platform's DLSS build reads: the license and the headers.
const DLSS_COMMON: &[File] = &[
    f("LICENSE.txt", "d4216e39ebef5f9b50a6712ebb37beeb5379862a67733a9999c651f21592aaf0", "LICENSE.txt"),
    f(
        "include/nvsdk_ngx_defs_dlssd.h",
        "2e97fe1595b1334f71b51724f2cb67496e8c296a7a2eec5606da53055882e716",
        "include/nvsdk_ngx_defs_dlssd.h",
    ),
    f(
        "include/nvsdk_ngx_defs_dlssg.h",
        "c29765ab9e0125f9d724ae9811cc2abe03738d17486ec8eee80d78a1504549e1",
        "include/nvsdk_ngx_defs_dlssg.h",
    ),
    f(
        "include/nvsdk_ngx_defs.h",
        "ea23f33497cd274860d1c25a97644fce807dcb0037c594547203343103fad03e",
        "include/nvsdk_ngx_defs.h",
    ),
    f(
        "include/nvsdk_ngx_defs_vk.h",
        "0a24d0861ace7d6b9362a67b7f08bea1b33ea123c5ce730b19133de8a891d031",
        "include/nvsdk_ngx_defs_vk.h",
    ),
    f("include/nvsdk_ngx.h", "dc38e7467cf415379c9d12ae1b6e4a494c453ed92720fb53e92aecb523e7b848", "include/nvsdk_ngx.h"),
    f(
        "include/nvsdk_ngx_helpers_cuda.h",
        "f2851a76107bdf4fbcb7b261f3c35573c312d8ecb4bc18fb02d9e2cec7705042",
        "include/nvsdk_ngx_helpers_cuda.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_d3d.h",
        "ec75224f36ed6580baaf250fa83620405ad81a1d98c2c436654a0c9cf6a6b8ba",
        "include/nvsdk_ngx_helpers_d3d.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssd_cuda.h",
        "7e35cde214d905346a487b9a28c05d35e1c55b480e237fe3de69580a7e332730",
        "include/nvsdk_ngx_helpers_dlssd_cuda.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssd_d3d.h",
        "6df9d02c6f47feee3afc8d12340a9f554a7ddce4ddacb1b6a13a42b26c2c6293",
        "include/nvsdk_ngx_helpers_dlssd_d3d.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssd.h",
        "a25fdda925b479dfe869593aeba311ef08793da54e0391f2fe2fe756cbef5532",
        "include/nvsdk_ngx_helpers_dlssd.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssd_vk.h",
        "11df8fbda532a97fe867c8ef2b40058b49b7898183710f3468e55b1eedb8f40b",
        "include/nvsdk_ngx_helpers_dlssd_vk.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssg_d3d.h",
        "6335ebcb95de39a361e87fe0257d6ce0723e5d8deaa15db3b35afacf2b3f4220",
        "include/nvsdk_ngx_helpers_dlssg_d3d.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssg.h",
        "6881d21419c40c78ec948b326902ece09fc112b0223144d51c4905f3fced7a1f",
        "include/nvsdk_ngx_helpers_dlssg.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_dlssg_vk.h",
        "b371d90b9f4372b9a415e68a0afbdc2f31b7b552e219a9637d848749bc965316",
        "include/nvsdk_ngx_helpers_dlssg_vk.h",
    ),
    f(
        "include/nvsdk_ngx_helpers.h",
        "5bcbadfe7478b802cf6d3aca4dc5ddd7d0889b99726e69c63f9e9bd555f44471",
        "include/nvsdk_ngx_helpers.h",
    ),
    f(
        "include/nvsdk_ngx_helpers_vk.h",
        "c192bff72138f12f770db48e12c1d8f712dfc81ea0e8e055ebb27d8dd1b31623",
        "include/nvsdk_ngx_helpers_vk.h",
    ),
    f(
        "include/nvsdk_ngx_loader.h",
        "abdf5f66a692620538357d29fdf78e3e2a677ee8140b7a638962ecb766585e75",
        "include/nvsdk_ngx_loader.h",
    ),
    f(
        "include/nvsdk_ngx_params_dlssd.h",
        "d41e3ba8f1d72583b55273092c99bf426dc4a0d46de827229e25d5b1325a370e",
        "include/nvsdk_ngx_params_dlssd.h",
    ),
    f(
        "include/nvsdk_ngx_params_dlssg.h",
        "c24fa5aa68ffa2808c828b75cfa32843f1606f80a145f84055ac450d7e6c74ad",
        "include/nvsdk_ngx_params_dlssg.h",
    ),
    f(
        "include/nvsdk_ngx_params.h",
        "943bc8cc5cdae03b6303016fbad3183636f2335ae27a2d18776798c3b4efabbc",
        "include/nvsdk_ngx_params.h",
    ),
    f(
        "include/nvsdk_ngx_standalone_common.h",
        "38413c3338b0aea0202c07da95d426bfee7080a42913823fa2d9dd585b087b53",
        "include/nvsdk_ngx_standalone_common.h",
    ),
    f(
        "include/nvsdk_ngx_standalone_cuda.h",
        "b23436ed5040051ac8e81554c9af080f2463c941435bbc3b82222e5431a09499",
        "include/nvsdk_ngx_standalone_cuda.h",
    ),
    f(
        "include/nvsdk_ngx_vk.h",
        "2d364ce7132881eb669e9498fd570d74cba563b1fbebc82c235f8e1ad2dd8b6d",
        "include/nvsdk_ngx_vk.h",
    ),
];

/// DLSS's per platform: the library the build links (into `lib/`), then DLSS's runtime and DLSS Frame Generation's
/// (into `runtime/` and the game folder's `ngx/`).
const DLSS_LINUX: [File; 3] = [
    f(
        "lib/Linux_x86_64/libnvsdk_ngx.a",
        "d3c50bb0d9624fc35e502a8e929b8fc1e2b505017ebdb84c0d5b627e11a235df",
        "lib/libnvsdk_ngx.a",
    ),
    f(
        "lib/Linux_x86_64/rel/libnvidia-ngx-dlss.so.310.9.16",
        "30982c937afed85c96b8d313323de9a650a2eec6de1bb0f714b21750f4ce98f6",
        "runtime/libnvidia-ngx-dlss.so.310.9.16",
    ),
    f(
        "lib/Linux_x86_64/rel/libnvidia-ngx-dlssg.so.310.9.16",
        "574839cae01c835de4d5845c36ca9cf75361f8160bd52397142c36a24e29e6f7",
        "runtime/libnvidia-ngx-dlssg.so.310.9.16",
    ),
];
const DLSS_WINDOWS: [File; 3] = [
    // (the static NGX loader built against the static C runtime (/MT); llvm-mingw links it with vk_ngx_msvc_rt.cpp)
    f(
        "lib/Windows_x86_64/x64/nvsdk_ngx_s.lib",
        "7b9a5239c1145af5f645ab472aa066f9ea79db36ac819562312aae69d65f3765",
        "lib/nvsdk_ngx_s.lib",
    ),
    f(
        "lib/Windows_x86_64/rel/nvngx_dlss.dll",
        "d00b5b0ab29c3ca101b70c3a5ecb3a01b7ef33766a3383a28a18623083e6123e",
        "runtime/nvngx_dlss.dll",
    ),
    f(
        "lib/Windows_x86_64/rel/nvngx_dlssg.dll",
        "8d59d49b36410dd6b62fef2503a5871d347415bd85d943fa0848563a95dd50c7",
        "runtime/nvngx_dlssg.dll",
    ),
];

/// AMD's FSR 4 runtime for Vulkan (Windows), beside the game.
const FSR_FILES: &[File] = &[
    f(
        "Kits/FidelityFX/signedbin/amd_fidelityfx_loader_vk.dll",
        "38a56a01fb3565c0757d9f5c1e4e69de4b65e5f2e5769b323b200dc1f142bab9",
        "amd_fidelityfx_loader_vk.dll",
    ),
    f(
        "Kits/FidelityFX/signedbin/amd_fidelityfx_upscaler_vk.dll",
        "4a7e895af6e0b615d40a84d59eb8c2b1054ad91cdd0c739567940270d51bdab7",
        "amd_fidelityfx_upscaler_vk.dll",
    ),
    f(
        "Kits/FidelityFX/docs/license.md",
        "32357259b965d6a8001e9e508ba60eda6034027c5923a7d226ffbaff813c2bfb",
        "LICENSE-AMD-FSR-SDK.md",
    ),
];

/// Intel's XeSS library (Windows), beside the game.
const XESS_FILES: &[File] = &[
    f("bin/libxess.dll", "251659dd84a3e84de67c886a4186e01f3eca49b00641906fe38bb6b807e5d5b7", "libxess.dll"),
    f("LICENSE.txt", "784be42c39a4a4d03cf85d88e15e59d75828fca773ce45b8dbfffbd7eaf208df", "LICENSE-INTEL-XESS-SDK.txt"),
    f(
        "third-party-programs.txt",
        "e8d8899dbfc4de0e8ca25560be9358e03c1405d7c956d20a1838c3c709b9b51c",
        "THIRD-PARTY-INTEL-XESS-SDK.txt",
    ),
];

/// What the upscalers asked for need: the vendors whose files are downloaded.
pub fn vendors(upscalers: &[Upscaler], windows: bool, own_ngx_sdk: bool) -> Vec<Vendor> {
    let mut v = Vec::new();
    if upscalers.contains(&Upscaler::Dlss) && !own_ngx_sdk {
        v.push(Vendor::Nvidia);
    }
    if windows && upscalers.contains(&Upscaler::Fsr4) {
        v.push(Vendor::Amd);
    }
    if windows && upscalers.contains(&Upscaler::Xess) {
        v.push(Vendor::Intel);
    }
    v
}

/// A refusal: a vendor's files are needed and its license was not accepted.
pub fn check_accepted(needed: &[Vendor], accepted: &[Vendor]) -> Result<()> {
    match needed.iter().find(|v| !accepted.contains(v)) {
        None => Ok(()),
        Some(v) => Err(Error::player(
            "license_not_accepted",
            format!(
                "{} Accept the license to build with it (--accept-licenses {}), or build without it.",
                v.notice(),
                v.name()
            ),
        )),
    }
}

/// The mark a build that reads a DLSS SDK folder leaves in it (its time: when the folder was last read, for the
/// cache's budget, crate::storage).
const SDK_USED: &str = ".used";

/// The runtimes fetched for a build.
#[derive(Clone, Debug, Default)]
pub struct Fetched {
    /// The DLSS SDK folder the compile reads (`LONGSHOT_NGX_SDK`'s place), when one was made.
    pub ngx_sdk: Option<PathBuf>,
    /// The files the game folder gets: (cached file, where in the game folder).
    pub files: Vec<(PathBuf, PathBuf)>,
    /// The game folder files of the vendors' that the player put there and are left alone.
    pub kept: Vec<String>,
    /// The SHA-256 each of `files`' game folder copies must have (checked on the copy itself before it is swapped in:
    /// [`crate::folder::install`]).
    pub pins: Vec<(PathBuf, &'static str)>,
}

/// A file's SHA-256, streamed (the DLSS runtime is 60 MB), as hex.
fn sha256_of(path: &Path) -> Option<String> {
    crate::sha256_file(path).ok().map(|(sha, _)| sha)
}

/// curl's options: no .curlrc read (`-q`, first), the URL taken as it is (`--globoff`), HTTPS alone and on every
/// redirect, a few retries, and a download that stalls (under 1 KB/s for a minute) given up, not waited on for ever.
pub(crate) const CURL: [&str; 15] = [
    "-q",
    "--globoff",
    "--proto",
    "=https",
    "--proto-redir",
    "=https",
    "-fsSL",
    "--retry",
    "3",
    "--connect-timeout",
    "60",
    "--speed-limit",
    "1024",
    "--speed-time",
    "60",
];

/// The part of [`CURL`] every curl the builder runs starts with: no .curlrc, the URL as it is, HTTPS alone and on
/// every redirect (the store's asks and uploads add their own time limits; perf.rs's POST, which may go to this PC
/// over http, has its own).
pub(crate) const CURL_SAFE: &[&str] = CURL.split_at(6).0;

/// wget's options: no .wgetrc read, HTTPS alone, a few tries, each read bounded (a stalled download ends).
const WGET: [&str; 5] = ["--no-config", "--https-only", "-q", "--tries=3", "--timeout=60"];

/// The downloader `name` the builder runs: Windows' own curl (`%SystemRoot%\System32\curl.exe`, by its full path:
/// never one beside the program or in the folder it runs in); elsewhere the first on the PATH, its relative and empty
/// entries skipped, and none in `avoid` (the program's folder, the game folder: a file there is not the system's).
pub(crate) fn downloader(name: &str, path: Option<std::ffi::OsString>, avoid: &[&Path]) -> Option<PathBuf> {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from)?;
        return Some(root.join("System32").join(format!("{name}.exe"))).filter(|p| p.is_file());
    }
    let same =
        |a: &Path, b: &Path| std::fs::canonicalize(a).ok().is_some_and(|a| Some(a) == std::fs::canonicalize(b).ok());
    std::env::split_paths(&path?)
        .filter(|d| d.is_absolute())
        .filter(|d| !avoid.iter().any(|a| same(d, a)))
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// One download of `url` into `to`: curl (every Windows 10 and 11 has it, as most Linux systems), else wget; neither
/// from a folder of `avoid` ([`downloader`]).
fn download(url: &str, to: &Path, avoid: &[&Path]) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let mut avoid: Vec<&Path> = avoid.to_vec();
    if let Some(e) = &exe {
        avoid.push(e);
    }
    let path = std::env::var_os("PATH");
    if let Some(curl) = downloader("curl", path.clone(), &avoid) {
        return match build_core::child::status(build_core::child::command(curl).args(CURL).arg("-o").arg(to).arg(url)) {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("curl failed ({s})")),
            Err(e) => Err(format!("curl did not start ({e})")),
        };
    }
    let Some(wget) = downloader("wget", path, &avoid) else { return Err("neither curl nor wget is on this PC".into()) };
    match build_core::child::status(build_core::child::command(wget).args(WGET).arg("-O").arg(to).arg(url)) {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("wget failed ({s})")),
        Err(e) => Err(format!("wget did not start ({e})")),
    }
}

/// The file in the cache (`<dir>/<sha256>-<its name>`), downloaded and checked when it is not there.
fn cached(dir: &Path, repo: (&str, &str), file: &File, out: &Path, r: &Reporter) -> Result<PathBuf> {
    let name = file.path.rsplit('/').next().unwrap_or(file.path);
    let at = dir.join(format!("{}-{name}", file.sha256));
    if at.is_file() && sha256_of(&at).as_deref() == Some(file.sha256) {
        crate::storage::mark_used(&at); // (its time: its last use, for the cache's budget)
        return Ok(at);
    }
    std::fs::create_dir_all(dir).at(dir)?;
    let url = format!("https://raw.githubusercontent.com/{}/{}/{}", repo.0, repo.1, file.path);
    r.log(format!("downloading {url}"));
    // (a name of this call's own: two builds downloading the same file never write one file)
    let part = fsx::temporary(&at);
    let failed = |why: String| {
        let _ = std::fs::remove_file(&part);
        Error::player(
            "download_failed",
            format!(
                "{} could not be downloaded ({why}). Check the connection and build again, or build without that \
                 upscaler.",
                url
            ),
        )
    };
    download(&url, &part, &[out]).map_err(failed)?;
    match sha256_of(&part) {
        Some(s) if s == file.sha256 => {}
        got => {
            return Err(failed(format!("its SHA-256 is {}, not the pinned {}", got.unwrap_or_default(), file.sha256)));
        }
    }
    fsx::rename(&part, &at).at(&at)?;
    Ok(at)
}

/// Where a DLSS runtime goes in the game folder.
fn dlss_dest(f: &File) -> PathBuf {
    Path::new("ngx").join(f.dest.trim_start_matches("runtime/"))
}

/// The vendors' files of the pins before this builder's: (their name in the game folder, SHA-256). A file there with
/// one of these, or with this pin's, is one a builder put there, even with no installed note to say so (the note
/// unwritten, a crash before it): the next pin's replaces it. None yet: each vendor's pin is the first.
const EARLIER_PINS: &[(&str, &str)] = &[];

/// Every vendor file this builder puts in the game folder: (its name there, its SHA-256).
fn pinned_files() -> impl Iterator<Item = (String, &'static str)> {
    let dlss = DLSS_WINDOWS[1..].iter().chain(&DLSS_LINUX[1..]).map(|f| (dlss_dest(f), f.sha256));
    let rest = FSR_FILES.iter().chain(XESS_FILES).map(|f| (PathBuf::from(f.dest), f.sha256));
    dlss.chain(rest).map(|(p, h)| (p.to_string_lossy().replace('\\', "/"), h))
}

/// The files of `dsts` in the game folder that are the player's own: there, not put there by the builder (the
/// installed note, read once), and not a pinned file of this builder's or an earlier one's (`earlier`).
fn players_with(out: &Path, dsts: impl Iterator<Item = PathBuf>, earlier: &[(&str, &str)]) -> Vec<String> {
    let rel = |d: &Path| d.strip_prefix(out).unwrap_or(d).to_string_lossy().replace('\\', "/");
    crate::folder::ours_of(out, dsts)
        .into_iter()
        .filter(|(d, ours)| d.is_file() && !ours)
        .filter(|(d, _)| {
            let (name, Some(sha)) = (rel(d), sha256_of(d)) else { return true };
            !pinned_files().any(|(n, h)| n == name && h == sha) && !earlier.iter().any(|&(n, h)| n == name && h == sha)
        })
        .map(|(d, _)| rel(&d))
        .collect()
}

fn players(out: &Path, dsts: impl Iterator<Item = PathBuf>) -> Vec<String> {
    players_with(out, dsts, EARLIER_PINS)
}

/// A vendor's files for the game folder taken (with the SHA-256 each must have there), unless one of them is the
/// player's (then all are left as they are).
fn take_or_keep(out: &Path, files: Vec<(PathBuf, PathBuf, &'static str)>, got: &mut Fetched) {
    let theirs = players(out, files.iter().map(|f| f.1.clone()));
    if theirs.is_empty() {
        for (src, dst, sha) in files {
            got.pins.push((dst.clone(), sha));
            got.files.push((src, dst));
        }
    } else {
        got.kept.extend(theirs);
    }
}

/// Every place in the game folder the builder puts a vendor's file for `upscaler` (on this platform).
pub fn game_folder_files(upscaler: Upscaler, windows: bool, out: &Path) -> Vec<PathBuf> {
    match upscaler {
        Upscaler::Dlss => {
            let platform: &[File] = if windows { &DLSS_WINDOWS } else { &DLSS_LINUX };
            platform[1..].iter().map(|f| out.join(dlss_dest(f))).collect()
        }
        Upscaler::Fsr4 if windows => FSR_FILES.iter().map(|f| out.join(f.dest)).collect(),
        Upscaler::Xess if windows => XESS_FILES.iter().map(|f| out.join(f.dest)).collect(),
        _ => Vec::new(),
    }
}

/// The files the upscalers need, fetched into `cache` (`<cache>/runtimes`): the DLSS SDK put together for the compile
/// (unless the developer's `own_ngx_sdk` is used), and the game folder's files. The vendors' licenses must have been
/// accepted ([`check_accepted`]).
pub fn fetch(
    upscalers: &[Upscaler],
    windows: bool,
    own_ngx_sdk: Option<&Path>,
    cache: &Path,
    out: &Path,
    r: &Reporter,
) -> Result<Fetched> {
    let dir = cache.join("runtimes");
    let mut got = Fetched::default();
    if upscalers.contains(&Upscaler::Dlss) {
        let platform: &[File] = if windows { &DLSS_WINDOWS } else { &DLSS_LINUX };
        let sdk = match own_ngx_sdk {
            Some(s) => s.to_path_buf(),
            None => {
                // (the SDK folder the compile reads: the cached files, linked into the layout LONGSHOT_NGX_SDK has)
                let sdk = dir.join(format!("ngx-{}-{}", if windows { "windows" } else { "linux" }, &DLSS_REPO.1[..12]));
                for file in DLSS_COMMON.iter().chain(platform) {
                    let from = cached(&dir, DLSS_REPO, file, out, r)?;
                    let to = sdk.join(file.dest);
                    if sha256_of(&to).as_deref() != Some(file.sha256) {
                        fsx::link_or_copy(&from, &to)?;
                    }
                }
                // (its time of use, for another release's prune: a folder this build reads is not old)
                let _ = fsx::write_atomic(&sdk.join(SDK_USED), b"");
                got.ngx_sdk = Some(sdk.clone());
                sdk
            }
        };
        // (DLSS's runtimes in the game folder's ngx/, where gpu/vk/vk_dlss.cpp looks first)
        let placed: Vec<(PathBuf, PathBuf, &'static str)> = platform[1..]
            .iter()
            .map(|f| (sdk.join(f.dest), out.join(dlss_dest(f)), f.sha256))
            .filter(|(f, _, _)| f.is_file())
            .collect();
        take_or_keep(out, placed, &mut got);
    }
    for (asked, repo, files) in [(Upscaler::Fsr4, FSR_REPO, FSR_FILES), (Upscaler::Xess, XESS_REPO, XESS_FILES)] {
        if !windows || !upscalers.contains(&asked) {
            continue;
        }
        // (the player's own files there: none downloaded)
        let theirs = players(out, files.iter().map(|f| out.join(f.dest)));
        if !theirs.is_empty() {
            got.kept.extend(theirs);
            continue;
        }
        for file in files {
            let dst = out.join(file.dest);
            got.pins.push((dst.clone(), file.sha256));
            got.files.push((cached(&dir, repo, file, out, r)?, dst));
        }
    }
    for k in &got.kept {
        r.log(format!("{k} in the game folder is not the pinned one: left as it is (delete it to get the pinned one)"));
    }
    Ok(got)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_file_is_pinned() {
        for file in DLSS_COMMON.iter().chain(&DLSS_LINUX).chain(&DLSS_WINDOWS).chain(FSR_FILES).chain(XESS_FILES) {
            assert_eq!(file.sha256.len(), 64, "{}", file.path);
            assert!(file.sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()), "{}", file.path);
        }
        for (_, c) in [DLSS_REPO, FSR_REPO, XESS_REPO] {
            assert_eq!(c.len(), 40);
        }
    }

    #[test]
    fn the_fsr_4_library_a_build_fetches_is_one_whose_models_the_renderer_selects() {
        // (main's #2331: the renderer's FSR 4 model row is on only for an upscaler library whose SHA-256 is a row of
        // gpu/vk/fsr4_models.cpp's kKnown, the one whose private layout was read; a pin moved without a row there gives
        // players Auto only)
        let known =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gpu/vk/fsr4_models.cpp"))
                .expect("gpu/vk/fsr4_models.cpp");
        let table = &known[known.find("const KnownLibrary kKnown[] = {").expect("kKnown")..];
        let table = &table[..table.find("};").unwrap()];
        let upscaler = FSR_FILES.iter().find(|f| f.dest == "amd_fidelityfx_upscaler_vk.dll").unwrap();
        assert!(table.contains(&format!("\"{}\"", upscaler.sha256)), "{} is not in kKnown:\n{table}", upscaler.sha256);
    }

    #[test]
    fn what_needs_which_license() {
        let all = [Upscaler::Dlss, Upscaler::Fsr4, Upscaler::Xess];
        assert_eq!(vendors(&all, true, false), [Vendor::Nvidia, Vendor::Amd, Vendor::Intel]);
        assert_eq!(vendors(&all, false, false), [Vendor::Nvidia], "Linux: the player's own FSR 4, no XeSS library");
        assert_eq!(vendors(&all, false, true), [], "a developer's own NGX SDK");
        assert!(check_accepted(&[Vendor::Nvidia], &[]).is_err());
        assert!(check_accepted(&[Vendor::Nvidia], &[Vendor::Nvidia]).is_ok());
        assert_eq!(Vendor::parse_list("nvidia, AMD").unwrap(), [Vendor::Nvidia, Vendor::Amd]);
        assert!(Vendor::parse_list("acme").is_err());
    }

    #[test]
    fn the_builder_s_own_dll_of_an_earlier_pin_is_replaced_and_the_player_s_kept() {
        let d = crate::scratch("runtimes-pin");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        let r = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        // (each XeSS file installed by the builder under an earlier pin: other bytes than this pin's)
        let dsts = game_folder_files(Upscaler::Xess, true, &out);
        let files: Vec<(PathBuf, PathBuf)> = dsts
            .iter()
            .map(|dst| {
                let f = src.join(dst.file_name().unwrap());
                std::fs::write(&f, b"an earlier pin's").unwrap();
                (f, dst.clone())
            })
            .collect();
        crate::folder::install(&out, &files, &[], &[]).unwrap();
        // (cached already, so nothing is downloaded: the test sees which files the build would install)
        let cache = d.join("cache");
        for file in XESS_FILES {
            let name = file.path.rsplit('/').next().unwrap();
            let at = cache.join("runtimes").join(format!("{}-{name}", file.sha256));
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(&at, b"x").unwrap();
        }
        let theirs = players(&out, dsts.iter().cloned());
        assert!(theirs.is_empty(), "the builder's own earlier files are not the player's: {theirs:?}");
        std::fs::write(&dsts[0], b"the player's own").unwrap();
        assert_eq!(players(&out, dsts.iter().cloned()).len(), 1);
        let got = fetch(&[Upscaler::Xess], true, None, &cache, &out, &r);
        assert!(got.is_ok_and(|g| g.files.is_empty() && g.kept.len() == 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_pinned_file_with_no_note_is_the_builder_s_and_any_other_the_player_s() {
        let d = crate::scratch("runtimes-earlier");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let dst = out.join("libxess.dll");
        std::fs::write(&dst, b"a builder's, of an earlier pin").unwrap();
        let sha = sha256_of(&dst).unwrap();
        let earlier = [("libxess.dll", sha.as_str())];
        // (no installed note: the earlier pin's hash says it is the builder's; without the table it is the player's)
        assert!(players_with(&out, [dst.clone()].into_iter(), &earlier).is_empty());
        assert_eq!(players_with(&out, [dst.clone()].into_iter(), &[]), ["libxess.dll"]);
        std::fs::write(&dst, b"the player's own").unwrap();
        assert_eq!(players_with(&out, [dst].into_iter(), &earlier), ["libxess.dll"]);
        assert!(pinned_files().any(|(n, h)| n == "libxess.dll" && h == XESS_FILES[0].sha256));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_downloader_beside_the_program_or_in_the_game_folder_is_not_run() {
        if cfg!(windows) {
            return;
        }
        let d = crate::scratch("runtimes-curl");
        let (game, sys) = (d.join("game"), d.join("sys"));
        for dir in [&game, &sys] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("curl"), b"").unwrap();
        }
        let path = std::env::join_paths([Path::new("relative"), &game, &sys]).unwrap();
        assert_eq!(downloader("curl", Some(path.clone()), &[&game]), Some(sys.join("curl")));
        assert_eq!(downloader("curl", Some(path), &[&game, &sys]), None);
        assert!(CURL[0] == "-q" && CURL.contains(&"--globoff") && CURL.windows(2).any(|w| w == ["--proto", "=https"]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_players_own_dll_is_left_alone() {
        let d = crate::scratch("runtimes");
        std::fs::write(d.join("libxess.dll"), b"the player's own").unwrap();
        let r = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        let got = fetch(&[Upscaler::Xess], true, None, &d.join("cache"), &d, &r).unwrap();
        assert_eq!(got.kept, ["libxess.dll"]);
        assert!(got.files.is_empty(), "nothing downloaded or copied");
        let _ = std::fs::remove_dir_all(&d);
    }
}
