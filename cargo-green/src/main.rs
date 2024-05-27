use std::{
    env,
    ffi::OsString,
    process::{Command as StdCommand, ExitCode},
};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let cargo = env::var("CARGO").unwrap_or("cargo".into());
    let args: Vec<OsString> = env::args_os().skip(1).collect(); // skips $0

    let subcmd = &args[0];
    if [&"build".into(), &"test".into() /*FIXME*/].contains(&subcmd) {
        // do pull
        // ensure bin exists
        // export env
    }

    let status = StdCommand::new(cargo).args(args).status()?;
    Ok(status.code().map_or(ExitCode::FAILURE, |code| ExitCode::from(code as u8)))
}
