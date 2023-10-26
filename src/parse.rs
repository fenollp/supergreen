use std::{collections::BTreeSet, path::Path};

use anyhow::{bail, Result};

/// RustcArgs contains parts of `rustc`'s arguments
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RustcArgs {
    /// 1: --crate-type
    pub(crate) crate_type: String,

    /// 0..: --extern=EXTERN | --extern EXTERN
    pub(crate) externs: BTreeSet<String>,

    /// 1: -C extra-filename=EXTRA_FILENAME
    pub(crate) extra_filename: String,

    /// 0|1: -C incremental=INCREMENTAL
    pub(crate) incremental: Option<String>,

    /// 1: plain path to (non-empty existing) file
    pub(crate) input: String,

    /// 1: --out-dir OUT_DIR
    pub(crate) out_dir: String,

    /// Target path:
    pub(crate) target_path: String,
}

pub(crate) fn as_rustc(
    pwd: &str,
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
            state.input = key.as_str().replace("/./", "/");
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
                Some(("extra-filename", v)) => {
                    assert_eq!(state.extra_filename, "");
                    state.extra_filename = v.to_owned();
                }
                Some(("incremental", v)) => {
                    assert_eq!(state.incremental, None);
                    state.incremental = Some(v.to_owned());
                }
                _ => {}
            },
            "-L" => {
                if let Some(("dependency", v)) = val.split_once('=') {
                    if !v.starts_with('/') {
                        val = format!("dependency={}", Path::new(pwd).join(v).to_string_lossy());
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
                let xtern = Path::new(xtern).file_name().unwrap_or_default().to_string_lossy();
                state.externs.insert(xtern.to_string());
            }
            "--out-dir" => {
                assert_eq!(state.out_dir, "");
                state.out_dir = val.clone();
                if !state.out_dir.starts_with('/') {
                    // TODO: decide whether $PWD is an issue. Maybe CARGO_TARGET_DIR can help?
                    state.out_dir = Path::new(pwd).join(&val).to_string_lossy().to_string();
                }
                val = state.out_dir.clone();
            }
            _ => {}
        }

        args.push(key.clone());
        args.push(val);
    }

    assert!(!state.crate_type.is_empty());
    assert!(!state.extra_filename.is_empty());
    assert!(!state.incremental.as_ref().map(String::is_empty).unwrap_or_default()); // MAY be unset: only set on last calls
    assert!(!state.input.is_empty());
    assert!(!state.out_dir.is_empty());

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
    state.target_path = if state.out_dir.ends_with("/deps") {
        state.out_dir[..(state.out_dir.len() - "/deps".len())].to_owned() // Drop /deps suffix
    } else if state.out_dir.contains("/build/") {
        let p = Path::new(&state.out_dir);
        loop {
            let Some(p) = p.parent() else { break };
            if p.to_string_lossy().ends_with("/build") {
                break;
            }
        }
        p.to_string_lossy().to_string()
    } else {
        bail!("BUG: --out-dir path should match /deps$|.+/build/.+: {}", state.out_dir)
    };

    Ok((state, args))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::parse::{as_rustc, RustcArgs};

    #[test]
    fn args_when_building_final_binary() {
        let home = "/home/maison".to_owned();
        let pwd = "$HOME/⺟/rustcbuildx.git".to_owned();

        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments: Vec<_> = [
            "$PWD/./dbg/debug/rustcbuildx",                                                   // this
            "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",             // rustc
            "--crate-name", "rustcbuildx",                                                    // state.crate_name
            "--edition=2021",
            "src/main.rs",                                                                    // state.input
            "--error-format=json",
            "--json=diagnostic-rendered-ansi,artifacts,future-incompat",
            "--diagnostic-width=211",
            "--crate-type", "bin",                                                            // state.crate_type~
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "-C", "metadata=710b4516f388a5e4",
            "-C", "extra-filename=-710b4516f388a5e4",                                         // state.extra_filename
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
        ].into_iter().map(ToOwned::to_owned)
                     .map(|x|x.replace("$PWD", &pwd))
                     .map(|x|x.replace("$HOME", &home))
                     .collect();

        let (st, args) = as_rustc(&pwd, env!("CARGO_PKG_NAME"), arguments.clone(), false).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                crate_type: "bin".to_owned(),
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
                extra_filename: "-710b4516f388a5e4".to_owned(),
                incremental: Some(
                    "/home/maison/⺟/rustcbuildx.git/target/debug/incremental".to_owned()
                ),
                input: "src/main.rs".to_owned(),
                out_dir: "/home/maison/⺟/rustcbuildx.git/target/debug/deps".to_owned(),
                target_path: "/home/maison/⺟/rustcbuildx.git/target/debug".to_owned(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  [
                "/home/maison/⺟/rustcbuildx.git/./dbg/debug/rustcbuildx",
                "/home/maison/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
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
                "--out-dir", "/home/maison/⺟/rustcbuildx.git/target/debug/deps",
                "-C", "incremental=/home/maison/⺟/rustcbuildx.git/target/debug/incremental",
                "-L", "dependency=/home/maison/⺟/rustcbuildx.git/target/debug/deps",
                "--extern", "anyhow=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern", "env_logger=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern", "log=/home/maison/⺟/rustcbuildx.git/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern", "mktemp=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern", "os_pipe=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
             ].into_iter().map(ToOwned::to_owned).collect::<Vec<_>>());
    }

    #[test]
    fn args_when_building_final_test() {
        let home = "/home/maison".to_owned();
        let pwd = "$HOME/⺟/rustcbuildx.git".to_owned();

        #[rustfmt::skip]
        // Original ordering per rustc 1.69.0 (84c898d65 2023-04-16)
        let arguments: Vec<_> = [
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
            "-C", "metadata=7c7a0950383d41d3",
            "-C", "extra-filename=-7c7a0950383d41d3",                                         // state.extra_filename
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
        ].into_iter().map(ToOwned::to_owned)
                     .map(|x|x.replace("$PWD", &pwd))
                     .map(|x|x.replace("$HOME", &home))
                     .collect();

        let (st, args) = as_rustc(&pwd, env!("CARGO_PKG_NAME"), arguments.clone(), false).unwrap();

        assert_eq!(
            st,
            RustcArgs {
                crate_type: "test".to_owned(),
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
                extra_filename: "-7c7a0950383d41d3".to_owned(),
                incremental: Some(
                    "/home/maison/⺟/rustcbuildx.git/target/debug/incremental".to_owned()
                ),
                input: "src/main.rs".to_owned(),
                out_dir: "/home/maison/⺟/rustcbuildx.git/target/debug/deps".to_owned(),
                target_path: "/home/maison/⺟/rustcbuildx.git/target/debug".to_owned(),
            }
        );

        #[rustfmt::skip]
        assert_eq!(args,  [
                "/home/maison/⺟/rustcbuildx.git/./dbg/debug/rustcbuildx",
                "/home/maison/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
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
                "--out-dir", "/home/maison/⺟/rustcbuildx.git/target/debug/deps",
                "-C", "incremental=/home/maison/⺟/rustcbuildx.git/target/debug/incremental",
                "-L", "dependency=/home/maison/⺟/rustcbuildx.git/target/debug/deps",
                "--extern", "anyhow=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libanyhow-f96497119bad6f50.rlib",
                "--extern", "env_logger=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libenv_logger-7e2d283f6e473671.rlib",
                "--extern", "log=/home/maison/⺟/rustcbuildx.git/target/debug/deps/liblog-27d1dc50ab631e5f.rlib",
                "--extern", "mktemp=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libmktemp-b84fe47f0a44f88d.rlib",
                "--extern", "os_pipe=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libos_pipe-f344e452b9bd1c5e.rlib",
                "--extern", "pretty_assertions=/home/maison/⺟/rustcbuildx.git/target/debug/deps/libpretty_assertions-9fa55d8a39fa5fe3.rlib",
             ].into_iter().map(ToOwned::to_owned).collect::<Vec<_>>());
    }
}
