//! `longshot-builder from-source`: a version folder made on this PC from any commit's source (a pull request's head,
//! main's newest commit), as a release's is made in CI (docs/dev/reference/releases.md, "A builder from source"):
//!
//! 1. **The source** ([`source`]): Forgejo's archive of the commit, by its full hash (`--commit`), or a file
//!    (`--archive`), checked (it names that commit; plain files inside its folder only) and unpacked into the cache.
//! 2. **Rust** ([`rust`]): the version and the parts the source pins (scripts/renhdrum.py, [`pins`]), each checked by
//!    its SHA-256, installed once in `<cache>/rust/`.
//! 3. **The crates** ([`cargo::fetch`]): `cargo fetch --locked`, its Cargo.lock's, into `<cache>/cargo/`.
//! 4. **The build tools** ([`tools`]): the source's tools.lock's for this system, the running builder's own when they
//!    are the same, else downloaded and checked, kept by their SHA-256.
//! 5. **What the builder carries**: RenHDrum's library compiled from the source (or the running builder's when this
//!    PC has no archiver for its C), the third-party objects compiled by this builder from the source's tree, and the
//!    pack (build_driver::sources' writer: the files the source's own `TREE` names).
//! 6. **The builder**: `cargo build --profile dist -p longshot-builder` with the release's flags, at a low priority
//!    and with as many jobs as the memory holds, the pack appended to it (build_driver::sources::attach; a source from
//!    before the pack was appended embeds it as it compiles, `LONGSHOT_SOURCES`); checked to read its pack; then
//!    `<out>/Longshot.x86_64` (`Longshot.exe`) and `<out>/source.json`, which says the commit and that it is unsigned.
//!
//! Every step says itself as build events (`stage`, `progress`, `log`), the last two `from_source` and `done`, or an
//! `error`. Killed at any point, it leaves nothing half made in use: downloads and unpacks go through `.part` names,
//! and each cache is kept for the next run (the source by its commit, Rust by its pins, the crates, the tools by their
//! SHA-256, the compiled crates in `<cache>/from-source/target`, the third-party objects' store).

pub mod cargo;
pub mod pins;
pub mod rust;
pub mod source;
pub mod tools;

use build_core::report::{Event, Reporter, Stage};
use build_core::{Error, Result};
use std::path::{Path, PathBuf};

/// What `from-source` was asked.
#[derive(Clone, Debug, Default)]
pub struct Asked {
    /// `--archive <file>`: the commit's archive, given.
    pub archive: Option<PathBuf>,
    /// `--commit <40 hex>`: the commit, downloaded (with `--archive`: the commit it must be).
    pub commit: Option<String>,
    /// `--url <base>`: the repository whose archive is downloaded ([`source::DEFAULT_URL`]).
    pub url: Option<String>,
    /// `--out <folder>`: the version folder.
    pub out: PathBuf,
    /// `--name <version>`: the name it carries (`source-<commit's first 8>`).
    pub name: Option<String>,
    /// `--jobs N`.
    pub jobs: Option<usize>,
}

/// The program players start in a version folder, on this system (play.rs's NAMES).
pub fn program_name() -> &'static str {
    if cfg!(windows) { "Longshot.exe" } else { "Longshot.x86_64" }
}

/// The version folder's note of where its program came from.
pub const NOTE: &str = "source.json";
/// The marker beside an unpacked source, naming its commit.
const SOURCE_MARK: &str = ".from-source-commit";
/// The memory one of cargo's compilers takes, about (most crates take far less; the biggest and the fat LTO link,
/// which runs alone at the end, more): cargo's jobs are as many as the memory free holds of these.
const COMPILER_MB: u64 = 1024;

/// Whether `name` may be a version's name: 1 to 64 of `[A-Za-z0-9._-]`, a letter or digit first.
pub fn name_ok(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=64).contains(&b.len())
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(c))
}

/// The source's files this builder needs before it builds anything: their absence says the commit has no
/// longshot-builder (it is older than it, built by make_game.py).
const NEEDED: [&str; 5] =
    ["Cargo.toml", "Cargo.lock", "tools.lock", "scripts/renhdrum.py", "builder/crates/longshot-builder/Cargo.toml"];

