//! `longshot-builder doctor [--profile <dir>] [--output text|json|user]`: what on this PC could stop Longshot building
//! or running, each check with a plain fix (docs/dev/reference/build.md, "Doctor"). It reads; it changes no setting
//! (the folders' check writes one empty file in each and removes it).
//!
//! - `vulkan`, `gpu`, `driver`: the game's own start rule (gpu/vk/vk_main.cpp `create_instance`, `pick_device`): a
//!   Vulkan 1.3 loader and a GPU whose driver offers 1.3, the one the game picks (a discrete GPU, then an integrated
//!   one, the newer driver of two). Asked in a process of its own (`doctor --vulkan-probe`, build_core::vulkan), so a
//!   driver that crashes or hangs is said. No GPU listed: on Linux, why, as the game's own log line says it
//!   (gpu/vk/compat/longshot_no_gpu.h: NVIDIA's module not loaded, a real-time kernel). The tree keeps no list of
//!   drivers the game refuses (gpu/vk/device_policy.h holds workarounds by driver, not refusals), so none is checked.
//! - `disk_space`: the room a build needs where the cache, the game folder and the profile are, by drive (build.md's
//!   "Disk space").
//! - `game_copy`: the profile's copy there, readable, the supported version (`check`'s answer).
//! - `folders`: the cache, the game folder and the profile writable.
//! - `long_paths` (Windows): `LongPathsEnabled`, and how near the deepest path a build writes comes to 260 characters.
//! - `defender` (Windows): Defender's real-time protection on the cache, read only: builds write thousands of files.
//! - `game_running`: the profile held by a play (single.rs's lock).

use build_core::report::{DoctorCheck, Event, Reporter};
use build_core::vulkan::{self, Gpu, Vulkan};
use std::path::{Path, PathBuf};

/// The hidden option of the process that asks Vulkan ([`ask_vulkan`]).
pub const PROBE_ARG: &str = "--vulkan-probe";

const GB: u64 = 1 << 30;
/// The deepest path a build writes under the cache, past the cache's own, in characters: the tree's deepest file
/// (`third_party/fidelityfx-sdk/include/FidelityFX/gpu/frameinterpolation/` and a header of 77 characters, 141 in all)
/// unpacked under `sources/<16 hex>/`, with some room; the work folder's deepest (`work/<name>-<hash>/build/obj/...`)
/// is 138.
pub const DEEPEST_IN_CACHE: usize = 170;
/// Windows' path limit without long paths (MAX_PATH, the NUL included).
const MAX_PATH: usize = 260;

fn check(id: &'static str, status: &'static str, text: impl Into<String>, fix: impl Into<String>) -> DoctorCheck {
    DoctorCheck { id, status, text: text.into(), fix: fix.into() }
}

// ---- Vulkan

/// What the Vulkan probe's process said.
#[derive(Debug, PartialEq)]
pub enum Probe {
    /// It answered.
    Answered(Vulkan),
    /// It crashed or hung (why), or could not be started.
    Failed(String),
}

/// Why Linux's Vulkan lists no GPU, as the game's start says it (gpu/vk/compat/longshot_no_gpu.h, its causes ported):
/// the graphics cards' PCI ids, and NVIDIA's module and a real-time kernel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoGpu {
    /// `unknown`, `nvidia-rt`, `nvidia-unloaded` or `nvidia-vulkan` (the header's keys).
    pub cause: &'static str,
    /// The display controllers' `vendor:device`, space-separated.
    pub cards: String,
}

