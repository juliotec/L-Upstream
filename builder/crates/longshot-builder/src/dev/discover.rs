//! `longshot-builder dev discover --image <dir>`: the discovery of the image in `<dir>` (a lift's image folder: image.bin,
//! image.json), with its counts and time.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

struct Args {
    image: PathBuf,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut out = Args { image: PathBuf::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--image" => out.image = it.next().ok_or("--image needs a folder")?.into(),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if out.image.as_os_str().is_empty() {
        return Err("--image <dir> is required".into());
    }
    Ok(out)
}

/// The command; `args` are those after `discover`.
pub fn run(args: &[String]) -> ExitCode {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("longshot-builder dev discover: {e}");
            return ExitCode::from(2);
        }
    };
    match discover(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("longshot-builder dev discover: {e}");
            ExitCode::from(2)
        }
    }
}

fn discover(a: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let image = image::Image::load(&a.image)?;
    let t_read = t0.elapsed();
    let inputs = cfg::entries::inputs(&image)?;
    let t_inputs = t0.elapsed() - t_read;
    let t1 = Instant::now();
    let mut d = cfg::Discovery::new(&image, inputs);
    d.run();
    let t_walk = t1.elapsed();
    let new = d.plan(&a.image.join("image.bin"));
    println!(
        "discover: {} entries, {} functions, {} instructions, {} jump tables, {} PLT stubs, {} setjmp owners",
        new.entries.len(),
        new.functions.len(),
        new.functions.values().map(cfg::plan::Function::len).sum::<usize>(),
        new.jump_tables.len(),
        new.plt.len(),
        new.setjmp_owners.len()
    );
    let counts: Vec<String> = d.stats.counts.iter().map(|(k, v)| format!("{k} {v}")).collect();
    println!("discover: {}", counts.join(", "));
    println!(
        "discover: read {:.2}s, entries {:.2}s, walks {:.2}s",
        t_read.as_secs_f64(),
        t_inputs.as_secs_f64(),
        t_walk.as_secs_f64()
    );
    Ok(())
}
