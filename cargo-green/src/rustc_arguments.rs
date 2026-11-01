use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;

use crate::{all_our_envs::OUT_DIR, ext::Popped, md::MdId, sys::fs};

const SYSROOT_CRATES: &[&str] = &["alloc", "core", "proc_macro", "std", "test"];

/// RustcArgs contains parts of `rustc`'s arguments
#[derive(Debug, PartialEq)]
pub(crate) struct RustcArgs {
    /// 0..: --extern=EXTERN | --extern EXTERN
    pub(crate) externs: IndexSet<String>,

    /// 1: -C extra-filename=EXTRAFN e.g. "-710b4516f388a5e4"
    pub(crate) mdid: Option<MdId>,

    /// 0|1: -C incremental=INCREMENTAL
    pub(crate) incremental: Option<Utf8PathBuf>,

    /// 1: plain path to (non-empty existing) file
    pub(crate) input: Utf8PathBuf,

    /// 1: --out-dir OUT_DIR
    pub(crate) out_dir: Utf8PathBuf,

    /// Target path:
    pub(crate) target_path: Utf8PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Arg {
    /// E.g. `--test`, `-g`
    Flag(String),
    /// E.g. `-Copt-level=3`, from `-C opt-level=3`, `-Copt_level=3`, `--codegen=opt-level=3`, ...
    Pair(String, String),
    /// The crate root
    Input(String),
}

const VALUELESS: &[&str] = &["--test", "-g", "-O", "-v", "-h", "-V"];

#[must_use]
fn canonical_flag(flag: &str) -> &str {
    match flag {
        "--allow" => "-A",
        "--codegen" => "-C",
        "--deny" => "-D",
        "--forbid" => "-F",
        "--help" => "-h",
        "--verbose" => "-v",
        "--version" => "-V",
        "--warn" => "-W",
        _ => flag,
    }
}

/// Splits arguments by their shape, the way rustc reads them: `--flag=value`, `--flag value`,
/// `-Fvalue`, `-F value`, valueless options (`-g`, grouped: `-gO`) and input,
/// outputting a canonical spelling.
fn tokenize(arguments: &[String]) -> Result<Vec<Arg>> {
    let mut args = Vec::with_capacity(arguments.len());
    let mut iter = arguments.iter().map(String::as_str);

    while let Some(arg) = iter.next() {
        if arg == "-" {
            bail!("BUG: unexpected rustc input from STDIN");
        }

        if !arg.starts_with('-') {
            args.push(Arg::Input(arg.to_owned()));
            continue;
        }

        // Parse flag and glued value (--flag=value or -Fvalue)
        let (flag, glued) = if let Some(rest) = arg.strip_prefix("--") {
            rest.split_once('=').map_or((arg, None), |(f, v)| (&arg[..f.len() + 2], Some(v)))
        } else {
            // Split after first char of short flag (e.g., `-C...` -> `-C`, `...`)
            let (f, v) = arg.split_at(arg.ceil_char_boundary(2));
            (f, (!v.is_empty()).then_some(v))
        };

        let mut flag = canonical_flag(flag).to_owned();
        let mut glued = glued;

        // Handle grouped short flags (e.g., `-gO` -> `-g` + `-O`, `-gDwarnings` -> `-g` + `-Dwarnings`)
        while !arg.starts_with("--") && VALUELESS.contains(&flag.as_str()) {
            let Some(rest) = glued else { break };
            args.push(Arg::Flag(flag));
            let (f, v) = rest.split_at(rest.ceil_char_boundary(1));
            (flag, glued) = (format!("-{f}"), (!v.is_empty()).then_some(v));
        }

        let is_valueless = VALUELESS.contains(&flag.as_str());

        let val = match (glued, is_valueless) {
            (None, true) => {
                args.push(Arg::Flag(flag));
                continue;
            }
            (Some(_), true) => bail!("BUG: unexpected rustc argument {arg:?}"),
            (Some(v), false) => v.to_owned(),
            (None, false) => match iter.next() {
                Some(v) => v.to_owned(),
                None => bail!("BUG: rustc argument {flag:?} is missing its value"),
            },
        };

        // Normalize `-C` and `-Z` key names (underscore to hyphen)
        let val = match flag.as_str() {
            "-C" | "-Z" => match val.split_once('=') {
                Some((key, v)) => format!("{}={v}", key.replace('_', "-")),
                None => val.replace('_', "-"),
            },
            _ => val,
        };

        args.push(Arg::Pair(flag, val));
    }

    Ok(args)
}

impl Arg {
    /// Arguments are sorted (stably) by this, for recipes not to depend on how they were passed.
    /// Yet rustc reads some in order, so these keep their relative order:
    /// * lint levels: a later one wins (how cargo's `[lints]` priorities work)
    /// * `-L`s: libraries are looked up in order
    /// * `-l`s: link order
    /// * `--remap-path-prefix`es: a later one wins
    /// * same-key `-C`s and `-Z`s: a later one wins, or they accumulate (`link-arg`, `llvm-args`, ...)
    /// * `--print`s: their outputs come in order
    #[must_use]
    fn sort_key(&self) -> (&str, &str, &str) {
        match self {
            Self::Flag(flag) => (flag, "", ""),
            Self::Pair(flag, val) => match flag.as_str() {
                "-A" | "-W" | "-D" | "-F" | "--force-warn" => ("-W", "", ""),
                "-L" | "-l" | "--remap-path-prefix" | "--print" | "--env-set" => (flag, "", ""),
                "-C" | "-Z" => {
                    let key = val.split_once('=').map_or(val.as_str(), |(key, _)| key);
                    (flag, key.strip_suffix('s').unwrap_or(key), "") // link-arg & link-args
                }
                _ => (flag, val, ""),
            },
            Self::Input(input) => ("", "", input),
        }
    }