/// [`NoGpu`] read under `root` (`/` for this PC; a test's folder holding `sys/` and `proc/`), as the header reads it.
pub fn no_gpu(root: &Path) -> NoGpu {
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).ok().map(|t| t.trim().to_string());
    let realtime = read("sys/kernel/realtime").is_some_and(|t| t.starts_with('1'))
        || read("proc/sys/kernel/version").is_some_and(|t| t.contains("PREEMPT_RT"));
    let nvidia_loaded = root.join("sys/module/nvidia").is_dir();
    let (mut nvidia, mut other, mut incomplete, mut cards) = (false, false, false, Vec::new());
    let mut devices: Vec<PathBuf> = std::fs::read_dir(root.join("sys/bus/pci/devices"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    devices.sort();
    for d in devices {
        let one = |f: &str| std::fs::read_to_string(d.join(f)).ok().map(|t| t.trim().to_string());
        let Some(class) = one("class") else {
            incomplete = true;
            continue;
        };
        if !class.starts_with("0x03") {
            continue;
        }
        let (Some(vendor), Some(device)) = (one("vendor"), one("device")) else {
            incomplete = true;
            continue;
        };
        let (Some(v), Some(dv)) = (vendor.strip_prefix("0x"), device.strip_prefix("0x")) else {
            incomplete = true;
            continue;
        };
        if v == "10de" {
            nvidia = true
        } else {
            other = true
        }
        cards.push(format!("{v}:{dv}"));
    }
    let cause = match (nvidia && !other && !incomplete, nvidia_loaded, realtime) {
        (false, ..) => "unknown",
        (true, true, _) => "nvidia-vulkan",
        (true, false, true) => "nvidia-rt",
        (true, false, false) => "nvidia-unloaded",
    };
    NoGpu { cause, cards: cards.join(" ") }
}

/// The GPU the game picks among `gpus` ([`vulkan`] 1.3 ones only): a discrete one, else an integrated one, else any;
/// of two alike, the newer driver (vk_main.cpp `pick_device`).
pub fn picked(gpus: &[Gpu]) -> Option<&Gpu> {
    let score = |g: &Gpu| match g.kind {
        2 => 2,
        1 => 1,
        _ => 0,
    };
    gpus.iter().filter(|g| g.api >= vulkan::version(1, 3)).max_by_key(|g| (score(g), g.driver_version))
}

/// A GPU's driver as its vendor numbers it: its own text (`driverInfo`) where it gives one, else its packed version.
fn driver_text(g: &Gpu) -> String {
    let number = if g.driver_info.is_empty() {
        build_assets::share::driver_number(&format!("{} {}", g.name, g.driver_name), u64::from(g.driver_version))
    } else {
        g.driver_info.clone()
    };
    if g.driver_name.is_empty() { number } else { format!("{} {number}", g.driver_name) }
}

/// The driver's fix, for this OS.
fn install_driver(windows: bool) -> &'static str {
    if windows {
        "Install the newest driver for your graphics card from its maker (NVIDIA, AMD or Intel), then restart the PC."
    } else {
        "Install your distribution's Vulkan loader and your GPU's Vulkan driver (Mesa's for AMD and Intel: \
         mesa-vulkan-drivers or vulkan-radeon / vulkan-intel; NVIDIA's own driver for NVIDIA), then restart."
    }
}

