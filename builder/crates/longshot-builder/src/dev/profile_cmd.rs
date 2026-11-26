//! `longshot-builder dev profile --map <profile_map.txt> --raw <counts> [--raw <counts> ...] [--into <profile>] --out
//! <profile>`: a run's raw counts (host/guest_profile.c's HOS_GUEST_PROFILE file) made a guest profile by the
//! instrumented build's map, added to `--into`'s counts when given (the same code's counts add up; a function whose
//! code changed keeps the newer counts alone), written to `--out`.

use std::path::PathBuf;
use std::process::ExitCode;

/// The command; `args` are those after `profile`.
pub fn run(args: &[String]) -> ExitCode {
    match profile(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("longshot-builder dev profile: {e}");
            ExitCode::FAILURE
        }
    }
}

fn profile(args: &[String]) -> Result<(), String> {
    let (mut map, mut raws, mut into, mut out) = (None, Vec::new(), None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a file"));
        match a.as_str() {
            "--map" => map = Some(PathBuf::from(value()?)),
            "--raw" => raws.push(PathBuf::from(value()?)),
            "--into" => into = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let (Some(map), Some(out)) = (map, out) else { return Err("--map and --out are required".into()) };
    if raws.is_empty() {
        return Err("at least one --raw is required".into());
    }
    let map = profile::Map::load(&map).map_err(|e| e.to_string())?;
    let mut p = match &into {
        Some(path) => profile::Profile::load(path).map_err(|e| e.to_string())?,
        None => profile::Profile::default(),
    };
    for raw in &raws {
        let text = std::fs::read_to_string(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
        let run = map.read_raw(&text, &raw.display().to_string()).map_err(|e| e.to_string())?;
        println!("longshot-builder dev profile: {}: {} functions ran", raw.display(), run.functions.len());
        p.merge(&run);
    }
    std::fs::write(&out, p.to_text()).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("longshot-builder dev profile: {} functions in {}", p.functions.len(), out.display());
    Ok(())
}