    /// Maps the value of a pair, or the input (e.g. to rewrite paths in it).
    #[must_use]
    pub(crate) fn map(self, f: impl FnOnce(&str) -> String) -> Self {
        match self {
            Self::Flag(_) => self,
            Self::Pair(flag, val) => Self::Pair(flag, f(&val)),
            Self::Input(input) => Self::Input(f(&input)),
        }
    }
}

/// The shortest spelling, as one argument: `-Dwarnings`, `--cfg=tokio_unstable`.
impl std::fmt::Display for Arg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flag(flag) | Self::Input(flag) => write!(f, "{flag}"),
            Self::Pair(flag, val) if flag.starts_with("--") => write!(f, "{flag}={val}"),
            Self::Pair(flag, val) => write!(f, "{flag}{val}"),
        }
    }
}

/// Expands `@path` arguments into the arguments they hold, one per line: cargo resorts to them
/// for overly long command lines. (`@shell:path` is unstable, left as is.)
pub(crate) fn expand_argfiles(pwd: &Utf8Path, arguments: Vec<String>) -> Result<Vec<String>> {
    let mut expanded = Vec::with_capacity(arguments.len());
    for arg in arguments {
        match arg.strip_prefix('@') {
            Some(path) if !path.starts_with("shell:") => {
                let path = pwd.join(path);
                let contents = fs()
                    .read_to_string(&path)
                    .map_err(|e| anyhow!("Failed reading argfile {path}: {e}"))?;
                expanded.extend(contents.lines().map(ToOwned::to_owned));
            }
            _ => expanded.push(arg),
        }
    }
    Ok(expanded)
}

