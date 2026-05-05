use std::{env, sync::LazyLock};

use camino::{Utf8Path, Utf8PathBuf};

const REWRITE_TARGETDIR: bool = true; // TODO: turn into a CARGOGREEN_EXPERIMENT

pub(crate) const VIRTUAL_TARGET_DIR: &str = "/target/";

static TARGET_DIR: LazyLock<Utf8PathBuf> = LazyLock::new(|| {
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

/// `$CARGO_TARGET_DIR/<profile>` — where host artifacts (proc-macros, build scripts and their
/// results) live. Differs from `target_path` only when cross-compiling, where then
/// `target_path` = `$CARGO_TARGET_DIR/<triple>/<profile>`. `None` otherwise.
#[must_use]
pub(crate) fn host_profile_dir(target_path: &Utf8Path) -> Option<Utf8PathBuf> {
    let cargo_target_dir = env::var("CARGO_TARGET_DIR").ok()?;
    let profile = target_path.file_name()?; // "release" | "debug" | <custom profile>
    let host = Utf8Path::new(&cargo_target_dir).join(profile);
    (host != target_path).then_some(host)
}

/// The real `deps/` dir actually holding `file`. When cross-compiling, a crate's own artifacts sit
/// in the triple-nested target subtree, but the proc-macros and build-script outputs it links
/// against stay in the host subtree. Deps are built (and extracted to disk) before any dependent
/// crate is wrapped, so we pick whichever subtree really has the file — i.e. where rustc and the
/// linker will look for it. Outside cross-compilation there's only ever the one subtree.
#[must_use]
pub(crate) fn deps_dir_for(file: &Utf8Path, target_path: &Utf8Path) -> Utf8PathBuf {
    let target_deps = target_path.join("deps");
    if !target_deps.join(file).exists()
        && let Some(host) = host_profile_dir(target_path)
    {
        let host_deps = host.join("deps");
        if host_deps.join(file).exists() {
            return host_deps;
        }
    }
    target_deps
}

#[test]
fn host_profile_dir_only_differs_when_cross_compiling() {
    // `cargo green install` sets $CARGO_TARGET_DIR with a trailing slash (see dirs.rs).
    temp_env::with_var("CARGO_TARGET_DIR", Some("/tmp/clis-marauder_master/"), || {
        // Cross: the target subtree carries the triple; fall back to the no-triple host dir.
        assert_eq!(
            host_profile_dir(
                "/tmp/clis-marauder_master/armv7-unknown-linux-musleabihf/release".into()
            ),
            Some("/tmp/clis-marauder_master/release".into())
        );
        // Native: `target_path` already is the host profile dir, so there's nothing to fall back to.
        assert_eq!(host_profile_dir("/tmp/clis-marauder_master/release".into()), None);
    });
}

#[test]
fn host_profile_dir_absent_without_cargo_target_dir() {
    temp_env::with_var("CARGO_TARGET_DIR", None::<&str>, || {
        assert_eq!(host_profile_dir("/tmp/whatever/release".into()), None);
    });
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
