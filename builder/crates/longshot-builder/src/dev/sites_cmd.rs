//! `longshot-builder dev sites --image <dir> --tables <game>`: the build's hook sites read, resolved and placed over the
//! image and its discovery (the same as the lift's: `cfg`, from the image alone), a line each. Exits 1 when a site
//! does not resolve or does not fit, naming it, as the lift would stop.

use image::Image;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: longshot-builder dev sites --image <dir> --tables <game>";

struct Args {
    image: PathBuf,
    tables: PathBuf,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let (mut image, mut tables) = (None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let slot = match a.as_str() {
            "--image" => &mut image,
            "--tables" => &mut tables,
            other => return Err(format!("unknown argument {other:?}")),
        };
        *slot = Some(PathBuf::from(it.next().ok_or_else(|| format!("{a} needs a value"))?));
    }
    match (image, tables) {
        (Some(image), Some(tables)) => Ok(Args { image, tables }),
        _ => Err("--image and --tables are each needed".into()),
    }
}

/// Runs the command on its arguments (after `sites`).
pub fn run(args: &[String]) -> ExitCode {
    let args = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("longshot-builder dev sites: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let image = match Image::load(&args.image) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("longshot-builder dev sites: {}: {e}", args.image.display());
            return ExitCode::FAILURE;
        }
    };
    let inputs = match cfg::entries::inputs(&image) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("longshot-builder dev sites: the image's modules: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut d = cfg::Discovery::new(&image, inputs);
    d.run();
    let plan = d.plan(&args.image.join("image.bin"));
    match sites::apply(&sites::Tables::new(&args.tables), &image, &plan) {
        Ok(Some((_, placement))) => {
            for line in &placement.log {
                println!("{line}");
            }
            println!("sites: {} placed", placement.sites.len());
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("hooks: no site table for this build: nothing placed");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