/// `from-source` as `asked`, said to `r`: the version folder, or why not.
pub fn run(asked: &Asked, r: &Reporter) -> Result<()> {
    let started = std::time::Instant::now();
    build_core::lower_priority();
    build_core::stay_awake();
    if !cfg!(target_arch = "x86_64") {
        return Err(Error::player("source_unreadable", "A builder is built from source on x86-64 PCs only."));
    }
    let cache = build_driver::layout::cache_root(build_driver::env_os);
    // (absolute: cargo and the compile run in other folders)
    let cache = std::path::absolute(&cache).map_err(|e| Error::io(&cache, e))?;
    let fs = cache.join("from-source");
    let _lock = build_core::lock::WorkLock::take(&fs)?;
    let out = std::path::absolute(&asked.out).map_err(|e| Error::io(&asked.out, e))?;
    let mut downloaded = 0u64;
    let say = |s: Stage, text: &str| r.stage(s, text);
    // (each step's time, in the log: the docs' measures)
    let mut lap = std::time::Instant::now();
    let mut took = |what: &str| {
        r.log(format!("{what}: {:.1} s", lap.elapsed().as_secs_f64()));
        lap = std::time::Instant::now();
    };

    // ---- the source ----
    say(Stage::Source, "Getting the commit's source");
    let (archive, from) = match (&asked.archive, &asked.commit) {
        (Some(a), _) => (std::path::absolute(a).map_err(|e| Error::io(a, e))?, "archive".to_string()),
        (None, Some(c)) => {
            let url = source::archive_url(asked.url.as_deref().unwrap_or(source::DEFAULT_URL), c)
                .map_err(|e| Error::player("usage", e))?;
            // (kept by the commit and where it came from: an archive another --url served, whatever commit its comment
            // names, is never taken for the forge's)
            let origin = &build_driver::sha256_text(&url)[..16];
            let to = fs.join("archives").join(format!("{}-{origin}.tar.gz", c.to_ascii_lowercase()));
            let had = to.is_file();
            let n = source::download(&url, &to)?;
            if !had {
                downloaded += n;
            }
            r.log(format!("source: {url}, {} KB{}", n >> 10, if had { ", kept from before" } else { "" }));
            (to, url)
        }
        (None, None) => return Err(Error::player("usage", "from-source needs --archive <file> or --commit <40 hex>.")),
    };
    r.progress(0.5);
    let src = fs.join("src");
    let archive_sha256 = build_driver::sha256_file(&archive).map(|(s, _)| s).unwrap_or_default();
    let commit = unpack_source(&archive, &archive_sha256, asked.commit.as_deref(), &src, r)?;
    let version = asked.name.clone().unwrap_or_else(|| format!("source-{}", &commit[..8]));
    r.progress(1.0);
    took("time: the source");
    for f in NEEDED {
        if !src.join(f).is_file() {
            return Err(Error::player(
                "no_builder_in_source",
                format!(
                    "Commit {} has no longshot-builder ({f} is not in it): it is older than the builder that builds \
                     from source, and can't be built by it.",
                    &commit[..8]
                ),
            ));
        }
    }
    let program = out.join(program_name());
    if let Some(done) = built_already(&out, &commit, &from, &version) {
        r.log(format!("{} is built from commit {} already: nothing to do", program.display(), &commit[..8]));
        return finish(r, &out, &program, &commit, &version, 0, started, done);
    }

    // ---- Rust ----
    let read = |rel: &str| std::fs::read_to_string(src.join(rel)).map_err(|e| Error::io(src.join(rel), e));
    let pins = pins::rust_pins(&read("scripts/renhdrum.py")?).map_err(|e| Error::player("source_unreadable", e))?;
    let parts = pins.for_host(cfg!(windows)).map_err(|e| Error::player("source_unreadable", e))?;
    say(Stage::Rust, &format!("Getting Rust {}", pins.version));
    if let Some(m) = read("mise.toml").ok().and_then(|t| pins::mise_rust(&t)).filter(|m| *m != pins.version) {
        r.log(format!(
            "rust: mise.toml pins {m}, scripts/renhdrum.py {}: the release's, {}, is taken",
            pins.version, pins.version
        ));
    }
    let (rust_dir, n) = rust::install(&cache.join("rust"), &pins.version, &parts, &|f| r.progress(f))?;
    downloaded += n;
    r.log(format!("rust: {} ({} MB downloaded now)", rust_dir.display(), n >> 20));
    took("time: Rust");

    // ---- the build tools (before the crates: the linker is theirs) ----
    say(Stage::SourceTools, "Getting the build tools");
    let (carried, carried_tree) = tools::carried(&cache)?;
    let (n, said) = tools::install(&src, &carried, &fs, &|f| r.progress(f))?;
    downloaded += n;
    for line in said {
        r.log(format!("tool {line}"));
    }
    let tc = toolchain(&rust_dir, &cache.join("cargo"), &src, &fs)?;
    took("time: the build tools");

    // ---- the crates ----
    say(Stage::Crates, "Getting the crates");
    let n = cargo::fetch(&tc, &src)?;
    downloaded += n;
    r.log(format!("crates: {} MB downloaded now", n >> 20));
    r.progress(1.0);
    took("time: the crates");

    // ---- what the builder carries ----
    say(Stage::Pack, "Making what the builder carries");
    let jobs = build_core::jobs::chosen(asked.jobs, build_driver::env("LONGSHOT_JOBS").as_deref())?;
    let cargo_jobs = build_core::memory::Gate::from_env().expected(jobs, COMPILER_MB);
    r.log(format!("jobs: {jobs}, {cargo_jobs} for cargo (as many compilers as the memory free holds)"));
    let (renhdrum, n) = renhdrum(&src, &fs, &tc, carried_tree.as_deref(), cargo_jobs, r)?;
    downloaded += n;
    r.progress(0.2);
    took("time: RenHDrum");
    let objects = third_party_objects(&src, &fs, &cache, jobs, r)?;
    took("time: the third-party objects");
    let target = fs.join("target");
    let game_rust = build_game_rust(&src, &tc, &target, cargo_jobs, r)?;
    let pack = fs.join("sources.pack");
    let w = make_pack(&src, &version, &objects, renhdrum.as_deref(), game_rust.as_deref(), &pack)?;
    took("time: the pack");
    r.log(format!(
        "pack: {} members, {} files, {} MB, {} MB compressed",
        w.members,
        w.files,
        w.raw >> 20,
        w.size >> 20
    ));
    r.progress(1.0);

    // ---- the builder ----
    say(Stage::Builder, "Compiling the builder");
    let artifact =
        cargo::out_dir(&target, "dist").join(if cfg!(windows) { "longshot-builder.exe" } else { "longshot-builder" });
    let whole = cargo::units(&tc, &src, "longshot-builder");
    let built = cargo::build(
        &tc,
        &src,
        &["--profile", "dist", "-p", "longshot-builder", "--bin", "longshot-builder"],
        &cargo::release_flags(&src, &tc.home),
        // (a source from before the pack was appended after the link embeds it as it compiles; a newer one ignores it)
        &[("LONGSHOT_SOURCES".into(), pack.display().to_string())],
        cargo_jobs,
        &target,
        &artifact,
        whole,
        "The source's longshot-builder",
        r,
        &|f| r.progress(f * 0.95),
    )?;
    took("time: the builder's compile");
    let file = if check_reads_its_pack(&built.file, &version).is_ok() {
        // (an older source's, its pack embedded)
        built.file.clone()
    } else {
        let with = fs.join(if cfg!(windows) { "longshot-builder-with-pack.exe" } else { "longshot-builder-with-pack" });
        attach(&built.file, &pack, &with)?;
        check_reads_its_pack(&with, &version)?;
        with
    };
    let program_sha256 = install(&file, &program)?;
    let note = serde_json::json!({
        "v": 1,
        "commit": commit,
        "version": version,
        "signed": false,
        "from": from,
        "archive_sha256": archive_sha256,
        "rust": pins.version,
        "built_by": crate::version_line(build_driver::sources::embedded()),
        "program": program_name(),
        "program_sha256": program_sha256,
        "made": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    });
    let text = serde_json::to_string_pretty(&note).unwrap_or_default() + "\n";
    build_core::fsx::write_atomic(&out.join(NOTE), text.as_bytes())?;
    r.progress(1.0);
    finish(r, &out, &program, &commit, &version, downloaded, started, built.seconds)
}

