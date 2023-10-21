use anyhow::{bail, Result};
use env_logger::Env;
use std::process::Command;

fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    let args: Vec<_> = std::env::args().collect();
    let ok = match args.iter().map(|arg| &arg[..]).collect::<Vec<_>>()[..] {
        [] | [_] => true,
        [_, rustc, "-", ..] => Command::new(rustc)
            .args(args.into_iter().skip(2))
            .spawn()?
            .wait()?
            .success(),
        [_, rustc, ..] => Command::new(rustc)
            .args(args.into_iter().skip(2))
            .spawn()?
            .wait()?
            .success(), // FIXME
                        // [_, _____, ..rest] => rest,
    };
    if !ok {
        bail!("spawn failed") // TODO: hide message from STDERR
    }

    Ok(())
}
