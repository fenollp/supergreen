use std::collections::BTreeSet;

use anyhow::{bail, Result};
use camino::{Utf8Path, Utf8PathBuf};

use crate::pops::Popped;

/// RustcArgs contains parts of `rustc`'s arguments
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RustcArgs {
    /// 1: --crate-type
    pub(crate) crate_type: String,

    /// 1: --emit=EMIT | --emit EMIT
    pub(crate) emit: String,

    /// 0..: --extern=EXTERN | --extern EXTERN
    pub(crate) externs: BTreeSet<String>,

    /// 1: -C metadata=METADATA
    pub(crate) metadata: String,

    /// 0|1: -C incremental=INCREMENTAL
    pub(crate) incremental: Option<Utf8PathBuf>,

    /// 1: plain path to (non-empty existing) file
    pub(crate) input: Utf8PathBuf,

    /// 1: --out-dir OUT_DIR
    pub(crate) out_dir: Utf8PathBuf,

    /// Target path:
    pub(crate) target_path: Utf8PathBuf,
}

pub(crate) fn as_rustc(
    pwd: impl AsRef<Utf8Path>,
    crate_name: &str,
    arguments: Vec<String>,
    debug: bool,
) -> Result<(RustcArgs, Vec<String>)> {
    let mut args = vec![];

    let mut state: RustcArgs = Default::default();

    // TODO: find something that sets value only once

    let mut s_e = true;
    let mut key = arguments.first().expect("PROOF: defo not empty").clone();

    #[allow(unused_assignments)] // TODO: but why
    let mut val = String::new();
    for arg in arguments.iter().skip(1) {
        (s_e, key, val) = if s_e {
            (!s_e, key, arg.clone()) // end
        } else {
            (!s_e, arg.clone(), "".to_owned()) // start
        };
        if s_e && val.is_empty() && arg.starts_with("--") && arg.contains('=') {
            let (lhs, rhs) = arg.split_once('=').expect("arg contains '='");
            (s_e, key, val) = (!s_e, lhs.to_owned(), rhs.to_owned());
        }

        if val.is_empty()
            && (key.starts_with('/') || key.ends_with("src/lib.rs") || key.ends_with("src/main.rs"))
        {
            assert_eq!(state.input, "");
            // For e.g. $HOME/.cargo/registry/src/github.com-1ecc6299db9ec823/ahash-0.7.6/./build.rs
            state.input = key.as_str().replace("/./", "/").into();
            (s_e, key) = (false, "".to_owned());
            continue;
        }

        if s_e && key == "--test" && val.is_empty() {
            assert_eq!(state.crate_type, "");
            state.crate_type = "test".to_owned(); // Not a real `--crate-type`
            (s_e, key) = (false, "".to_owned());
            args.push("--test".to_owned());
            continue;
        }

        if val.is_empty() {
            continue;
        }

        // FIXME: drop (strips out local config for now)
        match (key.as_str(), val.as_str()) {
            ("-C", "link-arg=-fuse-ld=/usr/local/bin/mold") => {
                (s_e, key) = (false, "".to_owned());
                continue;
            }
            ("-C", "linker=/usr/bin/clang") => {
                (s_e, key) = (false, "".to_owned());
                continue;
            }
            ("--json", "diagnostic-rendered-ansi,artifacts,future-incompat") if debug => {
                // remove coloring in output for readability during debug
                val = "artifacts,future-incompat".to_owned();
            }
            _ => {}
        }

        match key.as_str() {
            "-C" => match val.split_once('=') {
                Some(("metadata", v)) => {
                    assert_eq!(state.metadata, "");
                    state.metadata = v.to_owned();
                }
                Some(("incremental", v)) => {
                    assert_eq!(state.incremental, None);
                    state.incremental = Some(Utf8PathBuf::from(v));
                }
                _ => {}
            },
            "-L" => {
                if let Some(("dependency", v)) = val.split_once('=') {
                    if !v.starts_with('/') {
                        val = format!("dependency={}", pwd.as_ref().join(v));
                    }
                }
            }
            "--crate-name" => {
                assert_eq!(val, crate_name);
                assert!(!crate_name.is_empty());
            }
            "--crate-type" => {
                assert_eq!(state.crate_type, "");
                assert!(["bin", "lib", "proc-macro"].contains(&val.as_str()));
                state.crate_type = val.clone();
            }
            "--emit" => {
                assert_eq!(state.emit, "");
                // For instance: dep-info,link dep-info,metadata dep-info,metadata,link
                state.emit = val.clone();
            }
            "--extern" => {
                if ["alloc", "core", "proc_macro", "std", "test"].contains(&val.as_str()) {
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
                    // TODO: decide whether $PWD is an issue. Maybe CARGO_TARGET_DIR can help?
                    state.out_dir = pwd.as_ref().join(&val);
                }
                val = state.out_dir.to_string();
            }
            _ => {}
        }

        args.push(key.clone());
        args.push(val);
    }

    assert_ne!(state.crate_type, "");
    assert_ne!(state.metadata, "");
    assert!(!state.incremental.as_ref().map(|x| x == "").unwrap_or_default()); // MAY be unset: only set on last calls
    assert_ne!(state.input, "");
    assert_ne!(state.out_dir, "");

    // https://github.com/rust-lang/cargo/issues/12099
    // Sometimes, a proc-macro crate that depends on sysroot crate `proc_macro` is missing `--extern proc_macro` rustc flag.
    // So add it here or it won't compile. (e.g. openssl-macros-0.1.0-024d32b3f7af0a4f)
    // {"message":"unresolved import `proc_macro`","code":{"code":"E0432","explanation":"An import was unresolved.\n\nErroneous code example:\n\n```compile_fail,E0432\nuse something::Foo; // error: unresolved import `something::Foo`.\n```\n\nIn Rust 2015, paths in `use` statements are relative to the crate root. To\nimport items relative to the current and parent modules, use the `self::` and\n`super::` prefixes, respectively.\n\nIn Rust 2018 or later, paths in `use` statements are relative to the current\nmodule unless they begin with the name of a crate or a literal `crate::`, in\nwhich case they start from the crate root. As in Rust 2015 code, the `self::`\nand `super::` prefixes refer to the current and parent modules respectively.\n\nAlso verify that you didn't misspell the import name and that the import exists\nin the module from where you tried to import it. Example:\n\n```\nuse self::something::Foo; // Ok.\n\nmod something {\n    pub struct Foo;\n}\n# fn main() {}\n```\n\nIf you tried to use a module from an external crate and are using Rust 2015,\nyou may have missed the `extern crate` declaration (which is usually placed in\nthe crate root):\n\n```edition2015\nextern crate core; // Required to use the `core` crate in Rust 2015.\n\nuse core::any;\n# fn main() {}\n```\n\nSince Rust 2018 the `extern crate` declaration is not required and\nyou can instead just `use` it:\n\n```edition2018\nuse core::any; // No extern crate required in Rust 2018.\n# fn main() {}\n```\n"},"level":"error","spans":[{"file_name":"/home/pete/.cargo/registry/src/github.com-1ecc6299db9ec823/openssl-macros-0.1.0/src/lib.rs","byte_start":4,"byte_end":14,"line_start":1,"line_end":1,"column_start":5,"column_end":15,"is_primary":true,"text":[{"text":"use proc_macro::TokenStream;","highlight_start":5,"highlight_end":15}],"label":"use of undeclared crate or module `proc_macro`","suggested_replacement":null,"suggestion_applicability":null,"expansion":null}],"children":[{"message":"there is a crate or module with a similar name","code":null,"level":"help","spans":[{"file_name":"/home/pete/.cargo/registry/src/github.com-1ecc6299db9ec823/openssl-macros-0.1.0/src/lib.rs","byte_start":4,"byte_end":14,"line_start":1,"line_end":1,"column_start":5,"column_end":15,"is_primary":true,"text":[{"text":"use proc_macro::TokenStream;","highlight_start":5,"highlight_end":15}],"label":null,"suggested_replacement":"proc_macro2","suggestion_applicability":"MaybeIncorrect","expansion":null}],"children":[],"rendered":null}],"rendered":"error[E0432]: unresolved import `proc_macro`\n --> /home/pete/.cargo/registry/src/github.com-1ecc6299db9ec823/openssl-macros-0.1.0/src/lib.rs:1:5\n  |\n1 | use proc_macro::TokenStream;\n  |     ^^^^^^^^^^ use of undeclared crate or module `proc_macro`\n  |\nhelp: there is a crate or module with a similar name\n  |\n1 | use proc_macro2::TokenStream;\n  |     ~~~~~~~~~~~\n\n"}
    if state.crate_type == "proc-macro"
        && !arguments.iter().any(|arg| arg == "--extern=proc_macro")
    // TODO: just in-loop set has_extern_proc_macro
        && !arguments.to_vec().join(" ").contains("--extern proc_macro")
    {
        args.append(&mut vec!["--extern".to_owned(), "proc_macro".to_owned()]);
    }

    // Can't rely on $PWD nor $CARGO_TARGET_DIR because `cargo` changes them.
    // Out dir though...
    // --out-dir "$CARGO_TARGET_DIR/$PROFILE"/build/rustix-2a01a00f5bdd1924
    // --out-dir "$CARGO_TARGET_DIR/$PROFILE"/deps
    state.target_path = match &state.out_dir.iter().rev().take(3).collect::<Vec<_>>()[..] {
        ["deps", ..] => state.out_dir.clone().popped(1),
        [_crate_dir, "build", ..] => state.out_dir.clone().popped(2),
        nope => bail!("BUG: --out-dir path should match /deps$|.+/build/.+: {nope:?}"),
    };
    // TODO: make conversion Dockerfile <> HCL easier (just change extension)
    // TODO: return path makers through closures
    // TODO: namespace our files: {target_path}/{NS}/{profile}/...

    Ok((state, args))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::parse::{as_rustc, RustcArgs};

    const HOME: &str = "/home/maison";
    const PWD: &str = "$HOME/⺟/rustcbuildx.git";

    fn as_argument(arg: &str) -> String {
        arg.to_owned().replace("$PWD", PWD).replace("$HOME", HOME)
    }

    fn as_arguments(args: &[&str]) -> Vec<String> {
        args.iter().map(|x| as_argument(x)).collect()
    }

    #[test]
    fn args_when_building_final_binary() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments= as_arguments(&[
            "$PWD/./dbg/debug/rustcbuildx",                                                   // this
            "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",             // rustc
            "--crate-name", "rustcbuildx",                                                    // state.crate_name
            "--edition=2021",
            "src/main.rs",                                                                    // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",                                                            // state.crate_type~
            "--emit=dep-info,link",                                                           // state.emit
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "-C", "metadata=710b4516f388a5e4",                                                // state.metadata
            "-C", "extra-filename=-710b4516f388a5e4",
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

        let (st, args) = as_rustc(PWD, env!("CARGO_PKG_NAME"), arguments.clone(), false).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                crate_type: "bin".to_owned(),
                emit: "dep-info,link".to_owned(),
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
                metadata: "710b4516f388a5e4".to_owned(),
                incremental: Some(as_argument("$PWD/target/debug/incremental").into()),
                input: as_argument("src/main.rs").into(),
                out_dir: as_argument("$PWD/target/debug/deps").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  as_arguments(&[
                "$PWD/./dbg/debug/rustcbuildx",
                "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
                "--crate-name", "rustcbuildx",
                "--edition", "2021",
                "--error-format", "json",
                "--json", "diagnostic-rendered-ansi,artifacts,future-incompat",
                "--diagnostic-width", "211",
                "--crate-type", "bin",
                "--emit", "dep-info,link",
                "-C", "embed-bitcode=no",
                "-C", "debuginfo=2",
                "-C", "metadata=710b4516f388a5e4",
                "-C", "extra-filename=-710b4516f388a5e4",
                "--out-dir", "$PWD/target/debug/deps",
                "-C", "incremental=$PWD/target/debug/incremental",
                "-L", "dependency=$PWD/target/debug/deps",
                "--extern", "anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern", "env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern", "log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern", "mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern", "os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
             ]));
    }

    #[test]
    fn args_when_building_final_test() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments = as_arguments(&[
            "$PWD/./dbg/debug/rustcbuildx",                                                   // this
            "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",             // rustc
            "--crate-name", "rustcbuildx",                                                    // state.crate_name
            "--edition=2021",
            "src/main.rs",                                                                    // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=347",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "--test",
            "-C", "metadata=7c7a0950383d41d3",                                                // state.metadata
            "-C", "extra-filename=-7c7a0950383d41d3",
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

        let (st, args) = as_rustc(PWD, env!("CARGO_PKG_NAME"), arguments.clone(), false).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                crate_type: "test".to_owned(),
                emit: "dep-info,link".to_owned(),
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
                metadata: "7c7a0950383d41d3".to_owned(),
                incremental: Some(as_argument("$PWD/target/debug/incremental").into()),
                input: as_argument("src/main.rs").into(),
                out_dir: as_argument("$PWD/target/debug/deps").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  as_arguments(&[
                "$PWD/./dbg/debug/rustcbuildx",
                "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
                "--crate-name", "rustcbuildx",
                "--edition", "2021",
                "--error-format", "json",
                "--json", "diagnostic-rendered-ansi,artifacts,future-incompat",
                "--diagnostic-width", "347",
                "--emit", "dep-info,link",
                "-C", "embed-bitcode=no",
                "-C", "debuginfo=2",
                "--test",
                "-C", "metadata=7c7a0950383d41d3",
                "-C", "extra-filename=-7c7a0950383d41d3",
                "--out-dir", "$PWD/target/debug/deps",
                "-C", "incremental=$PWD/target/debug/incremental",
                "-L", "dependency=$PWD/target/debug/deps",
                "--extern", "anyhow=$PWD/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern", "env_logger=$PWD/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern", "log=$PWD/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern", "mktemp=$PWD/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern", "os_pipe=$PWD/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
                "--extern", "pretty_assertions=$PWD/target/debug/deps/libpretty_assertions-9fa55d8a39fa5fe3.rlib",
             ]));
    }

    #[test]
    fn args_when_building_build_script() {
        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments = as_arguments(&[
            "$PWD/./dbg/debug/rustcbuildx",                                                   // this
            "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",             // rustc
            "--crate-name", "build_script_build",                                             // state.crate_name
            "--edition=2021",
            "$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/rustix-0.38.20/build.rs", // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",                                                            // state.crate_type~
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

        let (st, args) = as_rustc(PWD, "build_script_build", arguments.clone(), false).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                crate_type: "bin".to_owned(),
                emit: "dep-info,link".to_owned(),
                externs: Default::default(),
                metadata: "c7101a3d6c8e4dce".to_owned(),
                incremental: None,
                input: as_argument("$HOME/.cargo/registry/src/index.crates.io-6f17d22bba15001f/rustix-0.38.20/build.rs").into(),
                out_dir: as_argument("$PWD/target/debug/build/rustix-c7101a3d6c8e4dce").into(),
                target_path: as_argument("$PWD/target/debug").into(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  as_arguments(&[
                "$PWD/./dbg/debug/rustcbuildx",
                "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
                "--crate-name", "build_script_build",
                "--edition", "2021",
                "--error-format", "json",
                "--json", "diagnostic-rendered-ansi,artifacts,future-incompat",
                "--diagnostic-width", "211",
                "--crate-type", "bin",
                "--emit", "dep-info,link",
                "-C", "embed-bitcode=no",
                "--cfg", "'feature=\"alloc\"'",
                "--cfg", "'feature=\"default\"'",
                "--cfg", "'feature=\"std\"'",
                "--cfg", "'feature=\"termios\"'",
                "--cfg", "'feature=\"use-libc-auxv\"'",
                "-C", "metadata=c7101a3d6c8e4dce",
                "-C", "extra-filename=-c7101a3d6c8e4dce",
                "--out-dir", "$PWD/target/debug/build/rustix-c7101a3d6c8e4dce",
                "-L", "dependency=$PWD/target/debug/deps",
                "--cap-lints", "warn",
             ]));
    }
}