#[allow(clippy::too_many_arguments, reason = "the two last events' fields")]
fn finish(
    r: &Reporter,
    out: &Path,
    program: &Path,
    commit: &str,
    version: &str,
    downloaded: u64,
    started: std::time::Instant,
    compile_seconds: f64,
) -> Result<()> {
    r.send(Event::FromSource {
        folder: out.display().to_string(),
        program: program.display().to_string(),
        commit: commit.to_string(),
        version: version.to_string(),
        signed: false,
        downloaded,
    });
    r.send(Event::Done {
        folder: out.display().to_string(),
        seconds: started.elapsed().as_secs_f64(),
        compile_seconds,
        previous_seconds: None,
    });
    Ok(())
}

/// The version folder's program when it is built from `commit` (fetched from `from`) as `version` already and is still the bytes its note
/// names: the seconds its compile took (0: not known).
fn built_already(out: &Path, commit: &str, from: &str, version: &str) -> Option<f64> {
    let note: serde_json::Value = serde_json::from_slice(&std::fs::read(out.join(NOTE)).ok()?).ok()?;
    let same = note["commit"] == commit
        && note["from"] == from
        && note["version"] == version
        && note["program"] == program_name();
    let sha = note["program_sha256"].as_str()?;
    (same && build_driver::sha256_file(&out.join(program_name())).ok()?.0 == sha).then_some(0.0)
}