/// The checks `vulkan`, `gpu` and `driver`, from the probe's answer (`no_gpu`: Linux's why, when no GPU is listed).
pub fn vulkan_checks(probe: &Probe, no_gpu: &NoGpu, windows: bool) -> Vec<DoctorCheck> {
    let v = match probe {
        Probe::Failed(why) => {
            return vec![check(
                "vulkan",
                "fail",
                format!("Asking Vulkan for its GPUs {why}: a graphics driver is broken."),
                format!("Reinstall your graphics driver. {}", install_driver(windows)),
            )];
        }
        Probe::Answered(v) => v,
    };
    let Some(loader) = v.loader else {
        let file = if windows { "vulkan-1.dll" } else { "libvulkan.so.1" };
        return vec![check("vulkan", "fail", format!("No Vulkan loader ({file}) was found."), install_driver(windows))];
    };
    let mut out = Vec::new();
    if loader < vulkan::version(1, 3) {
        out.push(check(
            "vulkan",
            "fail",
            format!("The Vulkan loader is {}: the game needs 1.3.", vulkan::version_text(loader)),
            install_driver(windows),
        ));
        return out;
    }
    if !v.instance {
        out.push(check(
            "vulkan",
            "fail",
            format!(
                "The Vulkan loader is {}, but no driver took Longshot's request (error {}).",
                vulkan::version_text(loader),
                v.instance_result
            ),
            install_driver(windows),
        ));
        return out;
    }
    out.push(check("vulkan", "ok", format!("The Vulkan loader is {}.", vulkan::version_text(loader)), ""));
    if v.gpus.is_empty() {
        let (why, fix) = match no_gpu.cause {
            "nvidia-rt" => (
                "NVIDIA's kernel module is not loaded, on a real-time kernel (PREEMPT_RT)",
                "Start a kernel that is not a real-time one, or install NVIDIA's driver built for this kernel, then \
                 restart.",
            ),
            "nvidia-unloaded" => (
                "NVIDIA's kernel module is not loaded",
                "Install NVIDIA's driver for this kernel (your distribution's nvidia package), then restart.",
            ),
            "nvidia-vulkan" => (
                "NVIDIA's kernel module is loaded, but no Vulkan driver lists the card",
                "Install NVIDIA's Vulkan driver (the nvidia-utils package, or the same version as the module).",
            ),
            _ => ("no Vulkan driver lists a GPU", install_driver(windows)),
        };
        let cards = if no_gpu.cards.is_empty() { String::new() } else { format!(" (graphics cards {})", no_gpu.cards) };
        out.push(check("gpu", "fail", format!("No GPU: {why}{cards}."), fix));
        return out;
    }
    let Some(g) = picked(&v.gpus) else {
        let best = v.gpus.iter().max_by_key(|g| g.api).expect("listed");
        out.push(check(
            "gpu",
            "fail",
            format!("{} offers Vulkan {}: the game needs 1.3.", best.name, vulkan::version_text(best.api)),
            format!(
                "Update its driver ({}). A GPU whose newest driver offers no Vulkan 1.3 can't run the game.",
                install_driver(windows)
            ),
        ));
        return out;
    };
    let others = v.gpus.len() - 1;
    let also = if others > 0 { format!(" ({others} more listed)") } else { String::new() };
    out.push(check(
        "gpu",
        "ok",
        format!("{}, Vulkan {}{also}: the one the game uses.", g.name, vulkan::version_text(g.api)),
        "",
    ));
    // (the driver: a software renderer alone; the same GPU twice, by two drivers (an AMD laptop's processor graphics'
    // and card's: the older one crashed in its compiler, vk_main.cpp); else nothing known against it)
    let twice = v.gpus.iter().any(|o| {
        !std::ptr::eq(o, g) && o.vendor == g.vendor && o.device == g.device && o.driver_version != g.driver_version
    });
    let driver = driver_text(g);
    out.push(if g.kind == 4 {
        check(
            "driver",
            "fail",
            format!(
                "Only a software renderer ({}, {driver}) draws with Vulkan here: far too slow for the game.",
                g.name
            ),
            install_driver(windows),
        )
    } else if twice {
        check(
            "driver",
            "warn",
            format!(
                "{} is listed twice, by two drivers: the game uses {driver}, the newer, and the older one has crashed \
                 building the game's shaders.",
                g.name
            ),
            "Install the newest driver for the processor's graphics too (AMD's laptop driver), so both are the same.",
        )
    } else {
        check("driver", "ok", format!("{driver}: no driver the game refuses."), "")
    });
    out
}

/// Asks Vulkan in a process of its own (this program, `doctor --vulkan-probe`), for up to 30 s.
pub fn ask_vulkan(exe: &Path) -> Probe {
    let mut c = std::process::Command::new(exe);
    c.args(["doctor", PROBE_ARG])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let Ok(mut child) = c.spawn() else { return Probe::Failed("could not be started".into()) };
    let mut out = child.stdout.take().expect("piped");
    let read = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        s
    });
    let t = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if t.elapsed() < std::time::Duration::from_secs(30) => {
                std::thread::sleep(std::time::Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::Failed("hung for 30 s".into());
            }
        }
    };
    let text = read.join().unwrap_or_default();
    match serde_json::from_str::<Vulkan>(text.trim()) {
        Ok(v) if status.success() => Probe::Answered(v),
        _ => Probe::Failed(format!("crashed ({status})")),
    }
}

