use std::{env, sync::LazyLock};

use camino::{Utf8Path, Utf8PathBuf};

use crate::dirs::pwd;

const REWRITE_TARGETDIR: bool = true; // TODO: turn into a CARGOGREEN_EXPERIMENT

pub(crate) const VIRTUAL_TARGET_DIR: &str = "/target/";

/// Fixed in-container root that local (workspace) crate paths are pinned to, so a crate's stage
/// (and the cwd its compiler bakes into objects) is independent of where the project lives on the
/// host — see [`virtual_pwd_str`]. Dependency paths are pinned via $CARGO_HOME instead.
pub(crate) const VIRTUAL_PWD: &str = "/work";

static TARGET_DIR: LazyLock<Utf8PathBuf> = LazyLock::new(|| {
    env::var("CARGO_TARGET_DIR")
        .expect("BUG: $CARGO_TARGET_DIR is unset (or not utf-8 encoded)")
        .into()
});

/// This rustc-wrapper process's working directory: the crate cargo is currently compiling. cargo
/// spawns one wrapper process per crate, so this is that crate's own directory.
static PWD: LazyLock<Utf8PathBuf> = LazyLock::new(pwd);

/// The host's CARGO_TARGET_DIR — must never appear in a BuildKit artifact (it's `/target` there).
#[must_use]
pub(crate) fn host_target_dir() -> &'static str {
    TARGET_DIR.as_str()
}

/// The host working directory — must never appear in a BuildKit artifact (it's [`VIRTUAL_PWD`]).
#[must_use]
pub(crate) fn host_pwd() -> &'static str {
    PWD.as_str()
}

/// Replace the host working directory prefix with [`VIRTUAL_PWD`]. Must be applied AFTER
/// [`crate::base_image::rewrite_cargo_home`]: dependency paths (under $CARGO_HOME) are rewritten
/// to `$CARGO_HOME/…` first and so no longer carry the host literal, leaving only genuinely-local
/// crate paths for this to pin.
pub(crate) fn virtual_pwd_str(txt: &str) -> String {
    replace_carefully(txt, PWD.as_str(), VIRTUAL_PWD)
}

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

#[test]
fn replace_target_dirs() {
    temp_env::with_var("CARGO_TARGET_DIR", Some("/some/path/"), || {
        assert_eq!(TARGET_DIR.as_str(), "/some/path/");

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

#[test]
fn virtual_pwd_pins_local_paths() {
    // PWD is this process's working directory; a local crate's own paths live under it.
    let here = PWD.as_str();
    assert_eq!(virtual_pwd_str(&format!("{here}/src/lib.rs")), "/work/src/lib.rs");
    assert_eq!(virtual_pwd_str(&format!("CARGO_MANIFEST_DIR={here}")), "CARGO_MANIFEST_DIR=/work");
    // Dependency paths (already $CARGO_HOME-rooted) and unrelated text are left untouched.
    assert_eq!(
        virtual_pwd_str("$CARGO_HOME/registry/src/index.crates.io/anyhow-1.0.100/src/lib.rs"),
        "$CARGO_HOME/registry/src/index.crates.io/anyhow-1.0.100/src/lib.rs"
    );
}