/// The archive's source at `src` (its commit, checked against `want`): unpacked unless the folder holds that commit's
/// already, unpacked from these very bytes (`archive_sha256`), whole (its mark, written last).
fn unpack_source(archive: &Path, archive_sha256: &str, want: Option<&str>, src: &Path, r: &Reporter) -> Result<String> {
    if let Some(w) = want {
        let mark = std::fs::read_to_string(src.join(SOURCE_MARK)).unwrap_or_default();
        let mut parts = mark.split_whitespace();
        if parts.next().is_some_and(|c| c.eq_ignore_ascii_case(w)) && parts.next() == Some(archive_sha256) {
            r.log(format!("source: commit {} unpacked already", &w[..8]));
            return Ok(w.to_ascii_lowercase());
        }
    }
    let _ = std::fs::remove_file(src.join(SOURCE_MARK));
    let u = source::unpack(archive, want, src)?;
    std::fs::write(src.join(SOURCE_MARK), format!("{} {archive_sha256}\n", u.commit)).map_err(|e| Error::io(src, e))?;
    r.log(format!("source: commit {}, {} files, {} MB", u.commit, u.files, u.bytes >> 20));
    Ok(u.commit)
}

/// The toolchain cargo runs with: the Rust at `rust`, the cargo home `home`, and the source's tools as the C compiler
/// and linker (Linux: a script in `fs`; Windows: llvm-mingw's clang).
fn toolchain(rust: &Path, home: &Path, src: &Path, fs: &Path) -> Result<cargo::Toolchain> {
    let tools = src.join("tools");
    let missing = |what: &Path| {
        Error::player("tools_download_failed", format!("The source's build tools have no {}.", what.display()))
    };
    #[cfg(windows)]
    {
        let _ = fs;
        let bin = tools.join("llvm-mingw").join("bin");
        let clang = bin.join("clang.exe");
        if !clang.is_file() {
            return Err(missing(&clang));
        }
        Ok(cargo::Toolchain { rust: rust.to_path_buf(), home: home.to_path_buf(), linker: clang, path: vec![bin] })
    }
    #[cfg(not(windows))]
    {
        let (llvm, sysroot) = (tools.join("llvm"), tools.join("sysroot"));
        for need in [llvm.join("bin").join("clang"), sysroot.join("usr").join("lib")] {
            if !need.exists() {
                return Err(missing(&need));
            }
        }
        let linker = fs.join("bin").join("cc");
        cargo::linker_script(&linker, &llvm, &sysroot)?;
        Ok(cargo::Toolchain { rust: rust.to_path_buf(), home: home.to_path_buf(), linker, path: vec![] })
    }
}

/// An archiver for RenHDrum's C (libopus, by the cc crate): the source's tools' `llvm-ar` when they have one, else this
/// PC's (`llvm-ar`, `ar` on the PATH).
fn archiver(src: &Path) -> Option<PathBuf> {
    let x = if cfg!(windows) { ".exe" } else { "" };
    let tools = ["llvm", "llvm-mingw"].map(|t| src.join("tools").join(t).join("bin").join(format!("llvm-ar{x}")));
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = ["llvm-ar", "ar"]
        .iter()
        .flat_map(|n| std::env::split_paths(&path).map(move |d| d.join(format!("{n}{x}"))))
        .collect::<Vec<_>>();
    tools.into_iter().chain(on_path).find(|p| p.is_file())
}