pub(crate) fn as_rustc(
    pwd: &Utf8Path,
    arguments: &[String],
    out_dir_var: Option<&Utf8Path>,
) -> Result<(RustcArgs, Vec<Arg>)> {
    let mut args = vec![];

    let mut state = RustcArgs {
        externs: IndexSet::default(),
        mdid: None,
        incremental: None,
        input: "".into(),
        out_dir: "".into(),
        target_path: "".into(),
    };

    for arg in tokenize(arguments)? {
        let (key, mut val) = match arg {
            Arg::Input(input) => {
                if state.input != "" {
                    bail!("BUG: more than one rustc input: {:?} and {input:?}", state.input)
                }
                // For e.g. $HOME/.cargo/registry/src/github.com-1ecc6299db9ec823/ahash-0.7.6/./build.rs
                state.input = input.replace("/./", "/").into();
                continue;
            }
            Arg::Flag(_) => {
                args.push(arg);
                continue;
            }
            Arg::Pair(key, val) => (key, val),
        };

        // FIXME: drop (strips out local config for now)
        match (key.as_str(), val.as_str()) {
            ("-C", "link-arg=-fuse-ld=/usr/local/bin/mold") => continue,
            ("-C", "linker=/usr/bin/clang") => continue,
            _ => {}
        }

        // Skip passing info on Terminal window size
        // https://doc.rust-lang.org/beta/rustc/command-line-arguments.html#--diagnostic-width-specify-the-terminal-width-for-diagnostics
        if key == "--diagnostic-width" {
            continue;
        }

        match key.as_str() {
            "-C" => match val.split_once('=') {
                Some(("extra-filename", v)) => {
                    assert!(state.mdid.is_none());
                    state.mdid = Some(MdId::new(v));
                }
                Some(("incremental", v)) => {
                    assert_eq!(state.incremental, None);
                    state.incremental = Some(Utf8PathBuf::from(v));
                }
                _ => {}
            },
            "-L" => {
                if let Some(("dependency", v)) = val.split_once('=')
                    && !v.starts_with('/')
                {
                    val = format!("dependency={}", pwd.join(v));
                }
            }
            "--extern" => {
                if SYSROOT_CRATES.contains(&val.as_str()) {
                    args.push(Arg::Pair(key, val));
                    continue; // Sysroot crates (e.g. https://doc.rust-lang.org/proc_macro)
                }

                let xtern = match val.split_once('=') {
                    None => &val,
                    Some((_, val)) => val,
                };

                // NOTE:
                // https://github.com/rust-lang/cargo/issues/9661
                // https://github.com/dtolnay/cxx/blob/83d9d43892d9fe67dd031e4115ae38d0ef3c4712/gen/build/src/target.rs#L10
                // https://github.com/rust-lang/cargo/issues/6100
                // This doesn't always verify: case "$extern" in "$deps_path"/*) ;; *) return 4 ;; esac
                // because $CARGO_TARGET_DIR is sometimes set to $PWD/target and sometimes $HOME/.cargo/registry/src/github.com-1ecc6299db9ec823/anstyle-parse-0.1.1
                // So we can't do: externs+=("${extern#"$deps_path"/}")
                // Anyway the goal is simply to just extract libutf8parse-03cddaef72c90e73.rmeta from $HOME/wefwefwef/buildxargs.git/target/debug/deps/libutf8parse-03cddaef72c90e73.rmeta
                // So let's just do that!
                if let Some(xtern) = Utf8Path::new(xtern).file_name() {
                    state.externs.insert(xtern.to_owned());
                } else {
                    bail!("BUG: {xtern} has no file name")
                }
            }
            "--out-dir" => {
                assert_eq!(state.out_dir, "");
                state.out_dir = val.clone().into();
                if state.out_dir.is_relative() {
                    state.out_dir = pwd.join(&val);
                }
                val = state.out_dir.to_string();
            }
            _ => {}
        }

        args.push(Arg::Pair(key, val));
    }

    // Can't rely on $PWD nor $CARGO_TARGET_DIR because `cargo` changes them.
    // Out dir though...
    // --out-dir "$CARGO_TARGET_DIR/$PROFILE"/build/rustix-2a01a00f5bdd1924
    // --out-dir "$CARGO_TARGET_DIR/$PROFILE"/deps
    state.target_path = if let Some(target_path) = out_dir_to_target_path(state.out_dir.clone()) {
        target_path
    } else if let Some(out_dir) = out_dir_var {
        // e.g. OUT_DIR=$HOME/work/supergreen/supergreen/target/debug/build/slab-94793bb2b78c57b5/out
        // NOTE: ducktaping in the dark!
        // We may as well get metadata from the input Rust file: $HOME/work/supergreen/supergreen/target/debug/build/slab-b0340a0384800aca/build-script-build
        // TODO: decide which to use

        assert_eq!(out_dir.file_name(), Some("out"));
        let Some(mdid) = MdId::from_out_dir_var(out_dir) else {
            bail!("BUG: {OUT_DIR} is surprising for this build script: {out_dir}")
        };
        state.mdid = Some(mdid);
        out_dir.to_owned().popped(3)
    } else {
        bail!(
            "BUG: --out-dir path should match /deps$|.+/build/.+: {:?}",
            (state.out_dir, out_dir_var)
        )
    };

    assert!(state.mdid.is_some());
    state.externs.sort(); // Their order makes no difference to rustc
    args.sort_by(|a, b| a.sort_key().cmp(&b.sort_key())); // Stable
    Ok((state, args))
}

#[must_use]
fn out_dir_to_target_path(mut out_dir: Utf8PathBuf) -> Option<Utf8PathBuf> {
    match out_dir.iter().rev().take(3).collect::<Vec<_>>()[..] {
        ["deps", ..] => Some(out_dir.popped(1)),
        ["examples", _profile, ..] => Some(out_dir.popped(1)),
        [_crate_dir, "build", ..] => Some(out_dir.popped(2)),
        ["out", _crate_dir, "build", ..] => Some(out_dir.popped(3)), // E.g. slab-0.4.9's build.rs
        _ => None,
    }
}

