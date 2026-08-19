use std::ops::Deref;

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

/// Parse `cargo [OPTIONS] [SUBCOMMAND]` arguments.
///
/// Does extra work so CargoArgs.verbose also represents the subcommand verbosity.
pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Option<CargoArgs> {
    let parsed @ CargoArgs { mut verbose, .. } = CargoArgs::try_parse_from(args).ok()?;
    verbose += parsed
        .command
        .clone()
        .as_deref()
        .into_iter()
        .flatten()
        .take_while(|arg| *arg != "--")
        .map(|arg| match arg.as_str() {
            v if v.trim_end_matches('v') == "-" => v.chars().filter(|c| *c == 'v').count() as u8,
            "--verbose" => 1,
            _ => 0,
        })
        .sum::<u8>();
    Some(CargoArgs { verbose, ..parsed })
}

#[derive(Clone, Debug, Parser)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct CargoArgs {
    #[arg(short = 'V', long)]
    version: bool,

    #[arg(long)]
    list: bool,

    #[arg(long, value_name = "CODE")]
    explain: Option<String>,

    #[arg(short, long, action = ArgAction::Count)]
    pub(crate) verbose: u8,

    #[arg(short, long)]
    quiet: bool,

    #[arg(long, value_name = "WHEN", value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,

    #[arg(short = 'C', value_name = "DIRECTORY")]
    chdir: Option<String>,

    #[arg(long)]
    locked: bool,

    #[arg(long)]
    offline: bool,

    #[arg(long)]
    frozen: bool,

    #[arg(long, value_name = "KEY=VALUE|PATH")]
    config: Vec<String>,

    #[arg(short = 'Z', value_name = "FLAG")]
    unstable_flags: Vec<String>,

    #[arg(short = 'h', long)]
    help: bool,

    #[command(subcommand)]
    command: Option<CargoSubcommand>,
}

impl CargoArgs {
    /// `cargo -Z bla --color=always build ..` => `build`
    pub(crate) fn subcommand(&self) -> Option<String> {
        self.command.as_deref()?.iter().next().cloned()
    }
}

#[derive(Clone, Debug, Subcommand)]
enum CargoSubcommand {
    #[command(external_subcommand)]
    External(Vec<String>),
}

impl Deref for CargoSubcommand {
    type Target = Vec<String>;

    fn deref(&self) -> &Self::Target {
        let CargoSubcommand::External(extra) = self;
        extra
    }
}

#[derive(Clone, Debug, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[test]
fn find_cargo_subcommand() {
    fn sub(args: &[&str]) -> Option<String> {
        parse(args.iter().map(ToString::to_string))?.subcommand()
    }

    assert_eq!("install", sub(&["install", "smth"]).unwrap());
    assert_eq!("build", sub(&["-Zbla", "build"]).unwrap());
    assert_eq!("build", sub(&["-Z", "bla", "build"]).unwrap());
    assert_eq!("build", sub(&["--color", "always", "build"]).unwrap());
    assert_eq!("build", sub(&["--color=always", "build"]).unwrap());
    assert_eq!("check", sub(&["-vv", "--frozen", "check"]).unwrap());
    assert_eq!("t", sub(&["-C", "some/dir", "-q", "t"]).unwrap());
    assert_eq!("r", sub(&["--config", "build.jobs=2", "-Z", "a", "-Zb", "r"]).unwrap());
    assert_eq!("fetch", sub(&["fetch"]).unwrap());
    assert_eq!("test", sub(&["test", "--all-targets", "--frozen"]).unwrap());

    assert_eq!(None, sub(&[]));
    assert_eq!(None, sub(&["--version"]));
    assert_eq!(None, sub(&["-V"]));
    assert_eq!(None, sub(&["--list"]));
    assert_eq!(None, sub(&["--explain", "E0308"]));
    assert_eq!(None, sub(&["-h"]));
    assert_eq!(None, sub(&["--definitely-not-a-cargo-flag", "build"]));

    assert_eq!("run", sub(&["run", "--color", "always"]).unwrap());
    assert_eq!("b", sub(&["b", "-Zunstable-options", "--", "-q"]).unwrap());
}

#[test]
fn find_cargo_verbosity() {
    fn verbosity(args: &[&str]) -> u8 {
        parse(args.iter().map(ToString::to_string)).map(|a| a.verbose).unwrap_or_default()
    }

    assert_eq!(1, verbosity(&["-v", "build"]));
    assert_eq!(1, verbosity(&["--verbose", "build"]));
    assert_eq!(2, verbosity(&["-vv", "--frozen", "check"]));
    assert_eq!(1, verbosity(&["build", "--verbose"]));
    assert_eq!(5, verbosity(&["-vvv", "install", "--verbose", "-v"]));

    assert_eq!(0, verbosity(&["build"]));
    assert_eq!(0, verbosity(&["-q", "build"]));
    assert_eq!(0, verbosity(&[]));
    assert_eq!(0, verbosity(&["--definitely-not-a-cargo-flag", "-v", "build"]));
    assert_eq!(0, verbosity(&["b", "--", "-v"]));
    assert_eq!(0, verbosity(&["-Zavoid-dev-deps", "build"]));
}