/// RenHDrum's library (renhdrum.so, renhdrum.dll), compiled from the source's third_party/renhdrum and libopus
/// (scripts/renhdrum.py's build, ported: its tracked files copied out of the tree, libopus from third_party/opus as
/// its vendor/opus), kept by the hash of what it is made from; with no archiver on this PC, the running builder's own
/// library (said); else none (said: the game plays its controllers' motors without it). The bytes downloaded.
fn renhdrum(
    src: &Path,
    fs: &Path,
    tc: &cargo::Toolchain,
    carried_tree: Option<&Path>,
    jobs: usize,
    r: &Reporter,
) -> Result<(Option<PathBuf>, u64)> {
    let name = if cfg!(windows) { "renhdrum.dll" } else { "renhdrum.so" };
    let crate_src = src.join("third_party").join("renhdrum");
    if !crate_src.join("Cargo.toml").is_file() {
        r.log("renhdrum: not in this source");
        return Ok((None, 0));
    }
    let Some(ar) = archiver(src) else {
        let theirs = carried_tree.map(|t| t.join("third_party").join("renhdrum").join(name)).filter(|p| p.is_file());
        r.warn(
            "renhdrum_not_built",
            format!(
                "RenHDrum (some controllers' HD rumble) was not compiled from this source: this PC has no archiver for \
                 its C code (llvm-ar or ar). {}",
                if theirs.is_some() {
                    "The running builder's own library goes in its place."
                } else {
                    "The game plays every controller's motors without it."
                }
            ),
        );
        return Ok((theirs, 0));
    };
    // (what it is made from: its files, libopus's, the Rust and the C compiler)
    let opus = src.join("third_party").join("opus");
    let mut inputs: Vec<PathBuf> = ["Cargo.toml", "Cargo.lock", "build.rs"].iter().map(|f| crate_src.join(f)).collect();
    for (dir, sub) in [(&crate_src, "src"), (&opus, "celt"), (&opus, "silk"), (&opus, "src"), (&opus, "include")] {
        inputs.extend(build_core::fsx::files_under(&dir.join(sub))?.into_iter().map(|f| dir.join(sub).join(f)));
    }
    inputs.push(opus.join("COPYING"));
    let mut h = <sha2::Sha256 as sha2::Digest>::new();
    for f in &inputs {
        sha2::Digest::update(&mut h, f.strip_prefix(src).unwrap_or(f).to_string_lossy().as_bytes());
        sha2::Digest::update(&mut h, std::fs::read(f).map_err(|e| Error::io(f, e))?);
    }
    sha2::Digest::update(&mut h, format!("{}|{}|{}", tc.rust.display(), tc.linker.display(), ar.display()).as_bytes());
    let key = build_core::key::to_hex(&sha2::Digest::finalize(h));
    let kept = fs.join("renhdrum").join(&key[..16]).join(name);
    if kept.is_file() {
        r.log(format!("renhdrum: {name} kept from before ({})", &key[..12]));
        return Ok((Some(kept), 0));
    }
    let work = fs.join("renhdrum").join("work");
    let _ = build_core::fsx::remove_tree(&work);
    let krate = work.join("renhdrum");
    for f in ["Cargo.toml", "Cargo.lock", "build.rs"] {
        build_core::fsx::link_or_copy(&crate_src.join(f), &krate.join(f))?;
    }
    build_core::fsx::copy_tree(&crate_src.join("src"), &krate.join("src"))?;
    let vendor = krate.join("vendor").join("opus");
    for sub in ["celt", "silk", "src", "include"] {
        build_core::fsx::copy_tree(&opus.join(sub), &vendor.join(sub))?;
    }
    // (a program of its own, with a main: RenHDrum's vendor/opus has none of it, build.rs compiles every .c of celt/)
    let _ = std::fs::remove_file(vendor.join("celt").join("opus_custom_demo.c"));
    build_core::fsx::link_or_copy(&opus.join("COPYING"), &vendor.join("COPYING"))?;
    let downloaded = cargo::fetch(tc, &krate)?;
    let triple = pins::triple(cfg!(windows));
    let under = triple.replace('-', "_");
    let mut flags = format!("--remap-path-prefix={}=renhdrum", krate.display());
    flags += if cfg!(windows) {
        " -C target-feature=+crt-static -C link-arg=-Wl,--no-insert-timestamp"
    } else {
        " -C link-arg=-Wl,--build-id=none"
    };
    let env = vec![
        (format!("CC_{under}"), tc.linker.display().to_string()),
        (format!("AR_{under}"), ar.display().to_string()),
        (format!("CFLAGS_{under}"), format!("-ffile-prefix-map={}=renhdrum", work.display())),
    ];
    let built = if cfg!(windows) { "renhdrum.dll" } else { "librenhdrum.so" };
    let target = work.join("target");
    let b = cargo::build(
        tc,
        &krate,
        &["--release", "--lib"],
        &flags,
        &env,
        jobs,
        &target,
        &cargo::out_dir(&target, "release").join(built),
        0,
        "RenHDrum",
        r,
        &|_| {},
    )?;
    build_core::fsx::link_or_copy(&b.file, &kept)?;
    let _ = build_core::fsx::remove_tree(&work);
    r.log(format!("renhdrum: {name} compiled in {:.0} s (archiver {})", b.seconds, ar.display()));
    Ok((Some(kept), downloaded))
}

