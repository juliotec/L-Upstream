//! What a compile is asked to do ([`Options`]) and the switches it reads ([`Settings`]).
//!
//! The switches are the environment's (`docs/dev/reference/build.md`, "Build switches"), read once into [`Settings`]
//! by [`Settings::from_env`], or set by the caller: nothing below reads the environment itself.

use std::path::PathBuf;

/// What the compile makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The game: its executable and the renderer's module.
    Game,
    /// The game alone, the renderer neither compiled nor linked (`compile --only game`: a development pack whose
    /// change cannot reach the renderer; never a player's build).
    GameOnly,
    /// The renderer alone, no lift needed (`compile --only renderer`).
    RendererOnly,
    /// Everything up to the compile, without the lift's files (no game here): what would be compiled, and which files
    /// it reads are missing. Nothing is compiled (CI's check of a player's tree).
    DryRun,
    /// The release's set of third-party objects, into this folder ([`crate::prebuilt`]): those alone compiled, nothing
    /// linked, no game needed.
    ThirdPartySet(PathBuf),
}

/// A profile build's half: count branches, or build with a profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pgo {
    /// Neither.
    Off,
    /// Count branches for a profile (`=gen`).
    Generate,
    /// Build with this merged profile.
    Use(PathBuf),
}

impl Pgo {
    fn from(value: Option<String>) -> Pgo {
        match value.as_deref().map(str::trim) {
            None | Some("") => Pgo::Off,
            Some("gen") => Pgo::Generate,
            Some(p) => Pgo::Use(PathBuf::from(p)),
        }
    }
}

/// An optimisation level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// `-O0`.
    O0,
    /// `-O1`.
    O1,
    /// `-O2`.
    O2,
    /// `-O3`.
    O3,
}

impl Level {
    /// The flag.
    pub fn flag(self) -> &'static str {
        match self {
            Level::O0 => "-O0",
            Level::O1 => "-O1",
            Level::O2 => "-O2",
            Level::O3 => "-O3",
        }
    }

    /// `O1`, `-O2`, `o3`...; `None` for anything else.
    pub fn parse(text: &str) -> Option<Level> {
        match text.trim().trim_start_matches('-').to_ascii_uppercase().as_str() {
            "O0" => Some(Level::O0),
            "O1" => Some(Level::O1),
            "O2" => Some(Level::O2),
            "O3" => Some(Level::O3),
            _ => None,
        }
    }
}

/// The build's switches.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The game is built for Windows (on Windows, or `RX_TARGET=windows` with llvm-mingw).
    pub windows: bool,
    /// `LONGSHOT_MINGW`: llvm-mingw's folder, whose clang, headers and libraries build the Windows game.
    pub mingw: Option<PathBuf>,
    /// `LONGSHOT_LLVM`: the Linux compiler's folder (`bin/clang`, `bin/ld.lld`): the builder's tools/llvm (build-driver's
    /// tools.rs), or a developer's LLVM release.
    pub llvm: Option<PathBuf>,
    /// `EMSDK`: an emsdk checkout, where clang and lld are looked for (`upstream/bin`) when `LONGSHOT_LLVM` names none.
    pub emsdk: PathBuf,
    /// `LONGSHOT_SYSROOT`: the root a Linux build compiles and links against (tools.lock's `sysroot`).
    pub sysroot: Option<PathBuf>,
    /// `RX_MARCH`: the CPU level, else this PC's CPU.
    pub march: Option<String>,
    /// `RX_CPM`: a CPM package cache, looked in first.
    pub cpm: Option<PathBuf>,
    /// `LONGSHOT_VULKAN_HEADERS`, `LONGSHOT_SPIRV_HEADERS`, `LONGSHOT_VMA_HEADERS`: a package root or its `include/`.
    pub header_dirs: [Option<PathBuf>; 3],
    /// `LONGSHOT_FFMPEG`: a prefix with FFmpeg's static libraries (Linux; else pkg-config's).
    pub ffmpeg: Option<PathBuf>,
    /// `LONGSHOT_CARGO`: the cargo that builds the game's Rust when the tree carries no library of its sources (else
    /// the PATH's; [`crate::rust`]).
    pub cargo: Option<PathBuf>,
    /// `LONGSHOT_SDL2`: `libSDL2-2.0.so.0` to put beside a Linux game (the file, or the folder that holds it).
    pub sdl2: Option<PathBuf>,
    /// `LONGSHOT_SDL_DIR`: the folder of `SDL2.dll` and `SDL3.dll` for a Windows game (else the tree's third_party/sdl2).
    pub sdl_dir: Option<PathBuf>,
    /// `LONGSHOT_NGX_SDK`: NVIDIA's NGX SDK (DLSS).
    pub ngx_sdk: Option<PathBuf>,
    /// `LONGSHOT_FSR=1`: AMD FSR 4.
    pub fsr: bool,
    /// `LONGSHOT_TRACY=1`: a Tracy build (a developer's).
    pub tracy: bool,
    /// `LONGSHOT_TRACY_FIBERS=1`: the game's fibers told to Tracy.
    pub tracy_fibers: bool,
    /// `RX_RELEASE=1`: a release (no console window; shaders prepared before the title).
    pub release: bool,
    /// `RX_VK_PGO`: the renderer's profile.
    pub vk_pgo: Pgo,
    /// `RX_HOT_OPT`: the guest profile's hot units' level (O2).
    pub hot_level: Level,
    /// `RX_A64_RT_O2` (on unless off): the lift's runtime (`gen/rt/a64_rt.c`, `a64_fp.c`) at -O2.
    pub a64_rt_o2: bool,
    /// The lifted units' level (`LONGSHOT_IR_LEVEL`, O1).
    pub ir_level: Level,
    /// `RX_COLD_OPT`: the level of the units of the functions the guest profile saw never run (`lifted_cold_*`, O0).
    pub cold_level: Level,
    /// `LONGSHOT_MEM_GATE=0`: compilers start by the job count alone.
    pub mem_gate: bool,
    /// `LONGSHOT_OBJ_STORE`: one object store for every work folder; else the work folder's own.
    pub obj_store: Option<PathBuf>,
    /// `LONGSHOT_THIRD_PARTY_OBJECTS`: the prebuilt third-party set (`None`: the tools' own; `Some(None)`: off).
    pub third_party_objects: Option<Option<PathBuf>>,
    /// `LONGSHOT_JOBS`, as given (the driver's `--jobs` wins).
    pub jobs_env: Option<String>,
    /// `LONGSHOT_VERSION`: the release's name, else the tree's `VERSION` file or git's.
    pub version: Option<String>,
}