#[test]
fn target_path_from_out_dir() {
    for out_dir in [
        "$CARGO_TARGET_DIR/$PROFILE/build/rustix-2a01a00f5bdd1924",
        "$CARGO_TARGET_DIR/$PROFILE/build/slab-3e929764daead7d0/out",
        "$CARGO_TARGET_DIR/$PROFILE/deps",
    ] {
        let res = out_dir_to_target_path(out_dir.into());
        assert_eq!(res, Some("$CARGO_TARGET_DIR/$PROFILE".into()));
    }

    // When cross-compiling:
    for out_dir in [
        "$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/$PROFILE/deps",
        "$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/$PROFILE/build/ring-2a01a00f5bdd1924",
        "$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/$PROFILE/build/slab-3e929764daead7d0/out",
    ] {
        let res = out_dir_to_target_path(out_dir.into());
        assert_eq!(res, Some("$CARGO_TARGET_DIR/armv7-unknown-linux-musleabihf/$PROFILE".into()));
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use pretty_assertions::assert_eq;

    use super::{RustcArgs, expand_argfiles};
    use crate::sys::{Sys, fake::FakeFs};

    const HOME: &str = "/home/maison";
    const PWD: &str = "$HOME/⺟/rustcbuildx.git";

    fn as_argument(arg: &str) -> String {
        arg.replace("$PWD", PWD).replace("$HOME", HOME)
    }

    /// Arguments as they're shown in recipes.
    fn as_rustc(
        pwd: &camino::Utf8Path,
        arguments: &[String],
        out_dir_var: Option<&camino::Utf8Path>,
    ) -> anyhow::Result<(RustcArgs, Vec<String>)> {
        let (st, args) = super::as_rustc(pwd, arguments, out_dir_var)?;
        Ok((st, args.iter().map(ToString::to_string).collect()))
    }

    fn as_arguments(args: &[&str]) -> Vec<String> {
        args.iter().map(|x| as_argument(x)).collect()
    }

    #[test]
    fn args_when_building_final_binary() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments = as_arguments(&[
            "--crate-name", "rustcbuildx",                                                    // crate_name
            "--edition=2021",
            "src/main.rs",                                                                    // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "-C", "metadata=710b4516f388a5e4",
            "-C", "extra-filename=-710b4516f388a5e4",                                         // state.mdid
            "--out-dir", "$PWD/target/debug/deps",                                            // state.out_dir =+> state.target_path
            "-C", "linker=/usr/bin/clang",
            "-C", "incremental=$PWD/target/debug/incremental",                                   // state.incremental
            "-L", "dependency=$PWD/target/debug/deps",
            "--extern", "anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",         // state.externs
            "--extern", "env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib", // state.externs
            "--extern", "log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",               // state.externs
            "--extern", "mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",         // state.externs
            "--extern", "os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",       // state.externs
            "-C", "link-arg=-fuse-ld=/usr/local/bin/mold",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: [
                    "libanyhow-f96497119bad6f50.rlib",
                    "libenv_logger-7e2d283f6e473671.rlib",
                    "liblog-27d1dc50ab631e5f.rlib",
                    "libmktemp-b84fe47f0a44f88d.rlib",
                    "libos_pipe-f344e452b9bd1c5e.rlib",
                ]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
                mdid: Some("710b4516f388a5e4".into()),
                incremental: Some(as_argument("$PWD/target/debug/incremental").into()),
                input: as_argument("src/main.rs").into(),
                out_dir: as_argument("$PWD/target/debug/deps").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  as_arguments(&[
                "--crate-name=rustcbuildx",
                "--crate-type=bin",
             // "--diagnostic-width", "211",
                "--edition=2021",
                "--emit=dep-info,link",
                "--error-format=json",
                "--extern=anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern=env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern=log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern=mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern=os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=$PWD/target/debug/deps",
                "-Cdebuginfo=2",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-710b4516f388a5e4",
                "-Cincremental=$PWD/target/debug/incremental",
                "-Cmetadata=710b4516f388a5e4",
                "-Ldependency=$PWD/target/debug/deps",
             ]));
    }

    #[test]
    fn args_when_building_final_test() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments = as_arguments(&[
            "--crate-name", "rustcbuildx",                                                    // crate_name
            "--edition=2021",
            "src/main.rs",                                                                    // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=347",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "--test",
            "-C", "metadata=7c7a0950383d41d3",
            "-C", "extra-filename=-7c7a0950383d41d3",                                         // state.mdid
            "--out-dir", "$PWD/target/debug/deps",                                            // state.out_dir =+> state.target_path
            "-C", "linker=/usr/bin/clang",
            "-C", "incremental=$PWD/target/debug/incremental",                                // state.incremental
            "-L", "dependency=$PWD/target/debug/deps",
            "--extern", "anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
            "--extern", "env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
            "--extern", "log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
            "--extern", "mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
            "--extern", "os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
            "--extern", "pretty_assertions=$PWD/target/debug/deps/libpretty_assertions-9fa55d8a39fa5fe3.rlib",
            "-C", "link-arg=-fuse-ld=/usr/local/bin/mold",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: [
                    "libanyhow-f96497119bad6f50.rlib",
                    "libenv_logger-7e2d283f6e473671.rlib",
                    "liblog-27d1dc50ab631e5f.rlib",
                    "libmktemp-b84fe47f0a44f88d.rlib",
                    "libos_pipe-f344e452b9bd1c5e.rlib",
                    "libpretty_assertions-9fa55d8a39fa5fe3.rlib",
                ]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
                mdid: Some("7c7a0950383d41d3".into()),
                incremental: Some(as_argument("$PWD/target/debug/incremental").into()),
                input: as_argument("src/main.rs").into(),
                out_dir: as_argument("$PWD/target/debug/deps").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(as_arguments(&[
                "--crate-name=rustcbuildx",
             // "--diagnostic-width", "347",
                "--edition=2021",
                "--emit=dep-info,link",
                "--error-format=json",
                "--extern=anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern=env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern=log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern=mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern=os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
                "--extern=pretty_assertions=$PWD/target/debug/deps/libpretty_assertions-9fa55d8a39fa5fe3.rlib",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=$PWD/target/debug/deps",
                "--test",
                "-Cdebuginfo=2",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-7c7a0950383d41d3",
                "-Cincremental=$PWD/target/debug/incremental",
                "-Cmetadata=7c7a0950383d41d3",
                "-Ldependency=$PWD/target/debug/deps",
             ]), args);
    }

    #[test]
    fn args_when_building_build_script() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments = as_arguments(&[
            "--crate-name", "build_script_build",                                             // crate_name
            "--edition=2021",
            "$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/rustix-0.38.20/build.rs", // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "--cfg", "'feature=\"alloc\"'",
            "--cfg", "'feature=\"default\"'",
            "--cfg", "'feature=\"std\"'",
            "--cfg", "'feature=\"termios\"'",
            "--cfg", "'feature=\"use-libc-auxv\"'",
            "-C", "metadata=c7101a3d6c8e4dce",
            "-C", "extra-filename=-c7101a3d6c8e4dce",
            "--out-dir", "$PWD/target/debug/build/rustix-c7101a3d6c8e4dce",                   // state.out_dir =+> state.target_path
            "-C", "linker=/usr/bin/clang",
            "-L", "dependency=$PWD/target/debug/deps",
            "--cap-lints", "warn",
            "-C", "link-arg=-fuse-ld=/usr/local/bin/mold",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: Default::default(),
                mdid: Some("c7101a3d6c8e4dce".into()),
                incremental: None,
                input: as_argument("$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/rustix-0.38.20/build.rs").into(),
                out_dir: as_argument("$PWD/target/debug/build/rustix-c7101a3d6c8e4dce").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  as_arguments(&[
                "--cap-lints=warn",
                "--cfg='feature=\"alloc\"'",
                "--cfg='feature=\"default\"'",
                "--cfg='feature=\"std\"'",
                "--cfg='feature=\"termios\"'",
                "--cfg='feature=\"use-libc-auxv\"'",
                "--crate-name=build_script_build",
                "--crate-type=bin",
             // "--diagnostic-width", "211",
                "--edition=2021",
                "--emit=dep-info,link",
                "--error-format=json",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=$PWD/target/debug/build/rustix-c7101a3d6c8e4dce",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-c7101a3d6c8e4dce",
                "-Cmetadata=c7101a3d6c8e4dce",
                "-Ldependency=$PWD/target/debug/deps",
             ]));
    }

    #[test]
    fn args_when_building_proc_macro() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.73.0
        let arguments = as_arguments(&[
            "--crate-name", "time_macros",
            "--edition=2021",
            "$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/time-macros-0.2.14/src/lib.rs",
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "proc-macro",
            "--emit=dep-info,link",
            "-C", "prefer-dynamic",
            "-C", "embed-bitcode=no",
            "-C", "debug-assertions=off",
            "--cfg", "'feature=\"formatting\"'",
            "--cfg", "'feature=\"parsing\"'",
            "-C", "metadata=89438a15ab938e2f",
            "-C", "extra-filename=-89438a15ab938e2f",
            "--out-dir", "/tmp/wfrefwef__cargo-deny_0-14-3/release/deps",
            "-C", "linker=/usr/bin/clang",
            "-L", "dependency=/tmp/wfrefwef__cargo-deny_0-14-3/release/deps",
            "--extern", "time_core=/tmp/wfrefwef__cargo-deny_0-14-3/release/deps/libtime_core-c880e75c55528c08.rlib",
            "--extern", "proc_macro", // oh hi
            "--cap-lints", "warn",
            "-C", "link-arg=-fuse-ld=/usr/local/bin/mold",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: ["libtime_core-c880e75c55528c08.rlib".to_owned()].into(),
                mdid: Some("89438a15ab938e2f".into()),
                incremental: None,
                input: as_argument("$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/time-macros-0.2.14/src/lib.rs").into(),
                out_dir: as_argument("/tmp/wfrefwef__cargo-deny_0-14-3/release/deps").into(),
                target_path: as_argument("/tmp/wfrefwef__cargo-deny_0-14-3/release").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(as_arguments(&[
                "--cap-lints=warn",
                "--cfg='feature=\"formatting\"'",
                "--cfg='feature=\"parsing\"'",
                "--crate-name=time_macros",
                "--crate-type=proc-macro",
             // "--diagnostic-width", "211",
                "--edition=2021",
                "--emit=dep-info,link",
                "--error-format=json",
                "--extern=proc_macro",
                "--extern=time_core=/tmp/wfrefwef__cargo-deny_0-14-3/release/deps/libtime_core-c880e75c55528c08.rlib",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=/tmp/wfrefwef__cargo-deny_0-14-3/release/deps",
                "-Cdebug-assertions=off",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-89438a15ab938e2f",
                "-Cmetadata=89438a15ab938e2f",
                "-Cprefer-dynamic",
                "-Ldependency=/tmp/wfrefwef__cargo-deny_0-14-3/release/deps",
             ]), args);
    }

    #[test]
    fn args_when_building_that_buildrs() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.73.0
        let arguments = as_arguments(&[
            "--crate-name", "build_script_build",
            "--edition=2021",
            "src/build.rs",
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debug-assertions=off",
            "--cfg", "feature=\"default\"",
            "-C", "metadata=96fe5c8493f1a08f",
            "-C", "extra-filename=-96fe5c8493f1a08f",
            "--out-dir", "/tmp/wfrefwef__cross@0.2.5/release/build/cross-96fe5c8493f1a08f",
            "-C", "linker=/usr/bin/clang",
            "-L", "dependency=/tmp/wfrefwef__cross@0.2.5/release/deps",
            "-C", "link-arg=-fuse-ld=/usr/local/bin/mold",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: Default::default(),
                mdid: Some("96fe5c8493f1a08f".into()),
                incremental: None,
                input: as_argument("src/build.rs").into(),
                out_dir: as_argument(
                    "/tmp/wfrefwef__cross@0.2.5/release/build/cross-96fe5c8493f1a08f"
                )
                .into(),
                target_path: as_argument("/tmp/wfrefwef__cross@0.2.5/release").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(as_arguments(&[
                "--cfg=feature=\"default\"",
                "--crate-name=build_script_build",
                "--crate-type=bin",
             // "--diagnostic-width", "211",
                "--edition=2021",
                "--emit=dep-info,link",
                "--error-format=json",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=/tmp/wfrefwef__cross@0.2.5/release/build/cross-96fe5c8493f1a08f",
                "-Cdebug-assertions=off",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-96fe5c8493f1a08f",
                "-Cmetadata=96fe5c8493f1a08f",
                "-Ldependency=/tmp/wfrefwef__cross@0.2.5/release/deps",
             ]), args);
    }

    #[test]
    fn args_when_build_script_main() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.73.0
        let arguments = as_arguments(&[
            "--crate-name", "build_script_main",
            "--edition=2018",
            "$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/openssl-sys-0.9.95/build/main.rs",
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--crate-type", "bin",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debug-assertions=off",
            "-C", "metadata=99f749eccead4467",
            "-C", "extra-filename=-99f749eccead4467",
            "--out-dir", "$HOME/instst/release/build/openssl-sys-99f749eccead4467",
            "-L", "dependency=$HOME/instst/release/deps",
            "--extern", "cc=$HOME/instst/release/deps/libcc-3c316ebdde73b0fe.rlib",
            "--extern", "pkg_config=$HOME/instst/release/deps/libpkg_config-a6962381fee76247.rlib",
            "--extern", "vcpkg=$HOME/instst/release/deps/libvcpkg-ebcbc23bfdf4209b.rlib",
            "--cap-lints", "warn",
        ]);

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: [
                    "libcc-3c316ebdde73b0fe.rlib".to_owned(),
                    "libpkg_config-a6962381fee76247.rlib".to_owned(),
                    "libvcpkg-ebcbc23bfdf4209b.rlib".to_owned(),
                ].into(),
                mdid: Some("99f749eccead4467".into()),
                incremental: None,
                input: as_argument("$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/openssl-sys-0.9.95/build/main.rs").into(),
                out_dir: as_argument(
                    "$HOME/instst/release/build/openssl-sys-99f749eccead4467"
                )
                .into(),
                target_path: as_argument("$HOME/instst/release").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(as_arguments(&[
                "--cap-lints=warn",
                "--crate-name=build_script_main",
                "--crate-type=bin",
                "--edition=2018",
                "--emit=dep-info,link",
                "--error-format=json",
                "--extern=cc=$HOME/instst/release/deps/libcc-3c316ebdde73b0fe.rlib",
                "--extern=pkg_config=$HOME/instst/release/deps/libpkg_config-a6962381fee76247.rlib",
                "--extern=vcpkg=$HOME/instst/release/deps/libvcpkg-ebcbc23bfdf4209b.rlib",
                "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
                "--out-dir=$HOME/instst/release/build/openssl-sys-99f749eccead4467",
                "-Cdebug-assertions=off",
                "-Cembed-bitcode=no",
                "-Cextra-filename=-99f749eccead4467",
                "-Cmetadata=99f749eccead4467",
                "-Ldependency=$HOME/instst/release/deps",
             ]), args);
    }

    #[test]
    fn the_weird_build_script_of_slab_0_4_9() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.80.0
        let arguments = as_arguments(&[
            // CARGO=$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
            // CARGO_CFG_PANIC=unwind
            // CARGO_CFG_TARGET_ABI=''
            // CARGO_CFG_TARGET_ARCH=x86_64
            // CARGO_CFG_TARGET_ENDIAN=little
            // CARGO_CFG_TARGET_ENV=gnu
            // CARGO_CFG_TARGET_FAMILY=unix
            // CARGO_CFG_TARGET_FEATURE=fxsr,sse,sse2
            // CARGO_CFG_TARGET_HAS_ATOMIC=16,32,64,8,ptr
            // CARGO_CFG_TARGET_OS=linux
            // CARGO_CFG_TARGET_POINTER_WIDTH=64
            // CARGO_CFG_TARGET_VENDOR=unknown
            // CARGO_CFG_UNIX=''
            // CARGO_ENCODED_RUSTFLAGS=''
            // CARGO_FEATURE_DEFAULT=1
            // CARGO_FEATURE_STD=1
            // CARGO_MANIFEST_DIR=$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/slab-0.4.9
            // CARGO_PKG_AUTHORS='Carl Lerche <me@carllerche.com>'
            // CARGO_PKG_DESCRIPTION='Pre-allocated storage for a uniform data type'
            // CARGO_PKG_HOMEPAGE=''
            // CARGO_PKG_LICENSE=MIT
            // CARGO_PKG_LICENSE_FILE=''
            // CARGO_PKG_NAME=slab
            // CARGO_PKG_README=README.md
            // CARGO_PKG_REPOSITORY='https://github.com/tokio-rs/slab'
            // CARGO_PKG_RUST_VERSION=1.31
            // CARGO_PKG_VERSION=0.4.9
            // CARGO_PKG_VERSION_MAJOR=0
            // CARGO_PKG_VERSION_MINOR=4
            // CARGO_PKG_VERSION_PATCH=9
            // CARGO_PKG_VERSION_PRE=''
            // DEBUG=true
            // HOST=x86_64-unknown-linux-gnu
            // LD_LIBRARY_PATH='$HOME/work/supergreen/supergreen/target/debug/deps:$HOME/work/supergreen/supergreen/target/debug:$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/x86_64-unknown-linux-gnu/lib:$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib'
            // NUM_JOBS=1
            // OPT_LEVEL=0
            // OUT_DIR=$HOME/work/supergreen/supergreen/target/debug/build/slab-94793bb2b78c57b5/out  <===
            // PROFILE=debug
            // RUSTC=$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc
            // RUSTC_WRAPPER=$HOME/.cargo/bin/rustcbuildx
            // RUSTDOC=$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustdoc
            // TARGET=x86_64-unknown-linux-gnu
            "$HOME/work/supergreen/supergreen/target/debug/build/slab-b0340a0384800aca/build-script-build",
        ]);

        let out_dir_var = Some(Utf8PathBuf::from(
            "$HOME/work/supergreen/supergreen/target/debug/build/slab-94793bb2b78c57b5/out",
        ));

        let (st, args) = as_rustc(PWD.into(), &arguments, out_dir_var.as_deref()).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                externs: [].into(),
                mdid: Some("94793bb2b78c57b5".into()),
                incremental: None,
                input: as_argument(
                    "$HOME/work/supergreen/supergreen/target/debug/build/slab-b0340a0384800aca/build-script-build"
                )
                .into(),
                out_dir: "".into(),
                target_path: "$HOME/work/supergreen/supergreen/target/debug".into(),
            }
        );
        assert_eq!(Vec::<String>::new(), args);
    }
    /// A crate's call, then `extra` as cargo appends `$RUSTFLAGS` (and `[lints]` to its own).
    fn call(extra: &str) -> Vec<String> {
        #[rustfmt::skip]
        let mut arguments = as_arguments(&[
            "--crate-name", "mycrate",
            "--edition=2021",
            "src/lib.rs",
            "--crate-type", "lib",
            "--out-dir", "$PWD/target/debug/deps",
            "-C", "extra-filename=-0123456789abcdef",
            "-L", "dependency=$PWD/target/debug/deps",
        ]);
        arguments.extend(extra.split_whitespace().map(as_argument));
        let (_, args) = as_rustc(PWD.into(), &arguments, None).unwrap();
        args
    }

    /// Without what [`call`] always has.
    fn extra(args: &[String]) -> String {
        let always = call("");
        args.iter().filter(|arg| !always.contains(arg)).cloned().collect::<Vec<_>>().join(" ")
    }

    /// Glued or valueless options were paired with whatever came next, or dropped when last.
    #[test]
    fn rustflags_are_not_dropped() {
        assert_eq!(extra(&call("-Dwarnings")), "-Dwarnings");
        assert_eq!(extra(&call("-g")), "-g");
        assert_eq!(extra(&call("-g -g -O")), "-O -g -g");
        assert_eq!(extra(&call("-gO")), "-O -g");
        assert_eq!(extra(&call("-gDwarnings")), "-Dwarnings -g");
        // Options we know nothing of get read by their shape
        assert_eq!(extra(&call("--some-new-opt x -Ynew")), "--some-new-opt=x -Ynew");
        assert_eq!(
            extra(&call("-Dwarnings -Ctarget-cpu=native")),
            "-Ctarget-cpu=native -Dwarnings"
        );
    }

    /// However they're spelled.
    #[test]
    fn arguments_are_canonical() {
        let canonical =
            call("-C opt-level=3 -D warnings -W unused -L native=/x -l foo --cfg tokio_unstable");
        for spelling in [
            "-Copt-level=3 -Dwarnings -Wunused -Lnative=/x -lfoo --cfg=tokio_unstable",
            "--codegen opt-level=3 --deny warnings --warn unused -L native=/x -l foo --cfg tokio_unstable",
            "--codegen=opt_level=3 --deny=warnings --warn=unused -Lnative=/x -lfoo --cfg tokio_unstable",
            "--cfg tokio_unstable -lfoo -Dwarnings -Lnative=/x -Wunused -Copt-level=3",
        ] {
            assert_eq!(call(spelling), canonical, "{spelling}");
        }
    }

    /// rustc reads some arguments in order: those keep theirs, the rest gets sorted.
    #[test]
    fn order_sensitive_arguments_keep_their_order() {
        let args = call(
            "--extern b=/p/libb.rlib --extern a=/p/liba.rlib --cfg b --cfg a \
             --deny unused --allow unused_imports \
             -L native=/usr/lib -L native=/p/target/debug/build/x-0123456789abcdef/out \
             -l ssl -l crypto \
             -C link-arg=-Wl,-b -C link-args=-Wl,-a -C opt-level=3 -C opt-level=0 \
             --remap-path-prefix=/p=/x --remap-path-prefix=/p/a=/y",
        );
        assert_eq!(
            extra(&args).split(' ').collect::<Vec<_>>(),
            [
                "--cfg=a",
                "--cfg=b",
                "--extern=a=/p/liba.rlib",
                "--extern=b=/p/libb.rlib",
                "--remap-path-prefix=/p=/x",
                "--remap-path-prefix=/p/a=/y",
                "-Clink-arg=-Wl,-b",
                "-Clink-args=-Wl,-a",
                "-Copt-level=3",
                "-Copt-level=0",
                "-Lnative=/usr/lib",
                "-Lnative=/p/target/debug/build/x-0123456789abcdef/out",
                "-Dunused",
                "-Aunused_imports",
                "-lssl",
                "-lcrypto",
            ]
        );
    }

    #[test]
    fn argfiles_are_expanded() {
        let fs = FakeFs::new();
        fs.file("/p/args.txt", "mycrate\n--edition=2021\n-C\nlink-arg=-Wl,a b\n");
        let _guard = Sys::install(Sys { fs, ..Sys::fake() });

        let arguments = ["--crate-name", "@args.txt", "@shell:x"].map(ToOwned::to_owned).into();
        assert_eq!(
            expand_argfiles("/p".into(), arguments).unwrap(),
            ["--crate-name", "mycrate", "--edition=2021", "-C", "link-arg=-Wl,a b", "@shell:x"]
        );

        let err = expand_argfiles("/p".into(), vec!["@nope.txt".to_owned()]).unwrap_err();
        assert!(err.to_string().starts_with("Failed reading argfile /p/nope.txt: "), "{err}");
    }

    /// Path values get rewritten before being glued to their flag: `-L/host/target/...` has no
    /// token boundary before the path.
    #[test]
    fn values_are_mapped_before_being_glued() {
        let mut args = call("-L /p/target/debug/build/x-0123456789abcdef/out -o /p/target/x");
        args.retain(|arg| arg.starts_with("-L/") || arg.starts_with("-o"));
        assert_eq!(args, ["-L/p/target/debug/build/x-0123456789abcdef/out", "-o/p/target/x"]);

        let (_, args) = super::as_rustc(PWD.into(), &call("-L /p/target/x"), None).unwrap();
        let args: Vec<_> = args
            .into_iter()
            .map(|arg| arg.map(|v| v.replace("/p/target/", "/target/")).to_string())
            .filter(|arg| arg.starts_with("-L/"))
            .collect();
        assert_eq!(args, ["-L/target/x"]);
    }

    /// cargo hands over an `@argfile` once spawning rustc fails with `E2BIG`: arguments over
    /// `ARG_MAX` (often 2MiB, see `getconf ARG_MAX`), as with thousands of `--extern`s.
    #[test]
    fn a_call_too_long_for_a_command_line() {
        const ARG_MAX: usize = 2 * 1024 * 1024;
        let deps = as_argument("$PWD/target/debug/deps");
        let externs: Vec<String> =
            (0..25_000).map(|i| format!("--extern\nc{i}={deps}/libc{i}-{i:016x}.rlib\n")).collect();
        let argfile = externs.concat();
        assert!(argfile.len() > ARG_MAX, "{}", argfile.len());

        let fs = FakeFs::new();
        fs.file("/tmp/args", argfile);
        let _guard = Sys::install(Sys { fs, ..Sys::fake() });

        let mut arguments = call("");
        arguments.push("@/tmp/args".to_owned());
        let arguments = expand_argfiles(PWD.into(), arguments).unwrap();

        let (st, args) = as_rustc(PWD.into(), &arguments, None).unwrap();
        assert_eq!(st.externs.len(), 25_000);
        assert_eq!(args.len(), call("").len() + 25_000);
        assert!(args.contains(&format!("--extern=c24999={deps}/libc24999-00000000000061a7.rlib")));

        // FIXME: the call then goes inline into a `RUN`, which BuildKit hands to `sh -c` as a
        // single argument: over `MAX_ARG_STRLEN` (128KiB) that fails with `E2BIG`. Calls between
        // 128KiB and `ARG_MAX` get no `@argfile` from cargo either. Fix: past some size, write the
        // arguments to a file (e.g. a `COPY <<EOF` heredoc, mounted) then call `rustc @file`.
        const MAX_ARG_STRLEN: usize = 128 * 1024;
        assert!(args.join(" ").len() > MAX_ARG_STRLEN);
    }

    #[test]
    fn unexpected_arguments_are_errors() {
        for args in [&["-Q"][..], &["-C"], &["--cfg"], &["-"], &["a.rs", "b.rs"], &["--test=x"]] {
            let args = args.iter().map(|&a| a.to_owned()).collect::<Vec<_>>();
            assert!(as_rustc(PWD.into(), &args, None).is_err(), "{args:?}");
        }
    }
}