/// The third-party objects (`compile --third-party-objects`) compiled by this builder from the source's tree and with
/// its tools, into a store kept for the next run: a set a player's build takes its objects from by their keys (the
/// compiler, the flags, the sources), so any the source's own compile would key otherwise it compiles itself.
fn third_party_objects(src: &Path, fs: &Path, cache: &Path, jobs: usize, r: &Reporter) -> Result<PathBuf> {
    let set = fs.join("third-party-objects");
    let _ = build_core::fsx::remove_tree(&set);
    let exe = std::env::current_exe().map_err(|e| Error::io("longshot-builder", e))?;
    let mut c = build_core::child::command(&exe);
    c.arg("compile")
        .arg("--work")
        .arg(fs.join("third-party-work"))
        .arg("--root")
        .arg(src)
        .arg("--store")
        .arg(fs.join("store"))
        .args(["--jobs", &jobs.to_string(), "--output", "json", "--cache"])
        .arg(cache)
        .arg("--third-party-objects")
        .arg(&set)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, _) in std::env::vars_os() {
        let k = k.to_string_lossy();
        if k.starts_with("RX_")
            || k.starts_with("A64_")
            || (k.starts_with("LONGSHOT_") && !k.starts_with("LONGSHOT_MEM_"))
        {
            c.env_remove(&*k);
        }
    }
    let mut child = build_core::child::spawn(&mut c).map_err(|e| Error::io(&exe, e))?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let mut why = String::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stderr.expect("piped")).lines().map_while(std::result::Result::ok) {
                r.log(line);
            }
        });
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout.expect("piped")).lines().map_while(std::result::Result::ok) {
            let Ok(e) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            match e["type"].as_str() {
                Some("progress") => r.progress(0.2 + 0.7 * e["fraction"].as_f64().unwrap_or(0.0)),
                Some("log") | Some("warning") => r.log(e["text"].as_str().unwrap_or_default().to_string()),
                Some("error") => why = e["text"].as_str().unwrap_or_default().to_string(),
                _ => {}
            }
        }
    });
    let status = child.wait().map_err(|e| Error::io(&exe, e))?;
    if !status.success() || !set.join("set.json").is_file() {
        return Err(Error::player(
            "pack_failed",
            format!("The third-party objects could not be compiled from the source ({status}): {why}"),
        ));
    }
    Ok(set)
}

fn game_rust_target() -> build_compile::rust::Target {
    use build_compile::rust::Target;
    if cfg!(windows) { Target::WindowsGnu } else { Target::Linux }
}

/// Compile the archive that the source's builder will link from the unpacked tree.
fn build_game_rust(
    src: &Path,
    tc: &cargo::Toolchain,
    target: &Path,
    jobs: usize,
    r: &Reporter,
) -> Result<Option<PathBuf>> {
    use build_compile::rust;
    // Older sources may have the placeholder longshot-rs workspace member but no game link or game profile.
    if !src.join("builder/crates/build-compile/src/rust.rs").is_file() || !rust::has_rust(src) {
        return Ok(None);
    }
    let folder = cargo::out_dir(target, "game");
    let archive = folder.join(game_rust_target().library());
    let key = rust::source_key(src)?;
    let flags = [("CARGO_ENCODED_RUSTFLAGS".into(), format!("--remap-path-prefix={}=longshot", src.display()))];
    let built = cargo::build(
        tc,
        src,
        &["--profile", "game", "-p", "longshot-rs", "--lib"],
        "",
        &flags,
        jobs,
        target,
        &archive,
        0,
        "The game's Rust archive",
        r,
        &|_| {},
    )?;
    let stamp = folder.join(rust::KEY_FILE);
    std::fs::write(&stamp, key).map_err(|e| Error::io(&stamp, e))?;
    r.log(format!("The game's Rust archive compiled in {:.1} s", built.seconds));
    Ok(Some(built.file))
}