/// An on/off switch, the game's rule (`gpu/vk/compat/longshot_env.h`): unset or off ([`build_core::switch`]) is off,
/// any other value on.
pub fn on(value: Option<&str>) -> bool {
    value.is_some_and(|v| build_core::switch(v) != Some(false))
}

fn path(value: Option<String>) -> Option<PathBuf> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Settings {
    /// The switches as the environment sets them.
    pub fn from_env() -> Settings {
        Settings::from_lookup(|name| std::env::var(name).ok())
    }

    /// The switches as `get` gives each variable (a test's table).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Settings {
        let windows = cfg!(windows) || get("RX_TARGET").is_some_and(|t| t.eq_ignore_ascii_case("windows"));
        let home = get(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default();
        let emsdk = path(get("EMSDK")).unwrap_or_else(|| {
            if cfg!(windows) { PathBuf::from(r"C:\emsdk") } else { PathBuf::from(home).join("emsdk") }
        });
        let level = |name: &str, default: Level| get(name).and_then(|v| Level::parse(&v)).unwrap_or(default);
        let third_party_objects = match get("LONGSHOT_THIRD_PARTY_OBJECTS").map(|v| v.trim().to_string()) {
            None => None,
            Some(v) if v.is_empty() => None,
            Some(v) if !on(Some(&v)) => Some(None),
            Some(v) => Some(Some(PathBuf::from(v))),
        };
        Settings {
            windows,
            mingw: if windows { path(get("LONGSHOT_MINGW")) } else { None },
            llvm: path(get("LONGSHOT_LLVM")),
            emsdk,
            sysroot: path(get("LONGSHOT_SYSROOT")),
            march: get("RX_MARCH").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()),
            cpm: path(get("RX_CPM")),
            header_dirs: [
                path(get("LONGSHOT_VULKAN_HEADERS")),
                path(get("LONGSHOT_SPIRV_HEADERS")),
                path(get("LONGSHOT_VMA_HEADERS")),
            ],
            ffmpeg: path(get("LONGSHOT_FFMPEG")),
            cargo: path(get("LONGSHOT_CARGO")),
            sdl2: path(get("LONGSHOT_SDL2")).map(|p| if p.is_dir() { p.join("libSDL2-2.0.so.0") } else { p }),
            sdl_dir: path(get("LONGSHOT_SDL_DIR")),
            ngx_sdk: path(get("LONGSHOT_NGX_SDK")),
            fsr: get("LONGSHOT_FSR").as_deref() == Some("1"),
            tracy: get("LONGSHOT_TRACY").as_deref() == Some("1"),
            tracy_fibers: get("LONGSHOT_TRACY_FIBERS").as_deref() == Some("1"),
            release: get("RX_RELEASE").as_deref() == Some("1"),
            vk_pgo: Pgo::from(get("RX_VK_PGO")),
            hot_level: level("RX_HOT_OPT", Level::O2),
            // (on unless set off)
            a64_rt_o2: get("RX_A64_RT_O2").is_none_or(|v| build_core::switch(&v) != Some(false)),
            ir_level: level("LONGSHOT_IR_LEVEL", Level::O1),
            cold_level: level("RX_COLD_OPT", Level::O0),
            mem_gate: get("LONGSHOT_MEM_GATE").as_deref() != Some("0"),
            obj_store: path(get("LONGSHOT_OBJ_STORE")).filter(|p| p.as_os_str() != "0"),
            third_party_objects,
            jobs_env: get("LONGSHOT_JOBS"),
            version: get("LONGSHOT_VERSION"),
        }
    }
}

