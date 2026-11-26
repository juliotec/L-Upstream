//! Every unit the compile builds, with its compiler and flags, and the two links.
//!
//! The game's executable is the lift's output (docs/dev/reference/builder.md, "The gen/ folder": the units `units.txt`
//! lists, the function table, the import stubs, the boot constants and the lift's runtime in `gen/rt`), the host in
//! `host/`, and the libraries it uses (libopus, zstd's decoder, Tracy's client in a Tracy build). The renderer,
//! `longshot_vk.so` / `longshot_vk.dll`, is a module of its own: `gpu/vk` with Eden's shader recompiler, sirit, Dear
//! ImGui, ReShade's FX compiler and zbic's zstd.
//!
//! Nothing is put on a command only to change an object's key: what decides an object without being a flag (a
//! profile's content) goes in its key ([`Unit::extra`]), and the command stays what the compiler needs.

use crate::options::{Level, Mode, Options, Pgo};
use crate::packages;
use crate::toolchain::Toolchain;
use build_core::{Error, IoContext, Result};
use std::path::{Path, PathBuf};

/// Which module a unit's object goes into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    /// The game's executable.
    Game,
    /// The renderer's module.
    Renderer,
}

/// What a unit is, for its memory need, its time estimate and the failure's account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A lifted unit, LLVM IR (`lifted_*.ll`).
    LiftedIr,
    /// A hot lifted unit (`lifted_hot_*.ll`), at the hot code's level.
    LiftedHotIr,
    /// A cold lifted unit (`lifted_cold_*.ll`: functions the guest profile saw never run), at the cold code's level.
    LiftedColdIr,
    /// The lift's C: the function table, the import stubs, the boot constants, its runtime (`gen/rt`).
    LiftC,
    /// A source of the tree (host/, gpu/vk's own files).
    Tree,
    /// Third-party code (Eden's recompiler, sirit, Dear ImGui, zstd, libopus, ReShade's FX compiler, Tracy).
    ThirdParty,
    /// A file the build wrote (the fonts, the adapted preprocessor).
    Generated,
}

impl Kind {
    /// Whether the lift wrote it (a failure there may be the lift's: the driver lifts again).
    pub fn from_lift(self) -> bool {
        matches!(self, Kind::LiftedIr | Kind::LiftedHotIr | Kind::LiftedColdIr | Kind::LiftC)
    }
}

/// What the lift's `units.txt` says of a lifted unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its text's SHA-1: its object's key starts from it, so a cold build hashes none of the IR again (while the file
    /// is at this size, or not there: its text is made when needed; else it is read).
    pub sha1: String,
    /// Its file's size.
    pub bytes: u64,
    /// Its estimated compile cost (bytes of IR, a function's size plus its square over 1.4 MB): the compile's order.
    pub cost: u64,
}

impl Listed {
    /// Whether its text is not made yet (`-` for its content key: the lift keeps no texts, and this unit's is new), its
    /// size and cost estimated.
    pub fn pending(&self) -> bool {
        self.sha1 == "-"
    }
}

/// One compile.
#[derive(Clone, Debug)]
pub struct Unit {
    /// What the unit is called in the log and the events: its source's file name.
    pub name: String,
    /// Its module.
    pub module: Module,
    /// What it is.
    pub kind: Kind,
    /// The compiler.
    pub compiler: PathBuf,
    /// The source.
    pub source: PathBuf,
    /// The object, in the build's `obj/`.
    pub object: PathBuf,
    /// The flags (the depfile's are added by the compile).
    pub flags: Vec<String>,
    /// What the key holds besides the flags, the source and the headers (a profile's content).
    pub extra: Vec<(String, Vec<u8>)>,
    /// Whether the compile writes a depfile (every C and C++ unit; not LLVM IR, which includes nothing).
    pub depfile: bool,
    /// Whether the release's prebuilt set may hold its object.
    pub third_party: bool,
    /// A lifted unit: what `units.txt` says of it.
    pub listed: Option<Listed>,
}

/// The folders of the tree and the build.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The tree.
    pub root: PathBuf,
    /// The work folder.
    pub work: PathBuf,
    /// The lift's output.
    pub gen_dir: PathBuf,
    /// The build's folder.
    pub out: PathBuf,
    /// The objects.
    pub obj: PathBuf,
    /// The files the build writes for the renderer.
    pub made: PathBuf,
    /// The lift's runtime and its headers (`gen/rt`: `a64.h`, `a64_config.h`), which the host is compiled against.
    pub rt: PathBuf,
}

impl Layout {
    /// The layout of a compile.
    pub fn of(o: &Options) -> Layout {
        let out = o.build_dir.clone();
        Layout {
            root: o.root.clone(),
            work: o.work.clone(),
            gen_dir: o.gen_dir.clone(),
            obj: out.join("obj"),
            made: out.join("made"),
            rt: o.gen_dir.join("rt"),
            out,
        }
    }

