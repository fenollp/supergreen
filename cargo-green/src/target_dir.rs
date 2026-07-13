use std::{env, sync::LazyLock};

use camino::{Utf8Path, Utf8PathBuf};

const REWRITE_TARGETDIR: bool = true; // TODO: turn into a CARGOGREEN_EXPERIMENT

pub(crate) const VIRTUAL_TARGET_DIR: &str = "/target/";

pub(crate) static TARGET_DIR: LazyLock<Utf8PathBuf> = LazyLock::new(|| {
    env::var("CARGO_TARGET_DIR")
        .expect("BUG: $CARGO_TARGET_DIR is unset (or not utf-8 encoded)")
        .into()
});

pub(crate) fn un_virtual_target_dir_str(txt: &str) -> String {
    if !REWRITE_TARGETDIR {
        return txt.to_owned();
    }
    replace_carefully(txt, VIRTUAL_TARGET_DIR, TARGET_DIR.as_str())
}

pub(crate) fn virtual_target_dir_str(txt: &str) -> String {
    if !REWRITE_TARGETDIR {
        return txt.to_owned();
    }
    replace_carefully(txt, TARGET_DIR.as_str(), VIRTUAL_TARGET_DIR)
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
    path.strip_prefix(TARGET_DIR.as_path())
        .map(|path| Utf8Path::new(VIRTUAL_TARGET_DIR).join(path))
        .unwrap_or_else(|_| path.to_owned())
}

/// Set to `$CARGO_TARGET_DIR/$PROFILE` when cross-compiling, `None` otherwise.
/// Never to `$CARGO_TARGET_DIR/<target triple>/$PROFILE`: that's `target_path`.
#[must_use]
pub(crate) fn host_profile_dir(target_path: &Utf8Path) -> Option<Utf8PathBuf> {
    let profile = target_path.file_name()?; // "release" | "debug" | $PROFILE
    let host = Utf8Path::new(TARGET_DIR.as_str()).join(profile);
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
    temp_env::with_var("CARGO_TARGET_DIR", Some("/some/path/"), || {
        assert_eq!(TARGET_DIR.as_str(), "/some/path/");

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
