use std::{ffi::OsStr, ops::Deref};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};

/// Parse `cargo [OPTIONS] [SUBCOMMAND]` arguments.
///
/// Does extra work so CargoArgs.verbose (and .color) also represent the subcommand's.
pub(crate) fn parse<'a>(args: impl IntoIterator<Item = &'a OsStr>) -> Option<CargoArgs> {
    let parsed @ CargoArgs { mut verbose, mut color, .. } = CargoArgs::try_parse_from(args).ok()?;
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

    // `cargo build --color always`: the flag follows the subcommand, which swallowed it
    let mut after = parsed.command.iter().flat_map(|c| c.iter()).take_while(|arg| *arg != "--");
    while let Some(arg) = after.next() {
        let when = match arg.strip_prefix("--color=") {
            Some(when) => Some(when),
            None if arg == "--color" => after.next().map(String::as_str),
            None => None,
        };
        if let Some(mode) = when.and_then(|when| ColorMode::from_str(when, true).ok()) {
            color = Some(mode);
        }
    }

    Some(CargoArgs { verbose, color, ..parsed })
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

    #[arg(long, value_name = "WHEN", value_enum)]
    color: Option<ColorMode>,

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
    /// `--color <WHEN>`, wherever it was passed: if it was.
    pub(crate) fn color(&self) -> Option<ColorMode> {
        self.color
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub(crate) enum ColorMode {
    Auto,
    Always,
    Never,
}

impl ColorMode {
    /// As cargo takes it: `always`
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[test]
fn find_cargo_subcommand() {
    fn sub(args: &[&str]) -> Option<String> {
        parse(args.iter().map(|&x| OsStr::new(x)))?.subcommand()
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
fn find_cargo_color() {
    fn color(args: &[&str]) -> Option<ColorMode> {
        parse(args.iter().map(|&x| OsStr::new(x)))?.color()
    }

    assert_eq!(None, color(&["build"]));
    assert_eq!(Some(ColorMode::Always), color(&["--color", "always", "build"]));
    assert_eq!(Some(ColorMode::Never), color(&["--color=never", "build"]));
    assert_eq!(Some(ColorMode::Auto), color(&["--color=auto", "build"]));
    // After the subcommand too
    assert_eq!(Some(ColorMode::Always), color(&["build", "--color", "always"]));
    assert_eq!(Some(ColorMode::Never), color(&["build", "--color=never", "--release"]));
    // The last one wins
    assert_eq!(Some(ColorMode::Never), color(&["--color=always", "build", "--color=never"]));
    // Flags after `--` are the program's, not cargo's
    assert_eq!(None, color(&["run", "--", "--color=always"]));
    assert_eq!(None, color(&["run", "--", "--color", "always"]));
    assert_eq!(Some(ColorMode::Never), color(&["--color=never", "run", "--", "--color=always"]));
    // Nonsense is cargo's to complain about
    assert_eq!(None, color(&["build", "--color=sometimes"]));
    assert_eq!(None, color(&["build", "--color"]));

    assert_eq!("always", ColorMode::Always.as_str());
}

#[test]
fn find_cargo_verbosity() {
    fn verbosity(args: &[&str]) -> u8 {
        parse(args.iter().map(|&x| OsStr::new(x))).map(|a| a.verbose).unwrap_or_default()
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
