use std::{env, sync::OnceLock};

use camino::{Utf8Path, Utf8PathBuf};

use crate::all_our_envs::CARGO_TARGET_DIR;

const REWRITE_TARGETDIR: bool = true; // TODO: turn into a CARGOGREEN_EXPERIMENT

pub(crate) const VIRTUAL_TARGET_DIR: &str = "/target/";

static TARGET_DIR: OnceLock<Utf8PathBuf> = OnceLock::new();

/// The `cargo green` parent calls this once, with `create_current_target_dir`'s
/// value: its own environment never has ${CARGO_TARGET_DIR} set. Wrapper
/// subprocesses don't call it: cargo hands them the actual value.
pub(crate) fn set_target_dir(dir: impl Into<Utf8PathBuf>) {
    let _ = TARGET_DIR.set(dir.into());
}

pub(crate) fn target_dir() -> &'static Utf8Path {
    TARGET_DIR.get_or_init(|| {
        env::var(CARGO_TARGET_DIR!())
            .unwrap_or_else(|_| panic!("BUG: {CARGO_TARGET_DIR} is unset (or not utf-8 encoded)"))
            .into()
    })
}

pub(crate) fn un_virtual_target_dir_str(txt: &str) -> String {
    if !REWRITE_TARGETDIR {
        return txt.to_owned();
    }
    replace_carefully(txt, VIRTUAL_TARGET_DIR, target_dir().as_str())
}

pub(crate) fn virtual_target_dir_str(txt: &str) -> String {
    if !REWRITE_TARGETDIR {
        return txt.to_owned();
    }
    replace_carefully(txt, target_dir().as_str(), VIRTUAL_TARGET_DIR)
}

#[expect(clippy::let_and_return)]
pub(crate) fn replace_carefully(txt: &str, from: &str, to: &str) -> String {
    let txt = if txt.starts_with(from) { txt.replacen(from, to, 1) } else { txt.to_owned() };
    let txt = txt.replace(&format!("\n{from}"), &format!("\n{to}"));
    let txt = txt.replace(&format!(" {from}"), &format!(" {to}"));
    let txt = txt.replace(&format!("'{from}"), &format!("'{to}"));
    let txt = txt.replace(&format!("\"{from}"), &format!("\"{to}"));
    let txt = txt.replace(&format!("={from}"), &format!("={to}"));
    let txt = txt.replace(&format!("`{from}"), &format!("`{to}"));
    txt
}

pub(crate) fn virtual_target_dir(path: &Utf8Path) -> Utf8PathBuf {
    if !REWRITE_TARGETDIR {
        return path.to_owned();
    }
    path.strip_prefix(target_dir())
        .map(|path| Utf8Path::new(VIRTUAL_TARGET_DIR).join(path))
        .unwrap_or_else(|_| path.to_owned())
}

/// Set to `$CARGO_TARGET_DIR/$PROFILE` when cross-compiling, `None` otherwise.
/// Never to `$CARGO_TARGET_DIR/<target triple>/$PROFILE`: that's `target_path`.
#[must_use]
pub(crate) fn host_profile_dir(target_path: &Utf8Path) -> Option<Utf8PathBuf> {
    let profile = target_path.file_name()?; // "release" | "debug" | $PROFILE
    let host = target_dir().join(profile);
    (host != target_path).then_some(host)
}

/// Cross-compilation -safe way of making target paths.
#[must_use]
pub(crate) fn locate_path(
    f: impl Fn(&Utf8Path) -> Utf8PathBuf,
    target_path: &Utf8Path,
    host_path: Option<&Utf8Path>,
) -> Utf8PathBuf {
    if let Some(host_path) = host_path {
        let host = f(host_path);
        if host.exists() {
            return host;
        }
    }
    f(target_path) // `Md::from_file` can emit its helpful not-found message
}

#[test]
fn target_dir_var() {
    temp_env::with_var(CARGO_TARGET_DIR!(), Some("/some/path/"), || {
        assert_eq!(target_dir().as_str(), "/some/path/");

        assert_eq!(host_profile_dir("/some/path/release".into()), None);
        assert_eq!(
            host_profile_dir("/some/path/armv7-unknown-linux-musleabihf/release".into()),
            Some("/some/path/release".into())
        );

        assert_eq!(
            virtual_target_dir("/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d".into()),
            "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d"
        );

        assert_eq!(
            virtual_target_dir_str(
                "/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
            ),
            "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
        );

        assert_eq!(
            virtual_target_dir_str(
                "/some/path/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
            ),
            "/target/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
        );

        assert_eq!(
            un_virtual_target_dir_str(
                "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
            ),
            "/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
        );

        assert_eq!(
            un_virtual_target_dir_str(
                "/target/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
            ),
            "/some/path/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
        );

        assert_eq!(
            un_virtual_target_dir_str(
                "error: couldn't read `/target/armv7-unknown-linux-musleabihf/release/build/pb-bd1e88e219ae6eda/out/hypercards.rs`: No such file or directory (os error 2)"
            ),
            "error: couldn't read `/some/path/armv7-unknown-linux-musleabihf/release/build/pb-bd1e88e219ae6eda/out/hypercards.rs`: No such file or directory (os error 2)"
        );
    });
}