/// The pack the new builder carries, written to `pack`: the source's tree as its own build-driver's `TREE` and
/// `LEFT_OUT` name it (this builder's when they can't be read), `VERSION`, the third-party objects, RenHDrum's library
/// and the game's Rust archive, plus the source's tools. Each member is compressed quickly (a pack for this PC: build_driver::sources::compress_fast).
fn make_pack(
    src: &Path,
    version: &str,
    objects: &Path,
    renhdrum: Option<&Path>,
    game_rust: Option<&Path>,
    pack: &Path,
) -> Result<build_driver::sources::Written> {
    use build_driver::sources;
    let theirs = std::fs::read_to_string(src.join("builder/crates/build-driver/src/sources.rs")).unwrap_or_default();
    let tree = pins::str_list(&theirs, "TREE").unwrap_or_else(|| sources::TREE.map(String::from).to_vec());
    let left = pins::str_list(&theirs, "LEFT_OUT").unwrap_or_else(|| sources::LEFT_OUT.map(String::from).to_vec());
    let present: Vec<&str> = tree.iter().map(String::as_str).filter(|p| src.join(p).exists()).collect();
    let left: Vec<&str> = left.iter().map(String::as_str).collect();
    let fail = |e: String| Error::player("pack_failed", format!("What the builder carries could not be made: {e}"));
    let listed = sources::listed(src, &present).map_err(fail)?;
    let name = if cfg!(windows) { "renhdrum.dll" } else { "renhdrum.so" };
    let mut add = Vec::new();
    if let Some(f) = renhdrum {
        add.push(format!("third_party/renhdrum/{name}={}", f.display()));
    }
    if let Some(archive) = game_rust {
        use build_compile::rust::KEY_FILE;
        let target = game_rust_target();
        let folder = format!("longshot-rs/prebuilt/{}", target.platform());
        add.push(format!("{folder}/{}={}", target.library(), archive.display()));
        add.push(format!("{folder}/{KEY_FILE}={}", archive.with_file_name(KEY_FILE).display()));
    }
    let entries = sources::tree_entries_leaving(src, &listed, version, Some(objects), &add, &left).map_err(fail)?;
    let mut tools: Vec<(String, PathBuf)> = std::fs::read_dir(src.join("tools"))
        .map_err(|e| Error::io(src.join("tools"), e))?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    tools.sort();
    sources::write(pack, &entries, &tools, &sources::compress_fast).map_err(fail)
}

/// The new builder asked its version (`--version`): it must read the pack it carries as the release `version`, so a
/// source whose pack has another format than this builder writes is said, not installed.
fn check_reads_its_pack(program: &Path, version: &str) -> Result<()> {
    let out = build_core::child::output(build_core::child::command(program).arg("--version"))
        .map_err(|e| Error::io(program, e))?;
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() && line.starts_with(&format!("longshot-builder {version} (source ")) {
        return Ok(());
    }
    Err(Error::player(
        "pack_failed",
        format!(
            "The builder compiled from this source does not read the pack this builder made for it ({line:?}): the \
             source changed what a builder carries. Build it with a newer Longshot."
        ),
    ))
}

/// The compiled builder `built` with the pack at `pack` appended (build_driver::sources::attach), written to `to`
/// (executable).
fn attach(built: &Path, pack: &Path, to: &Path) -> Result<()> {
    let program = std::fs::read(built).map_err(|e| Error::io(built, e))?;
    let pack = std::fs::read(pack).map_err(|e| Error::io(pack, e))?;
    let bytes = build_driver::sources::attach(&program, &pack).map_err(|e| Error::player("pack_failed", e))?;
    std::fs::write(to, bytes).map_err(|e| Error::io(to, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755)).map_err(|e| Error::io(to, e))?;
    }
    Ok(())
}

