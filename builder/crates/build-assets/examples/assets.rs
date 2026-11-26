//! Make one of the finish stage's outputs, to check it against another build's:
//!
//! ```text
//! cargo run --release -p build-assets --example assets -- <step> <romfs> <out folder> [work folder]
//! ```
//!
//! `step`: `texts`, `pipelines <sources folder>`, `census`, `prompts <repository>` (`PROMPTS_DUMP=<folder>`: every
//! face's layers as PNG too), or `finish <repository>` (all of them, as the driver runs them). The work folder (the cache) defaults to `<out folder>/.work`.

use build_core::report::{Format, Reporter, Stage};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "assets <texts | pipelines <sources> | census | prompts <repository> | finish <repository>> <romfs> <out> [work]";
    let (step, rest) = args.split_first().expect(usage);
    let (extra, rest) = if matches!(step.as_str(), "pipelines" | "finish" | "prompts") {
        let (e, r) = rest.split_first().expect(usage);
        (Some(PathBuf::from(e)), r)
    } else {
        (None, rest)
    };
    let romfs = build_assets::romfs::Folder::new(&PathBuf::from(rest.first().expect(usage))).expect("a romfs folder");
    let out = PathBuf::from(rest.get(1).expect(usage));
    let work = rest.get(2).map_or_else(|| out.join(".work"), PathBuf::from);
    let report = Reporter::stdout(Format::Text);
    report.stage(Stage::Finish, step);
    let cache = build_assets::cache::Cache::open(&work);
    let started = Instant::now();
    let result = match step.as_str() {
        "texts" => build_assets::texts::make(&romfs, &out.join("texts"), &cache, &report)
            .map(|made| format!("{} languages reworded", made.len())),
        "pipelines" => build_assets::pipelines::ship(&extra.expect(usage), &out, &mut |l| report.log(l))
            .map(|n| format!("{n} pipelines")),
        "census" => build_assets::census::census(&romfs, build_core::jobs::default_jobs(), &|f| report.progress(f))
            .and_then(|c| {
                build_core::fsx::write_atomic(&out.join(build_assets::census::FILE), &c.bytes())?;
                for s in &c.skipped {
                    report.log(s.clone());
                }
                Ok(format!("{} programs from {} stages", c.programs.len(), c.stages.len()))
            }),
        "prompts" => {
            let sources = build_assets::prompts::Sources::in_checkout(&extra.expect(usage));
            let options = build_assets::prompts::Options {
                jobs: build_core::jobs::chosen(None, std::env::var("LONGSHOT_JOBS").ok().as_deref()).unwrap_or(1),
                dump: std::env::var_os("PROMPTS_DUMP").map(PathBuf::from),
                ..Default::default()
            };
            build_assets::prompts::make(&romfs, &sources, &out.join("prompts"), &cache, &options, &report)
                .map(|m| format!("made {:?}, kept {:?}", m.made, m.kept))
        }
        _ => panic!("{usage}"),
    };
    match result {
        Ok(text) => report.log(format!("{step}: {text} in {:.1} s", started.elapsed().as_secs_f64())),
        Err(e) => {
            report.error(&e);
            std::process::exit(1);
        }
    }
}