/// The probe's process: Vulkan asked, its answer as JSON on stdout.
pub fn vulkan_probe_main() -> std::process::ExitCode {
    match serde_json::to_string(&vulkan::probe()) {
        Ok(json) => {
            println!("{json}");
            std::process::ExitCode::SUCCESS
        }
        Err(_) => std::process::ExitCode::FAILURE,
    }
}

// ---- the disk

/// A place a build writes and how much it needs there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// What it is, for the player.
    pub what: &'static str,
    /// Where.
    pub path: PathBuf,
    /// The bytes a build may write there.
    pub need: u64,
    /// The bytes free there (`None`: not told).
    pub free: Option<u64>,
    /// Its drive (places on one drive share its room).
    pub volume: String,
}

/// The room a build needs (build.md's "Disk space"): in the cache 3 GB for a first build (`first`), and 0.6 GB more when
/// a release's tools are still to be unpacked (`unpack`), else 2 GB; 1 GB in the game folder; 0.1 GB for the profile's
/// saves.
pub fn needs(first: bool, unpack: bool) -> [u64; 3] {
    let cache = if first { 3 * GB } else { 2 * GB } + if unpack { 6 * GB / 10 } else { 0 };
    [cache, GB, GB / 10]
}

/// The check `disk_space`: each drive's free room against what the places on it need together.
pub fn disk_check(rooms: &[Room]) -> DoctorCheck {
    let gb = |b: u64| b as f64 / GB as f64;
    let mut volumes: Vec<&str> = rooms.iter().map(|r| r.volume.as_str()).collect();
    volumes.dedup();
    volumes.sort();
    volumes.dedup();
    let (mut short, mut said, mut unknown) = (Vec::new(), Vec::new(), false);
    for v in volumes {
        let on: Vec<&Room> = rooms.iter().filter(|r| r.volume == v).collect();
        let need: u64 = on.iter().map(|r| r.need).sum();
        let names = on.iter().map(|r| format!("{} ({})", r.what, r.path.display())).collect::<Vec<_>>().join(", ");
        match on[0].free {
            None => unknown = true,
            Some(free) if free < need => {
                short.push(format!("{:.1} GB free where {names} are; a build needs about {:.1} GB", gb(free), gb(need)))
            }
            Some(free) => said.push(format!("{:.1} GB free for {names}", gb(free))),
        }
    }
    if !short.is_empty() {
        check(
            "disk_space",
            "warn",
            short.join("; ") + ".",
            "Free some space on that drive (or move Longshot's folder to a drive with more), so a build doesn't stop \
             on a full drive.",
        )
    } else if unknown && said.is_empty() {
        check("disk_space", "unknown", "The free space could not be read.", "")
    } else {
        check("disk_space", "ok", said.join("; ") + ".", "")
    }
}

/// The drive `path` is on: Linux its device; Windows its root (`C:`, `\\server\share`).
fn volume(path: &Path) -> String {
    let at = game_files::disk::existing_parent(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&at).map(|m| m.dev().to_string()).unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        at.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).unwrap_or_default()
    }
}

// ---- the profile's game copy

/// What was found of the profile's copy.
#[derive(Debug, PartialEq, Eq)]
pub enum GameCopy {
    /// The profile names none (none set up yet), or its profile.json can't be read (why).
    NotSet(Option<String>),
    /// It names one that isn't there.
    Missing(PathBuf),
    /// It is there but can't be read as game files (the reason).
    Unreadable(PathBuf, String),
    /// `check`'s answer for it.
    Checked {
        /// Where.
        path: PathBuf,
        /// The supported version.
        ok: bool,
        /// Its sentence.
        text: String,
    },
}