/// The compiled builder put at `program` (Windows: made a windowed program, as a release's version folder has it),
/// through `<program>.new`: its SHA-256.
fn install(built: &Path, program: &Path) -> Result<String> {
    let bytes = std::fs::read(built).map_err(|e| Error::io(built, e))?;
    let bytes = if cfg!(windows) {
        crate::dev::release_cmd::windowed(bytes).map_err(|e| Error::player("pack_failed", e))?
    } else {
        bytes
    };
    if let Some(dir) = program.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let new = PathBuf::from(format!("{}.new", program.display()));
    std::fs::write(&new, &bytes).map_err(|e| Error::io(&new, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| Error::io(&new, e))?;
    }
    build_core::fsx::rename(&new, program).map_err(|e| Error::io(program, e))?;
    Ok(build_core::key::to_hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_s_name() {
        for ok in ["pr-2286-72beb786", "source-72beb786", "main-1.2", "v3.1.0"] {
            assert!(name_ok(ok), "{ok}");
        }
        for bad in ["", "-x", "a b", "a/b", "../x", &"a".repeat(65)] {
            assert!(!name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn a_folder_built_from_the_commit_already_is_kept() {
        let d = source::tests::scratch("built-already");
        let c = "72beb786419f0123456789abcdef0123456789ab";
        assert_eq!(built_already(&d, c, "f", "v"), None, "no note");
        std::fs::write(d.join(program_name()), b"program").unwrap();
        let sha = build_driver::sha256_file(&d.join(program_name())).unwrap().0;
        let note = |commit: &str, sha: &str| {
            serde_json::json!({"commit": commit, "from": "f", "version": "v", "program": program_name(), "program_sha256": sha})
                .to_string()
        };
        std::fs::write(d.join(NOTE), note(c, &sha)).unwrap();
        assert_eq!(built_already(&d, c, "f", "v"), Some(0.0));
        assert_eq!(built_already(&d, c, "f", "w"), None, "another name");
        assert_eq!(built_already(&d, c, "elsewhere", "v"), None, "the same commit from another --url");
        std::fs::write(d.join(NOTE), note(&"0".repeat(40), &sha)).unwrap();
        assert_eq!(built_already(&d, c, "f", "v"), None, "another commit");
        std::fs::write(d.join(NOTE), note(c, &"0".repeat(64))).unwrap();
        assert_eq!(built_already(&d, c, "f", "v"), None, "the program changed");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_from_source_pack_contains_its_game_rust() {
        use build_compile::rust;
        let d = source::tests::scratch("pack-game-rust");
        let src = d.join("src");
        let objects = d.join("objects");
        for rel in ["assets/pipeline_states", "longshot-rs/src", "game/crates/guest", "tools"] {
            std::fs::create_dir_all(src.join(rel)).unwrap();
        }
        std::fs::create_dir_all(&objects).unwrap();
        std::fs::write(objects.join("set.json"), b"{}").unwrap();
        std::fs::write(src.join("assets/pipeline_states/fixture.bin.xz"), b"fixture").unwrap();
        std::fs::write(src.join("Cargo.toml"), b"[workspace]\n").unwrap();
        std::fs::write(src.join("longshot-rs/Cargo.toml"), b"[package]\n").unwrap();
        std::fs::write(src.join("game/crates/guest/fixture.rs"), b"fn guest() {}").unwrap();

        let target = game_rust_target();
        let archive = d.join(target.library());
        std::fs::write(&archive, b"fixture static library").unwrap();
        let key = rust::source_key(&src).unwrap();
        std::fs::write(d.join(rust::KEY_FILE), &key).unwrap();
        let pack = d.join("sources.pack");
        make_pack(&src, "source-test", &objects, None, Some(&archive), &pack).unwrap();

        let unpacked = build_driver::sources::unpack(&std::fs::read(pack).unwrap(), &d.join("cache")).unwrap();
        assert_eq!(rust::source_key(&unpacked.dir).unwrap(), key);
        let archive = rust::prebuilt(&unpacked.dir, target, &key).expect("the archive must match its packed sources");
        assert_eq!(std::fs::read(archive).unwrap(), b"fixture static library");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_needed_files_are_this_tree_s() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        for f in NEEDED {
            assert!(root.join(f).is_file(), "{f}");
        }
    }
}