/// A compile.
#[derive(Clone, Debug)]
pub struct Options {
    /// The builder's source tree: `host/`, `gpu/`, `third_party/`, `assets/`.
    pub root: PathBuf,
    /// The work folder: the Windows resources the driver writes (`longshot_game.res`), the toolchain's probes.
    pub work: PathBuf,
    /// The lift's output (docs/dev/reference/builder.md, "The gen/ folder"): `units.txt` and its units, `table.c`,
    /// `imports.c`, `image.c`, `rt/`.
    pub gen_dir: PathBuf,
    /// The build's folder: the objects, the executable and the renderer's module, the compile's state.
    pub build_dir: PathBuf,
    /// What to make.
    pub mode: Mode,
    /// Compilers at once (`--jobs`); `None`: `LONGSHOT_JOBS`, else one per CPU thread but one.
    pub jobs: Option<usize>,
    /// The object store's folder: one the builder's updates leave alone (a per-user cache the driver chooses), else
    /// `LONGSHOT_OBJ_STORE`, else the work folder's `objects/`.
    pub store: Option<PathBuf>,
    /// The switches.
    pub settings: Settings,
    /// Where a lifted unit's text comes from when its file is not in gen/ (the lift keeps no texts by default):
    /// made as the compile needs it, and given to the compiler on its standard input.
    pub ir_text: Option<IrSource>,
    /// Told as soon as the renderer and the shader preparation's driver are linked, while the game's units still
    /// compile (their units go first): the driver starts the preparation then.
    pub early: Option<EarlyRenderer>,
    /// The build's memory gate, shared with what runs beside the compile (the shader preparation), so one
    /// `LONGSHOT_MEM_LIMIT_GB` holds them all; `None`: the compile's own, from the environment. Unused when the
    /// switches turn the gate off (`LONGSHOT_MEM_GATE=0`).
    pub gate: Option<std::sync::Arc<build_core::memory::Gate>>,
}

/// What is told the renderer is ready before the game is: its module and the preparation's driver (lib.rs
/// `link_prepare`).
pub trait RendererReady: Send + Sync {
    /// The renderer's module and the preparation's driver, both linked.
    fn ready(&self, module: &std::path::Path, driver: &std::path::Path);
}

/// A [`RendererReady`], shared.
#[derive(Clone)]
pub struct EarlyRenderer(pub std::sync::Arc<dyn RendererReady>);

impl std::fmt::Debug for EarlyRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EarlyRenderer")
    }
}

/// What makes a lifted unit's text on demand (the lift's `lift::whole::Texts`, through the driver).
pub trait IrText: Send + Sync {
    /// The text of the unit whose file would be `name` (`lifted_*.ll`).
    fn text(&self, name: &str) -> build_core::Result<String>;
}

/// An [`IrText`], shared.
#[derive(Clone)]
pub struct IrSource(pub std::sync::Arc<dyn IrText>);

impl std::fmt::Debug for IrSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IrSource")
    }
}

impl Options {
    /// A compile of the game in `work` (the lift's output in `<work>/gen`, the build in `<work>/build`), from the tree
    /// at `root`, with the environment's switches.
    pub fn new(root: impl Into<PathBuf>, work: impl Into<PathBuf>) -> Options {
        let settings = Settings::from_env();
        let mode = Mode::Game;
        let work: PathBuf = work.into();
        Options {
            root: root.into(),
            gen_dir: work.join("gen"),
            build_dir: work.join("build"),
            work,
            mode,
            jobs: None,
            store: None,
            settings,
            ir_text: None,
            early: None,
            gate: None,
        }
    }

    /// The object store's folder.
    pub fn store_dir(&self) -> PathBuf {
        self.store.clone().or_else(|| self.settings.obj_store.clone()).unwrap_or_else(|| self.work.join("objects"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings(vars: &[(&str, &str)]) -> Settings {
        let m: HashMap<String, String> = vars.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        Settings::from_lookup(|n| m.get(n).cloned())
    }

    #[test]
    fn the_switches_read_from_the_environment() {
        let s = settings(&[]);
        assert!(s.a64_rt_o2 && s.mem_gate);
        assert_eq!((s.hot_level, s.ir_level, s.cold_level), (Level::O2, Level::O1, Level::O0));
        let s = settings(&[
            ("RX_COLD_OPT", "O1"),
            ("RX_HOT_OPT", "O1"),
            ("RX_VK_PGO", "/p.profdata"),
            ("RX_A64_RT_O2", ""),
            ("LONGSHOT_IR_LEVEL", "-O2"),
            ("LONGSHOT_THIRD_PARTY_OBJECTS", "0"),
        ]);
        assert_eq!((s.hot_level, s.ir_level, s.cold_level), (Level::O1, Level::O2, Level::O1));
        assert_eq!(s.vk_pgo, Pgo::Use("/p.profdata".into()));
        assert!(!s.a64_rt_o2, "RX_A64_RT_O2 set but empty is off");
        assert_eq!(s.third_party_objects, Some(None));
        assert_eq!(settings(&[("RX_VK_PGO", "gen")]).vk_pgo, Pgo::Generate);
    }
}