/// The check `game_copy`.
pub fn copy_check(c: &GameCopy) -> DoctorCheck {
    let set_up = "Set the game up from your launcher (RecompLauncher's Easy Setup), or start Longshot once with --game \
                  <your .nup file or dump folder>.";
    match c {
        GameCopy::NotSet(None) => check("game_copy", "warn", "The profile names no game files yet.", set_up),
        GameCopy::NotSet(Some(why)) => check("game_copy", "fail", why.clone(), set_up),
        GameCopy::Missing(p) => check(
            "game_copy",
            "fail",
            format!("The game files the profile names, {}, are not there (moved or deleted).", p.display()),
            "Put them back where they were, or choose them again with --game <where they are now>.",
        ),
        GameCopy::Unreadable(p, why) => check(
            "game_copy",
            "fail",
            format!("The game files at {} can't be read: {why}", p.display()),
            "Close what holds them, or copy them again from your console's dump.",
        ),
        GameCopy::Checked { path, ok: true, text } => {
            check("game_copy", "ok", format!("{}: {text}", path.display()), "")
        }
        GameCopy::Checked { path, ok: false, text } => check(
            "game_copy",
            "fail",
            format!("{}: {text}", path.display()),
            "Dump the supported version of the game (`longshot-builder check --game <file>` says which one you have).",
        ),
    }
}

// ---- the folders