    fn r(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// What the plan worked out beside the units.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// The units, in link order.
    pub units: Vec<Unit>,
    /// The renderer's header packages (Vulkan, SPIR-V, VMA), when the renderer is built.
    pub vk_headers: Option<[PathBuf; 3]>,
    /// FFmpeg on Windows (the CPM cache's).
    pub ffmpeg_windows: Option<PathBuf>,
    /// FFmpeg's link flags on Linux.
    pub ffmpeg_linux_libs: Vec<String>,
    /// dav1d on Windows.
    pub dav1d: Option<PathBuf>,
    /// NVIDIA's NGX SDK, when DLSS is built.
    pub ngx: Option<PathBuf>,
    /// Whether the game's executable is linked.
    pub link_game: bool,
    /// The log's lines about the plan.
    pub notes: Vec<String>,
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn inc(p: &Path) -> String {
    format!("-I{}", p.display())
}

fn sorted_files(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.flatten().filter(|e| e.path().is_file()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.retain(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(&keep));
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `flags` with the level `from` replaced by `to`.
fn relevel(flags: &[String], from: Level, to: Level) -> Vec<String> {
    flags.iter().map(|f| if f == from.flag() { to.flag().to_string() } else { f.clone() }).collect()
}

/// Context for the plan: what it reads besides the options.
pub struct Inputs<'a> {
    /// The compile.
    pub options: &'a Options,
    /// The tools.
    pub tc: &'a Toolchain,
    /// The folders.
    pub layout: &'a Layout,
    /// The version line's define.
    pub version: String,
    /// The shader translator's stamp (when the renderer is built).
    pub shader_stamp: Option<String>,
    /// The renderer's profile: `RX_VK_PGO`'s, else the committed one this compiler reads (vk_profile.rs).
    pub vk_pgo: Pgo,
}

/// The plan of a compile.
pub fn plan(i: &Inputs) -> Result<Plan> {
    let (o, tc, l) = (i.options, i.tc, i.layout);
    let st = &o.settings;
    let linux = !tc.windows;
    let mut p = Plan::default();
    let tp_cpu = &tc.third_party_cpu;
    let with_lift = !matches!(o.mode, Mode::RendererOnly);
    let game = !matches!(o.mode, Mode::RendererOnly);
    let renderer = matches!(o.mode, Mode::Game | Mode::RendererOnly | Mode::DryRun | Mode::ThirdPartySet(_));
    let gen_dir = &l.gen_dir;
    let unit = |name: String, module, kind, cc: &Path, src: PathBuf, obj: &str, flags: Vec<String>| Unit {
        name,
        module,
        kind,
        compiler: cc.to_path_buf(),
        source: src,
        object: l.obj.join(obj),
        flags,
        extra: Vec::new(),
        depfile: true,
        third_party: false,
        listed: None,
    };

    // ---- the lift's output
    let base = [tc.target.clone(), tc.cpu.clone()].concat();
    let ir_fl = ir_unit_flags(&base, st.ir_level);
    let ir_hot_fl = relevel(&ir_fl, st.ir_level, st.hot_level);
    let ir_cold_fl = relevel(&ir_fl, st.ir_level, st.cold_level);
    // (the lift's C: against gen/rt alone, the lift's choices in its a64_config.h, none on the command line)
    let mut lift_fl = base.clone();
    lift_fl.push(inc(&l.rt));
    lift_fl.extend(s(&["-Wno-everything", "-fno-strict-aliasing", "-mcx16", "-O1", "-g0", "-ffunction-sections"]));
    let rt_fl = if st.a64_rt_o2 { relevel(&lift_fl, Level::O1, Level::O2) } else { lift_fl.clone() };
    p.notes.push(format!("the lift's runtime at {} (RX_A64_RT_O2=0: -O1)", if st.a64_rt_o2 { "-O2" } else { "-O1" }));

    if with_lift && matches!(o.mode, Mode::Game | Mode::GameOnly) {
        for (name, listed) in lifted_units(gen_dir)? {
            let stem = name.trim_end_matches(".ll").to_string();
            // (the guest profile's hottest: their branch weights and entry counts are in the IR itself; the ones it
            // saw never run built cheaply, -O0: a fifth of -O1's compile, half again the code)
            let (kind, flags) = if name.starts_with("lifted_hot_") {
                (Kind::LiftedHotIr, &ir_hot_fl)
            } else if name.starts_with("lifted_cold_") {
                (Kind::LiftedColdIr, &ir_cold_fl)
            } else {
                (Kind::LiftedIr, &ir_fl)
            };
            let mut u = unit(
                name.clone(),
                Module::Game,
                kind,
                &tc.cc,
                gen_dir.join(&name),
                &format!("{stem}.o"),
                flags.clone(),
            );
            u.depfile = false;
            u.listed = Some(listed);
            p.units.push(u);
        }
    }
    if with_lift {
        for rel in LIFT_C {
            let src = gen_dir.join(rel);
            let name = file_name(&src);
            let flags = if rel.starts_with("rt/") { rt_fl.clone() } else { lift_fl.clone() };
            let obj = format!("lift_{}.o", name.trim_end_matches(".c"));
            p.units.push(unit(name, Module::Game, Kind::LiftC, &tc.cc, src, &obj, flags));
        }
    }

    // ---- the runtime (host/), and what only it uses
    let dirs = packages::cpm_dirs(st, &l.root, &l.work);
    let host = l.r("host");
    let posix = host.join("posix");
    let tracy_public = l.r("third_party/tracy/public");
    let mut tracy_flags = Vec::new();
    if st.tracy {
        if st.tracy_fibers {
            tracy_flags.push("-DTRACY_FIBERS".to_string());
        }
        tracy_flags.extend(s(&[
            "-DTRACY_ENABLE",
            "-DTRACY_ON_DEMAND",
            "-DTRACY_ONLY_LOCALHOST",
            "-DTRACY_NO_BROADCAST",
            "-DTRACY_NO_CRASH_HANDLER",
            "-DTRACY_NO_CODE_TRANSFER",
            "-DTRACY_DELAYED_INIT",
            "-DTRACY_MANUAL_LIFETIME",
            "-fno-omit-frame-pointer",
        ]));
        tracy_flags.push(inc(&tracy_public));
    }
    if game {
        p.link_game = matches!(o.mode, Mode::Game | Mode::GameOnly);
        if linux {
            match packages::ffmpeg_linux(st) {
                Ok((c, libs)) => {
                    p.ffmpeg_linux_libs = libs;
                    p.notes.push(format!("FFmpeg: {}", c.join(" ")));
                    plan_host(i, &mut p, &host, &posix, &tracy_flags, &c, &dirs)?;
                }
                Err(missing) => {
                    if matches!(o.mode, Mode::ThirdPartySet(_)) {
                        plan_host(i, &mut p, &host, &posix, &tracy_flags, &[], &dirs)?;
                    } else {
                        return Err(Error::player("ffmpeg_missing", missing.join(" ")));
                    }
                }
            }
        } else {
            p.ffmpeg_windows = packages::ffmpeg_windows(&dirs);
            plan_host(i, &mut p, &host, &posix, &tracy_flags, &[], &dirs)?;
        }
        let cc = &tc.cc;
        if st.tracy {
            p.notes.push(
                "the game: with the Tracy profiler's client (LONGSHOT_TRACY: 127.0.0.1 only, a developer's build)"
                    .into(),
            );
            let mut f = base.clone();
            f.extend(s(&["-std=c++17", "-O2", "-g0", "-DNDEBUG", "-Wno-everything"]));
            f.extend(tracy_flags.iter().cloned());
            f.push("-DTRACY_EXPORTS".into());
            let mut u = unit(
                "TracyClient.cpp".into(),
                Module::Game,
                Kind::ThirdParty,
                &tc.cxx,
                tracy_public.join("TracyClient.cpp"),
                "host_tracy_client.o",
                f,
            );
            u.third_party = false; // (Tracy builds are a developer's: not in the release's set)
            p.units.push(u);
        }
        // (libopus 1.5.2: nn::codec's Opus decoder, host/nn_codec.c)
        let opus = l.r("third_party/opus");
        let mut of = [tc.target.clone(), tp_cpu.clone()].concat();
        of.extend(s(&[
            "-O2",
            "-g0",
            "-Wno-everything",
            "-DOPUS_BUILD",
            "-DUSE_ALLOCA",
            "-DHAVE_LRINTF",
            "-DHAVE_LRINT",
        ]));
        of.extend(["include", "celt", "silk", "silk/float", "src"].iter().map(|d| inc(&opus.join(d))));
        let list = opus.join("sources.txt");
        let sources = std::fs::read_to_string(&list).at(&list)?;
        for rel in sources.split_whitespace() {
            let obj = format!("opus_{}.o", rel.replace('/', "_").trim_end_matches(".c"));
            let mut u =
                unit(file_name(Path::new(rel)), Module::Game, Kind::ThirdParty, cc, opus.join(rel), &obj, of.clone());
            u.third_party = true;
            p.units.push(u);
        }
        // (zstd's decoder: host/zs.c reads every zstd frame of the host's own with it, host/hos_zstd.c the game's streams)
        let mut zf = [tc.target.clone(), tp_cpu.clone()].concat();
        zf.extend(s(&["-O2", "-g0", "-DNDEBUG", "-Wno-everything", "-DZSTD_DISABLE_ASM", "-DZSTD_LEGACY_SUPPORT=0"]));
        zf.push("-DZSTD_MULTITHREAD=0".into());
        p.units.extend(zstd_units(l, cc, &zf).into_iter().map(|mut u| {
            u.object = l.obj.join(format!("zstd_{}", file_name(&u.object)));
            u
        }));
    }

    // ---- the renderer
    if renderer && !matches!(o.mode, Mode::GameOnly) {
        let found: Vec<Option<PathBuf>> = packages::VK_HEADERS
            .iter()
            .zip(&st.header_dirs)
            .map(|(pkg, env)| packages::header_package(pkg, env.as_deref(), &dirs))
            .collect();
        if found.iter().any(Option::is_none) {
            // (was silent once: the exe linked, the renderer fell through to a stub, a black window)
            let missing: Vec<&str> =
                packages::VK_HEADERS.iter().zip(&found).filter(|(_, f)| f.is_none()).map(|(p, _)| p.label).collect();
            return Err(Error::player(
                "vulkan_headers_missing",
                format!(
                    "no headers for {} (missing {}). Set RX_CPM to a CPM cache, or LONGSHOT_VULKAN_HEADERS / \
                     LONGSHOT_SPIRV_HEADERS / LONGSHOT_VMA_HEADERS (package root or its include/ folder), or install \
                     distro packages vulkan-headers + spirv-headers + vulkan-memory-allocator.",
                    if linux { "longshot_vk.so" } else { "longshot_vk.dll" },
                    missing.join(", ")
                ),
            ));
        }
        let [vkh, spv, vma] = [
            found[0].clone().unwrap_or_default(),
            found[1].clone().unwrap_or_default(),
            found[2].clone().unwrap_or_default(),
        ];
        p.notes.push(format!(
            "the Vulkan back end: Vulkan {}, SPIR-V {}, VMA {}",
            vkh.display(),
            spv.display(),
            vma.display()
        ));
        p.vk_headers = Some([vkh, spv, vma]);
        plan_renderer(i, &mut p, &tracy_flags, &dirs)?;
    }
    if matches!(o.mode, Mode::DryRun | Mode::ThirdPartySet(_)) {
        // (the lift's files: there is no game here)
        p.units.retain(|u| !u.source.starts_with(gen_dir));
    }
    if let Mode::ThirdPartySet(_) = o.mode {
        p.units.retain(|u| u.third_party);
    }
    Ok(p)
}

/// The files of zstd's decoder (third_party/zstd: in the runtime, and once in the renderer before zbic).
const ZSTD_FILES: [&str; 10] = [
    "common/debug.c",
    "common/entropy_common.c",
    "common/error_private.c",
    "common/fse_decompress.c",
    "common/xxhash.c",
    "common/zstd_common.c",
    "decompress/huf_decompress.c",
    "decompress/zstd_ddict.c",
    "decompress/zstd_decompress.c",
    "decompress/zstd_decompress_block.c",
];

fn zstd_units(l: &Layout, cc: &Path, flags: &[String]) -> Vec<Unit> {
    let zstd = l.r("third_party/zstd/lib");
    ZSTD_FILES
        .iter()
        .map(|f| {
            let name = file_name(Path::new(f));
            Unit {
                name: name.clone(),
                module: Module::Game,
                kind: Kind::ThirdParty,
                compiler: cc.to_path_buf(),
                source: zstd.join(f),
                object: PathBuf::from(format!("{}.o", &name[..name.len() - 2])),
                flags: flags.to_vec(),
                extra: Vec::new(),
                depfile: true,
                third_party: true,
                listed: None,
            }
        })
        .collect()
}

/// The host files that use zstd's decoder: the game's streams (hos_zstd.c) and every other read of a zstd frame
/// (zs.c: the RomFS's .zs files with the game's dictionaries, the mods' size tables, the sound archives, the heart
/// containers' maps, the prompts).
const ZSTD_HOST: [&str; 2] = ["hos_zstd.c", "zs.c"];

/// The host's C files the game compiles: host/'s own (not pgo_rt.c, the renderer's profile counters' writer, compiled
/// into the renderer alone; on Linux not the Windows window, movies and wgpu stand-in; dav1d's movies only with dav1d),
/// the game's, `game/` beside `host/` (compiled as the host's, with its flags, into the same game binary:
/// game/README.md), and on Linux the POSIX stand-ins.
pub(crate) fn host_sources(host: &Path, posix: &Path, linux: bool, dav1d: bool) -> Vec<PathBuf> {
    let mut srcs = sorted_files(host, |f| {
        f.ends_with(".c")
            && f != "pgo_rt.c"
            && !(linux && matches!(f, "display.c" | "mf_av1.c" | "rx_nowgpu.c"))
            && (f != "dav1d_av1.c" || dav1d)
    });
    if let Some(tree) = host.parent() {
        srcs.extend(sorted_files(&tree.join("game"), |f| f.ends_with(".c")));
        // (and its Insert menu rows, game/menu/: data the renderer's menu adds, through MkwVkHost's game_menu_rows)
        srcs.extend(sorted_files(&tree.join("game").join("menu"), |f| f.ends_with(".c")));
    }
    if linux {
        srcs.extend(["win32_posix.c", "display_sdl.c", "linux_stubs.c", "prof.c"].iter().map(|f| posix.join(f)));
    }
    srcs
}

fn plan_host(
    i: &Inputs,
    p: &mut Plan,
    host: &Path,
    posix: &Path,
    tracy_flags: &[String],
    ffmpeg_cflags: &[String],
    dirs: &[PathBuf],
) -> Result<()> {
    let (o, tc, l) = (i.options, i.tc, i.layout);
    let st = &o.settings;
    let linux = !tc.windows;
    let dav1d = packages::dav1d_windows(st, dirs);
    if let Some(d) = &dav1d {
        p.notes.push(format!("AV1 movies: dav1d ({})", d.display()));
    }
    p.dav1d = dav1d.clone();
    let srcs = host_sources(host, posix, linux, dav1d.is_some());
    // (the game's C, game/: its objects named game_<file>.o, beside the host's host_<file>.o)
    let game_dir = l.r("game");
    let game_menu_dir = game_dir.join("menu");
    let mut hf = [tc.target.clone(), tc.cpu.clone()].concat();
    if linux {
        hf.extend([inc(posix), "-D_GNU_SOURCE".into()]);
    }
    // (the lift's headers: a64.h with the lift's choices in a64_config.h, so the host is built as the IR was written)
    hf.extend([inc(host), inc(&l.rt)]);
    hf.extend(s(&["-Wno-everything", "-O2", "-g0", "-mcx16"]));
    if st.tracy {
        hf.extend(tracy_flags.iter().cloned());
        hf.push("-DTRACY_EXPORTS".into());
    }
    let opus_inc = inc(&l.r("third_party/opus/include"));
    let zstd_inc = inc(&l.r("third_party/zstd/lib"));
    for src in srcs {
        let name = file_name(&src);
        let prefix = match src.parent() {
            Some(d) if d == game_dir.as_path() => "game_",
            Some(d) if d == game_menu_dir.as_path() => "game_menu_",
            _ => "host_",
        };
        let obj = format!("{}{}.o", prefix, &name[..name.len() - 2]);
        let mut f = hf.clone();
        f.push(opus_inc.clone());
        if let Some(ff) = &p.ffmpeg_windows {
            f.push(inc(&ff.join("include")));
        }
        f.extend(ffmpeg_cflags.iter().cloned());
        if name == "version.c" {
            f.push(i.version.clone());
        }
        if ZSTD_HOST.contains(&name.as_str()) {
            f.push(zstd_inc.clone());
        }
        if let Some(d) = dav1d.as_ref().filter(|_| matches!(name.as_str(), "nn_video.c" | "dav1d_av1.c")) {
            f.extend(["-DLONGSHOT_DAV1D".to_string(), inc(&d.join("include"))]);
        }
        p.units.push(Unit {
            name,
            module: Module::Game,
            kind: Kind::Tree,
            compiler: tc.cc.clone(),
            source: src,
            object: l.obj.join(obj),
            flags: f,
            extra: Vec::new(),
            depfile: true,
            third_party: false,
            listed: None,
        });
    }
    Ok(())
}

/// The renderer's own files: `gpu/vk/*.cpp` (bar the NGX runtime shim), its Insert menu's modules, Eden's
/// recompiler and sirit, and the CPU texture decoders (`gpu/vk/texdecode`: ported code, its PROVENANCE.md). suyu's
/// sources (`gpu/third_party/suyu`) are not compiled: only its headers are read.
pub fn vk_sources(gpu: &Path) -> Vec<PathBuf> {
    let vk = gpu.join("vk");
    let mut out = Vec::new();
    for sub in ["eden", "sirit/src"] {
        let base = vk.join(sub);
        out.extend(
            build_core::fsx::files_under(&base)
                .unwrap_or_default()
                .into_iter()
                .filter(|r| r.extension().is_some_and(|e| e == "cpp"))
                .map(|r| base.join(r)),
        );
    }
    out.extend(sorted_files(&vk, |f| f.ends_with(".cpp") && f != NGX_MSVC_RT));
    out.extend(sorted_files(&vk.join("menu"), |f| f.ends_with(".cpp")));
    out.extend(sorted_files(&vk.join("texdecode"), |f| f.ends_with(".cpp")));
    out.sort();
    out
}

/// llvm-mingw: what NVIDIA's static library asks of Microsoft's C++ runtime.
const NGX_MSVC_RT: &str = "vk_ngx_msvc_rt.cpp";

fn plan_renderer(i: &Inputs, p: &mut Plan, tracy_flags: &[String], _dirs: &[PathBuf]) -> Result<()> {
    let (o, tc, l) = (i.options, i.tc, i.layout);
    let st = &o.settings;
    let linux = !tc.windows;
    let gpu = l.r("gpu");
    let vk = gpu.join("vk");
    let Some([vkh, spv, vma]) = p.vk_headers.clone() else { return Ok(()) };
    let pic: Vec<String> = if linux { s(&["-fPIC"]) } else { vec![] };
    let mut vf = [tc.target.clone(), tc.cpu.clone()].concat();
    let head = vf.len(); // (what follows the target and the CPU: the third-party parts' flags too)
    if linux {
        vf.extend(["-fPIC".to_string(), inc(&l.r("host/posix")), "-D_GNU_SOURCE".into()]);
    }
    // (what the code takes for granted, which newer C++ libraries bring in by themselves and older ones do not: issue 48)
    for h in ["algorithm", "cstdio", "cstdlib", "cstring", "functional", "string", "vector"] {
        vf.extend(["-include".to_string(), h.to_string()]);
    }
    vf.extend(s(&[
        "-std=c++20",
        "-O2",
        "-g0",
        "-DNDEBUG",
        "-Wno-everything",
        "-DVK_NO_PROTOTYPES",
        "-DMKWVK_BUILDING",
    ]));
    if st.release {
        vf.push("-DLONGSHOT_RELEASE".into()); // (the shaders prepared before the game)
    }
    // (Eden's recompiler before suyu's root: suyu has its own shader_recompiler/ headers, and a Shader::Info of the other
    // layout in some objects crashed the descriptor walk. The order is in the key, with the -I flags)
    for d in
        [vk.join("compat"), gpu.join("compat"), vk.join("eden"), gpu.join("third_party/suyu"), vk.join("sirit/include")]
    {
        vf.push(inc(&d));
    }
    vf.push(inc(&vk.join("sirit/src")));
    vf.extend([inc(&spv.join("include")), inc(&spv.join("include/spirv/unified1")), inc(&vkh.join("include"))]);
    vf.extend([inc(&vma.join("include")), inc(&vk), inc(&l.made), inc(&l.r("third_party/zbic"))]);
    // (the third-party parts: the same, at the third-party CPU level, and without the release's define, which none of
    // them reads, so a set made for players fits a developer's build too)
    let mut tpf = [tc.target.clone(), tc.third_party_cpu.clone()].concat();
    tpf.extend(vf[head..].iter().filter(|f| *f != "-DLONGSHOT_RELEASE").cloned());
    let ngx = packages::ngx_sdk(st);
    if let Some(why) = ngx.1 {
        p.notes.push(why);
    }
    p.ngx = ngx.0.clone();
    let ngx_flags: Vec<String> = match &p.ngx {
        Some(sdk) => {
            p.notes.push(format!("the Vulkan back end: with NVIDIA DLSS (NVIDIA's NGX SDK in {})", sdk.display()));
            vec!["-DLONGSHOT_NGX".into(), inc(&sdk.join("include"))]
        }
        None => vec![],
    };
    let fsr_flags: Vec<String> = if st.fsr {
        p.notes.push("the Vulkan back end: with AMD FSR 4 (AMD's FSR API headers, third_party/fidelityfx)".into());
        vec!["-DLONGSHOT_FSR".into(), inc(&l.r("third_party/fidelityfx"))]
    } else {
        vec![]
    };
    // (Intel XeSS in every Windows build: it loads the player's own libxess.dll at run time, never shipped; Intel ships
    // no Linux library, so there the stubs say XeSS is Windows only)
    let xess_flags: Vec<String> =
        if linux { vec![] } else { vec!["-DLONGSHOT_XESS".into(), inc(&l.r("third_party/xess/inc"))] };
    let imgui = l.r("third_party/imgui");
    let mut imgui_flags = vec![inc(&imgui)];
    imgui_flags.extend(s(&[
        "-DIMGUI_IMPL_VULKAN_NO_PROTOTYPES",
        "-DIMGUI_DISABLE_DEFAULT_SHELL_FUNCTIONS",
        "-DIMGUI_DISABLE_WIN32_FUNCTIONS",
        "-DIMGUI_DISABLE_DEMO_WINDOWS",
        "-DIMGUI_DISABLE_DEBUG_TOOLS",
    ]));
    let imgui_users = ["vk_imgui.cpp", "vk_menu.cpp", "reshade.cpp"];
    let vu = |name: String, kind, cc: &Path, src: PathBuf, obj: String, flags: Vec<String>, tp: bool| Unit {
        name,
        module: Module::Renderer,
        kind,
        compiler: cc.to_path_buf(),
        source: src,
        object: l.obj.join(obj),
        flags,
        extra: Vec::new(),
        depfile: true,
        third_party: tp,
        listed: None,
    };
    let (cc, cxx) = (&tc.cc, &tc.cxx);
    for f in ["imgui.cpp", "imgui_draw.cpp", "imgui_tables.cpp", "imgui_widgets.cpp", "backends/imgui_impl_vulkan.cpp"]
    {
        let name = file_name(Path::new(f));
        let obj = format!("vk_imgui_{}.o", &name[..name.len() - 4]);
        p.units.push(vu(
            name,
            Kind::ThirdParty,
            cxx,
            imgui.join(f),
            obj,
            [tpf.clone(), imgui_flags.clone()].concat(),
            true,
        ));
    }
    let mut ff = tc.target.clone();
    ff.extend(pic.iter().cloned());
    ff.extend(s(&["-O0", "-g0", "-w"]));
    p.units.push(vu(
        "ui_fonts.cpp".into(),
        Kind::Generated,
        cxx,
        l.made.join("ui_fonts.cpp"),
        "vk_ui_fonts.o".into(),
        ff,
        false,
    ));
    let mut pgo_flags = Vec::new();
    let mut pgo_extra = Vec::new();
    match &i.vk_pgo {
        Pgo::Off => {}
        Pgo::Generate => {
            pgo_flags = s(&["-fprofile-instr-generate", "-mllvm", "-disable-vp=true"]);
            p.notes.push("the Vulkan back end: counting its branches for a profile (RX_VK_PGO=gen; RX_VK_PGO_FILE=<file> in a run)".into());
        }
        Pgo::Use(prof) => {
            let bytes = std::fs::read(prof).at(prof)?;
            pgo_flags.push(format!("-fprofile-instr-use={}", prof.display()));
            pgo_extra.push(("profile".to_string(), sha1_smol::Sha1::from(&bytes).digest().to_string().into_bytes()));
            p.notes.push(format!("the Vulkan back end: built with the profile {}", prof.display()));
        }
    }
    if st.tracy {
        pgo_flags.extend(tracy_flags.iter().cloned());
        pgo_flags.push("-DTRACY_IMPORTS".into());
        p.notes.push("the Vulkan back end: with the Tracy profiler's zones (LONGSHOT_TRACY)".into());
    }
    let shader_stamp = format!("-DLONGSHOT_SHADER_STAMP=0x{}ull", i.shader_stamp.clone().unwrap_or_default());
    for src in vk_sources(&gpu) {
        let rel = src.strip_prefix(&vk).map(|r| r.to_string_lossy().replace('\\', "/"));
        let (name, lib) = match &rel {
            Ok(r) => (r.trim_end_matches(".cpp").replace('/', "_"), r.starts_with("eden/") || r.starts_with("sirit/")),
            Err(_) => (file_name(&src).trim_end_matches(".cpp").to_string(), false),
        };
        let base = file_name(&src);
        if lib {
            p.units.push(vu(base, Kind::ThirdParty, cxx, src, format!("vk_{name}.o"), tpf.clone(), true));
            continue;
        }
        let mut f = [vf.clone(), pgo_flags.clone()].concat();
        match base.as_str() {
            "vk_shader.cpp" => f.push(shader_stamp.clone()),
            "vk_version.cpp" => f.push(i.version.clone()),
            "vk_dlss.cpp" => f.extend(ngx_flags.iter().cloned()),
            "vk_fsr4.cpp" => f.extend(fsr_flags.iter().cloned()),
            "vk_xess.cpp" => f.extend(xess_flags.iter().cloned()),
            _ => {}
        }
        if imgui_users.contains(&base.as_str()) {
            f.extend(imgui_flags.iter().cloned());
        }
        let mut u = vu(base, Kind::Tree, cxx, src, format!("vk_{name}.o"), f, false);
        u.extra = pgo_extra.clone();
        p.units.push(u);
    }
    // (ReShade's FX compiler: the upstream files are third-party; the adapted preprocessor is the build's own)
    let upstream = l.r("third_party/reshade_fx/source");
    let bridge = vk.join("reshade");
    let fx_inc = [inc(&upstream), inc(&bridge)];
    let mut fx_vendor = [tpf.clone(), fx_inc.to_vec(), s(&["-fno-char8_t"])].concat();
    if !linux {
        fx_vendor.extend(s(&["-include", "share.h"]));
    }
    for name in ["codegen_spirv", "expression", "lexer", "parser_exp", "parser_stmt", "preprocessor", "symbol_table"] {
        let (src, kind, tp) = if name == "preprocessor" {
            (l.made.join("reshade_preprocessor.cpp"), Kind::Generated, false)
        } else {
            (upstream.join(format!("effect_{name}.cpp")), Kind::ThirdParty, true)
        };
        let mut f = fx_vendor.clone();
        if name == "symbol_table" {
            f.push("-O0".into()); // (optimising the generated intrinsics table dominates the build; FX compiles on a worker)
        }
        p.units.push(vu(file_name(&src), kind, cxx, src, format!("vk_reshade_vendor_{name}.o"), f, tp));
    }
    for name in ["compile", "files", "pipeline_cache", "preset", "runtime", "texture"] {
        let mut f = [vf.clone(), pgo_flags.clone(), fx_inc.to_vec()].concat();
        if name == "texture" {
            f.push(inc(&l.r("third_party/stb_image")));
        }
        let src = bridge.join(format!("fx_{name}.cpp"));
        let mut u = vu(file_name(&src), Kind::Tree, cxx, src, format!("vk_reshade_fx_{name}.o"), f, false);
        u.extra = pgo_extra.clone();
        p.units.push(u);
    }
    // (the program cache's zstd: third_party/zbic's single file, as C into the same module)
    let zbic = l.r("third_party/zbic");
    let mut zf = [tc.target.clone(), tc.third_party_cpu.clone(), pic.clone()].concat();
    zf.extend(s(&["-O2", "-g0", "-DNDEBUG", "-Wno-everything", "-fvisibility=hidden"]));
    zf.push(inc(&zbic));
    p.units.push(vu("zstd.c".into(), Kind::ThirdParty, cc, zbic.join("zstd.c"), "vk_zstd.o".into(), zf, true));
    if p.ngx.is_some() && tc.mingw.is_some() {
        // (NVIDIA's library was built with Microsoft's compiler for its static runtime: what it asks of that runtime)
        let f = [vf.clone(), ngx_flags.clone()].concat();
        p.units.push(vu(
            NGX_MSVC_RT.into(),
            Kind::Tree,
            cxx,
            vk.join(NGX_MSVC_RT),
            "vk_ngx_msvc_rt.o".into(),
            f,
            false,
        ));
        let mut f = tc.target.clone();
        f.extend(s(&["-O2", "-g0", "-Wno-everything"]));
        let src = l.r("host/mingw_msvc_shims.c");
        p.units.push(vu(
            "mingw_msvc_shims.c".into(),
            Kind::Tree,
            cc,
            src,
            "vk_ngx_mingw_msvc_shims.o".into(),
            f,
            false,
        ));
    }
    let posix = l.r("host/posix");
    if linux {
        // (the module's own copy of the Win32 calls on POSIX)
        let mut f = [tc.target.clone(), tc.cpu.clone()].concat();
        f.extend(s(&["-fPIC", "-O2", "-g0", "-Wno-everything", "-D_GNU_SOURCE"]));
        f.push(inc(&posix));
        p.units.push(vu(
            "win32_posix.c".into(),
            Kind::Tree,
            cc,
            posix.join("win32_posix.c"),
            "vk_posix_win32_posix.o".into(),
            f,
            false,
        ));
    }
    if i.vk_pgo == Pgo::Generate {
        // (the counters' writer, the module's own copy: the sections it reads are the module's)
        let mut f = tc.target.clone();
        if linux {
            f.extend(tc.cpu.iter().cloned());
            f.extend(["-fPIC".to_string(), "-D_GNU_SOURCE".into(), inc(&posix)]);
        }
        f.extend(s(&["-O2", "-g0", "-Wno-everything", "-DPGO_ENV=\"RX_VK_PGO_FILE\""]));
        p.units.push(vu("pgo_rt.c".into(), Kind::Tree, cc, l.r("host/pgo_rt.c"), "vk_pgo_rt.o".into(), f, false));
    }
    Ok(())
}

/// The lift's C files in gen/ (the function table, the import stubs, the boot constants, the runtime).
pub const LIFT_C: [&str; 5] = ["table.c", "imports.c", "image.c", "rt/a64_rt.c", "rt/a64_fp.c"];

/// The units `gen/units.txt` lists (`<file> <compile cost> <bytes> <key> <content sha1> <functions>` a line), by name:
/// those alone are the game's, whatever other `.ll` files are there. A gen/ without the list was not lifted.
pub fn lifted_units(gen_dir: &Path) -> Result<Vec<(String, Listed)>> {
    let path = gen_dir.join("units.txt");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::player(
                "lift_missing",
                format!("The translated game is not in {} (no units.txt): translate it first.", gen_dir.display()),
            ));
        }
        Err(e) => return Err(Error::io(&path, e)),
    };
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split(' ').collect();
        let bad = || Error::format(path.display().to_string(), format!("not a unit line: {line:?}"));
        let [file, cost, bytes, _key, sha1, _functions] = f[..] else { return Err(bad()) };
        if !file.ends_with(".ll") || file.contains(['/', '\\']) {
            return Err(bad());
        }
        let (Ok(cost), Ok(bytes)) = (cost.parse(), bytes.parse()) else { return Err(bad()) };
        out.push((file.to_string(), Listed { sha1: sha1.to_string(), bytes, cost }));
    }
    // (in name order: the link's, the same from build to build whatever the costs)
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The flags of a lifted IR unit: the target's and the CPU's, and the IR level. IR includes nothing and defines
/// nothing, so only what reaches code generation; LoopRotate's header copies capped, and capture tracking past LLVM's
/// 100 uses (the context parameter is noescape).
///
/// And two caps on LLVM's time over the lift's biggest functions, which set a build's critical path:
/// - `-cgpp-huge-func=1000`: CodeGenPrepare restarts its walk over a function's blocks after each change to the
///   dominator tree unless the function has 10,000 blocks or more. On an interpreter's dispatch of 4,000 blocks (a
///   jump table of 1,953 cases) that was 145 s of a 150 s unit; with the walk going on, 1 s, and the same object.
/// - `-grow-region-complexity-budget=1000`: greedy register allocation's region growth. On a player's CPU without
///   AVX-512 (16 vector registers: most Intel desktops, AMD before Zen 4; `-march=x86-64-v3` too) one float-heavy
///   function took 23 minutes in the register allocator, the whole build waiting on it; 14 s capped.
pub(crate) fn ir_unit_flags(base: &[String], level: Level) -> Vec<String> {
    let mut f = base.to_vec();
    f.extend(s(&["-Wno-everything", "-mcx16", level.flag(), "-g0", "-ffunction-sections"]));
    f.extend(s(&["-mllvm", "-rotation-max-header-size=0", "-mllvm", "-capture-tracking-max-uses-to-explore=5000"]));
    f.extend(s(&["-mllvm", "-cgpp-huge-func=1000", "-mllvm", "-grow-region-complexity-budget=1000"]));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ir_units_cap_llvm_s_time_on_giant_functions() {
        // (without them, one unit took 150 s (CodeGenPrepare) and another 23 minutes (register allocation, on a CPU
        // without AVX-512): ir_unit_flags' docs)
        let f = ir_unit_flags(&s(&["-march=x86-64-v3"]), Level::O1);
        for cap in ["-cgpp-huge-func=1000", "-grow-region-complexity-budget=1000"] {
            let at = f.iter().position(|x| x == cap).unwrap_or_else(|| panic!("{cap} in {f:?}"));
            assert_eq!(f[at - 1], "-mllvm", "{f:?}");
        }
        assert!(f.starts_with(&s(&["-march=x86-64-v3"])) && f.contains(&"-O1".to_string()));
    }

    #[test]
    fn the_units_are_those_units_txt_lists() {
        let d = crate::testdir("units");
        assert!(matches!(lifted_units(&d), Err(Error::Player { code: "lift_missing", .. })));
        std::fs::write(
            d.join("units.txt"),
            "# longshot-builder units 1: ...\nlifted_hot_80000010.ll 9 20 k1 s1 1\nlifted_80000000.ll 5 10 k2 s2 3\n",
        )
        .unwrap();
        let u = lifted_units(&d).unwrap();
        assert_eq!(
            u.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ["lifted_80000000.ll", "lifted_hot_80000010.ll"]
        );
        assert_eq!(u[0].1, Listed { sha1: "s2".into(), bytes: 10, cost: 5 });
        std::fs::write(d.join("units.txt"), "../x.ll 1 1 k s 1\n").unwrap();
        assert!(lifted_units(&d).is_err());
    }

    #[test]
    fn the_renderer_compiles_the_cpu_texture_decoders_and_none_of_suyu_s_sources() {
        // (main's #2307: the CPU decoders are texdecode's, compiled as the renderer's own files with its profile, and
        // suyu's decoders.cpp and astc.cpp leave the build)
        let gpu = crate::testdir("vk-sources");
        for f in [
            "vk/vk_draw.cpp",
            "vk/vk_ngx_msvc_rt.cpp",
            "vk/menu/threads.cpp",
            "vk/texdecode/astc.cpp",
            "vk/texdecode/block_linear.cpp",
            "vk/texdecode/texdecode.h",
            "vk/eden/a/b.cpp",
            "vk/sirit/src/c.cpp",
            "third_party/suyu/video_core/textures/decoders.cpp",
            "third_party/suyu/video_core/textures/astc.cpp",
        ] {
            let p = gpu.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let got: Vec<String> = vk_sources(&gpu)
            .iter()
            .map(|p| p.strip_prefix(&gpu).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            got,
            [
                "vk/eden/a/b.cpp",
                "vk/menu/threads.cpp",
                "vk/sirit/src/c.cpp",
                "vk/texdecode/astc.cpp",
                "vk/texdecode/block_linear.cpp",
                "vk/vk_draw.cpp"
            ]
        );
        // (the checkout's own: every texdecode source in it, none of suyu's, no two units of one object)
        let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../gpu");
        let list = vk_sources(&real);
        for f in sorted_files(&real.join("vk/texdecode"), |f| f.ends_with(".cpp")) {
            assert!(list.contains(&f), "{} is not compiled", f.display());
        }
        assert!(
            list.iter().any(|f| f.ends_with("texdecode/astc.cpp"))
                && list.iter().any(|f| f.ends_with("texdecode/block_linear.cpp"))
        );
        assert!(!list.iter().any(|f| f.starts_with(real.join("third_party"))), "a gpu/third_party source is compiled");
        let mut objects: Vec<String> = list
            .iter()
            .map(|f| f.strip_prefix(real.join("vk")).unwrap().to_string_lossy().replace(['/', '\\'], "_"))
            .collect();
        let n = objects.len();
        objects.sort();
        objects.dedup();
        assert_eq!(objects.len(), n, "two sources make one object");
    }
}

#[cfg(test)]
#[path = "host_sources_tests.rs"]
mod host_sources_tests;
