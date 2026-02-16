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
    txt.replace(VIRTUAL_TARGET_DIR, TARGET_DIR.as_str())
}

pub(crate) fn virtual_target_dir_str(txt: &str) -> String {
    if !REWRITE_TARGETDIR {
        return txt.to_owned();
    }
    txt.replace(TARGET_DIR.as_str(), VIRTUAL_TARGET_DIR)
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
        assert_eq!(
            virtual_target_dir("/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d".into()),
            "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d"
        );

        assert_eq!(
            virtual_target_dir_str("/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d"),
            "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d"
        );

        assert_eq!(un_virtual_target_dir_str("/target/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"),
        "/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs");
    });
}
