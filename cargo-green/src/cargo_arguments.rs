use clap::{ArgAction, Parser, Subcommand, ValueEnum};

/// `cargo -Z bla --color=always build ..` => `build`
pub(crate) fn subcommand(args: impl IntoIterator<Item = String>) -> Option<String> {
    let CargoSubcommand::External(extra) = CargoArgs::try_parse_from(args).ok()?.command?;
    extra.into_iter().next()
}

#[derive(Debug, Parser)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
// #[allow(dead_code)] // Only `command` is ever read
struct CargoArgs {
    #[arg(short = 'V', long)]
    version: bool,

    #[arg(long)]
    list: bool,

    #[arg(long, value_name = "CODE")]
    explain: Option<String>,

    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,

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

#[derive(Debug, Subcommand)]
enum CargoSubcommand {
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(ValueEnum, Clone, Debug)]
#[value(rename_all = "kebab-case")]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[test]
fn find_cargo_subcommand() {
    fn sub(args: &[&str]) -> Option<String> {
        subcommand(args.iter().map(ToString::to_string))
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