/// The check `folders`: each place and whether a file could be made there (the error's text when not).
pub fn folders_check(places: &[(&'static str, PathBuf, Result<(), String>)]) -> DoctorCheck {
    let bad: Vec<String> = places
        .iter()
        .filter_map(|(what, p, r)| r.as_ref().err().map(|e| format!("the {what} ({}): {e}", p.display())))
        .collect();
    if bad.is_empty() {
        let all = places.iter().map(|(w, _, _)| *w).collect::<Vec<_>>().join(", ");
        check("folders", "ok", format!("The {all} can be written."), "")
    } else {
        check(
            "folders",
            "fail",
            format!("Can't write {}.", bad.join("; ")),
            "Move Longshot's folder to one of your own (not under Program Files, not on a read-only drive), or give \
             your user write access to it.",
        )
    }
}

/// Whether a file can be made in `dir` (or, when it isn't there yet, the nearest folder above it that is): one empty
/// file made and removed.
fn writable(dir: &Path) -> Result<(), String> {
    let at = game_files::disk::existing_parent(dir);
    let probe = at.join(format!(".longshot-doctor-{}", std::process::id()));
    std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

// ---- Windows

/// The check `long_paths` (Windows): `enabled`, the registry's `LongPathsEnabled` (`None`: not read); the cache's path.
pub fn long_paths_check(enabled: Option<bool>, cache: &Path) -> DoctorCheck {
    let deepest = cache.as_os_str().len() + 1 + DEEPEST_IN_CACHE;
    let fix = format!(
        "Move Longshot's folder somewhere with a shorter path (like C:\\Games\\Longshot), or turn on long paths: in \
         PowerShell as administrator, run New-ItemProperty -Path HKLM:\\SYSTEM\\CurrentControlSet\\Control\\FileSystem \
         -Name LongPathsEnabled -Value 1 -PropertyType DWORD -Force, then restart. (The deepest path a build writes is \
         about {DEEPEST_IN_CACHE} characters past the cache's own.)"
    );
    match enabled {
        Some(true) => check("long_paths", "ok", "Long paths are turned on.", ""),
        _ if deepest >= MAX_PATH => check(
            "long_paths",
            "fail",
            format!(
                "Long paths are off, and the deepest path a build writes in the cache would be about {deepest} \
                 characters: Windows stops at {MAX_PATH}."
            ),
            fix,
        ),
        _ if deepest + 20 >= MAX_PATH => check(
            "long_paths",
            "warn",
            format!(
                "Long paths are off, and the build's deepest path comes to about {deepest} of {MAX_PATH} characters."
            ),
            fix,
        ),
        _ => check(
            "long_paths",
            "ok",
            format!("Long paths are off, but the build's deepest path is about {deepest} of {MAX_PATH} characters."),
            "",
        ),
    }
}

/// What Windows Defender says (the registry, read only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defender {
    /// Whether Defender is there at all (its key).
    pub present: bool,
    /// Its real-time protection is turned off (by the player, a policy, or another antivirus taking over).
    pub realtime_off: bool,
    /// The folders it excludes, when Windows shows them (to administrators only).
    pub exclusions: Option<Vec<String>>,
}

/// Whether `dir` is `excluded` or inside it (Windows' paths: any case, either separator).
fn inside(dir: &Path, excluded: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (d, e) = (norm(&dir.to_string_lossy()), norm(excluded));
    !e.is_empty() && (d == e || d.starts_with(&format!("{e}\\")))
}

/// The check `defender` (Windows), for the cache at `cache`.
pub fn defender_check(d: &Defender, cache: &Path) -> DoctorCheck {
    let how = format!(
        "A build writes thousands of files into {}, and Defender scans each one as it is written, which can make a \
         build several times slower. To exclude that folder: open Windows Security, then Virus & threat protection, \
         Manage settings, Exclusions, Add or remove exclusions, Add an exclusion, Folder, and choose it (or in \
         PowerShell as administrator: Add-MpPreference -ExclusionPath \"{}\"). Exclude that folder only. Longshot \
         changes no setting itself.",
        cache.display(),
        cache.display()
    );
    if !d.present {
        return check(
            "defender",
            "unknown",
            "Windows Defender isn't on (another antivirus may be): if builds are slow, it may be scanning them.",
            format!("Exclude {} in your antivirus, if it can.", cache.display()),
        );
    }
    if d.realtime_off {
        return check("defender", "ok", "Defender's real-time protection is off: it doesn't slow builds.", "");
    }
    match &d.exclusions {
        Some(all) if all.iter().any(|e| inside(cache, e)) => {
            check("defender", "ok", format!("Defender excludes {}.", cache.display()), "")
        }
        Some(_) => check(
            "defender",
            "warn",
            format!("Defender's real-time protection scans {}, where builds write.", cache.display()),
            how,
        ),
        None => check(
            "defender",
            "unknown",
            format!(
                "Defender's real-time protection is on; whether it excludes {} can't be read (Windows shows its \
                 exclusions to administrators only).",
                cache.display()
            ),
            how,
        ),
    }
}

/// [`Defender`] from the registry (`reg`: build_core::machine_registry, a test's stand-in).
pub fn defender(reg: impl Fn(&str) -> (Vec<(String, String)>, Vec<String>)) -> Defender {
    const KEY: &str = r"SOFTWARE\Microsoft\Windows Defender";
    const POLICY: &str = r"SOFTWARE\Policies\Microsoft\Windows Defender";
    let (values, subkeys) = reg(KEY);
    let on = |vals: &[(String, String)], name: &str| vals.iter().any(|(k, v)| k.eq_ignore_ascii_case(name) && v == "1");
    let realtime_off = on(&values, "DisableAntiSpyware")
        || on(&reg(&format!(r"{KEY}\Real-Time Protection")).0, "DisableRealtimeMonitoring")
        || on(&reg(&format!(r"{POLICY}\Real-Time Protection")).0, "DisableRealtimeMonitoring")
        || on(&reg(POLICY).0, "DisableAntiSpyware");
    // (the exclusions' key lists its subkeys only to an administrator: otherwise nothing at all is read)
    let readable = !reg(&format!(r"{KEY}\Exclusions")).1.is_empty();
    let exclusions = readable.then(|| {
        [KEY, POLICY].iter().flat_map(|k| reg(&format!(r"{k}\Exclusions\Paths")).0).map(|(path, _)| path).collect()
    });
    Defender { present: !values.is_empty() || !subkeys.is_empty(), realtime_off, exclusions }
}

// ---- the game running

/// The check `game_running`: the profile `profile` held by a play (`holder`: its pid).
pub fn running_check(held: bool, holder: Option<u32>, profile: &Path) -> DoctorCheck {
    if held {
        let by = holder.map(|p| format!(" (process {p})")).unwrap_or_default();
        check(
            "game_running",
            "warn",
            format!("Longshot is running for the profile {}{by}: the game, or its build.", profile.display()),
            "Close the game (or wait for its build to end) before you build, update or start it again.",
        )
    } else {
        check("game_running", "ok", format!("Longshot isn't running for the profile {}.", profile.display()), "")
    }
}

// ---- the run

/// Where doctor looks.
#[derive(Debug)]
pub struct Places {
    /// The version folder (this program's).
    pub version: PathBuf,
    /// The builder's cache.
    pub cache: PathBuf,
    /// The profile.
    pub profile: PathBuf,
}

/// Every check, for the places `p`, this program at `exe` (its Vulkan probe); `unpack`: a release's tools are still to
/// be unpacked into the cache.
pub fn checks(p: &Places, exe: &Path, unpack: bool) -> Vec<DoctorCheck> {
    let windows = cfg!(windows);
    let mut all = vulkan_checks(&ask_vulkan(exe), &no_gpu(Path::new("/")), windows);
    // (the game folder the profile plays: builds/default, the Mod API's stamp aside)
    let builds = build_driver::play::game_folder(&p.version, None);
    let first = build_driver::folder::Record::read(&builds).is_err();
    let [cache_need, builds_need, profile_need] = needs(first, unpack);
    let rooms: Vec<Room> =
        [("cache", &p.cache, cache_need), ("game folder", &builds, builds_need), ("profile", &p.profile, profile_need)]
            .into_iter()
            .map(|(what, path, need)| Room {
                what,
                path: path.clone(),
                need,
                free: game_files::disk::free_bytes(path),
                volume: volume(path),
            })
            .collect();
    all.push(disk_check(&rooms));
    all.push(copy_check(&game_copy(&p.profile)));
    let places =
        [("cache", p.cache.clone()), ("game folder", builds.clone()), ("profile", p.profile.clone())].map(|(w, d)| {
            let r = writable(&d);
            (w, d, r)
        });
    all.push(folders_check(&places));
    if windows {
        let enabled = build_core::machine_registry(r"SYSTEM\CurrentControlSet\Control\FileSystem")
            .0
            .into_iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("LongPathsEnabled"))
            .map(|(_, v)| v == "1");
        all.push(long_paths_check(enabled, &p.cache));
        all.push(defender_check(&defender(build_core::machine_registry), &p.cache));
    }
    let held = build_core::lock::ProfileLock::held(&p.profile);
    all.push(running_check(held, build_core::lock::ProfileLock::holder(&p.profile), &p.profile));
    all
}

/// What the profile at `profile` names for its copy, checked as `check` checks it.
fn game_copy(profile: &Path) -> GameCopy {
    let p = match build_driver::play::Profile::read(profile) {
        Ok(Some(p)) => p,
        Ok(None) => return GameCopy::NotSet(None),
        Err(e) => return GameCopy::NotSet(Some(e.to_string())),
    };
    let Some(path) = p.game_in(profile) else { return GameCopy::NotSet(None) };
    if !path.exists() {
        return GameCopy::Missing(path);
    }
    let game = match build_driver::GameSource::from_path(&path) {
        Ok(g) => g,
        Err(e) => return GameCopy::Unreadable(path, e.to_string()),
    };
    match build_driver::Stages::real().game.check(&game) {
        Ok(c) => GameCopy::Checked { path, ok: c.ok, text: c.text },
        Err(e) => GameCopy::Unreadable(path, e.to_string()),
    }
}

/// `doctor`: the checks said with `r`. Whether none failed.
pub fn run(p: &Places, exe: &Path, unpack: bool, r: &Reporter) -> bool {
    let checks = checks(p, exe, unpack);
    let ok = !checks.iter().any(|c| c.status == "fail");
    r.send(Event::Doctor { ok, checks });
    ok
}

#[cfg(test)]
mod tests;
