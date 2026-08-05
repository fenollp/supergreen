use std::{env, fs};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use pico_args::Arguments;

use crate::dirs::{Paths, hashed_args, pwd, replace_carefully, tmp};

const VIRTUAL_TARGET_DIR: &str = "/target/";

#[must_use]
pub(crate) fn is_named_same_as_virtual_target_dir(fname: &str) -> bool {
    fname == VIRTUAL_TARGET_DIR.trim_matches('/')
}

pub(crate) fn create_current_target_dir(is_install: bool) -> Result<Utf8PathBuf> {
    let target_dir = Arguments::from_env()
        .opt_value_from_str("--target-dir")
        .map_err(|e| anyhow!("Bad --target-dir argument: {e}"))?;
    let target_dir = if let Some(target_dir) = target_dir {
        target_dir
    } else if let Ok(target_dir) = env::var(CARGO_TARGET_DIR!()) {
        target_dir
    } else if false {
        todo!("check build.target-dir in config.toml.s")
    } else if is_install {
        tmp().join(hashed_args()).to_string()
    } else {
        pwd().join("target").to_string() // TODO: fallback to workspace root, not necessarily pwd()
    };

    fs::create_dir_all(&target_dir)
        .map_err(|e| anyhow!("Failed to `mkdir -p {target_dir}`: {e}"))?;

    Utf8PathBuf::from(&target_dir)
        .canonicalize_utf8()
        .map_err(|e| anyhow!("Failed to canonicalize target dir {target_dir}: {e}"))
}

impl Paths {
    pub(crate) fn target_dir(&self) -> &Utf8Path {
        self.host_target_dir.as_deref().expect("PROOF: set in main for wrap'd commands")
    }

    pub(crate) fn un_rewrite_target_dir_str(&self, txt: &str) -> String {
        let target_dir = format!("{}/", self.target_dir());
        replace_carefully(txt, VIRTUAL_TARGET_DIR, &target_dir)
    }

    pub(crate) fn rewrite_target_dir_str(&self, txt: &str) -> String {
        let target_dir = format!("{}/", self.target_dir());
        replace_carefully(txt, &target_dir, VIRTUAL_TARGET_DIR)
    }

    pub(crate) fn rewrite_target_dir(&self, path: &Utf8Path) -> Utf8PathBuf {
        path.strip_prefix(self.target_dir())
            .map(|path| Utf8Path::new(VIRTUAL_TARGET_DIR).join(path))
            .unwrap_or_else(|_| path.to_owned())
    }
}

#[test]
fn target_dir_var() {
    let paths = Paths { host_target_dir: Some("/some/path".into()), ..Default::default() };

    assert_eq!(paths.target_dir().as_str(), "/some/path");

    assert_eq!(paths.host_profile_dir("/some/path/release".into()), None);
    assert_eq!(
        paths.host_profile_dir("/some/path/armv7-unknown-linux-musleabihf/release".into()),
        Some("/some/path/release".into())
    );

    assert_eq!(
        paths
            .rewrite_target_dir("/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d".into()),
        "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d"
    );

    assert_eq!(
        paths.rewrite_target_dir_str(
            "/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
        ),
        "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
    );

    assert_eq!(
        paths.rewrite_target_dir_str(
            "/some/path/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
        ),
        "/target/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
    );

    assert_eq!(
        paths.un_rewrite_target_dir_str(
            "/target/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
        ),
        "/some/path/release/deps/target_lexicon-8a85e67f3430b2ca.d: /home/pete/.cargo/registry/src/index.crates.io-0000000000000000/target-lexicon-0.12.16/src/lib.rs"
    );

    assert_eq!(
        paths.un_rewrite_target_dir_str(
            "/target/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
        ),
        "/some/path/debug/deps/cc-63321ad70751c592.d: /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/lib.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target.rs /home/pete/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.2.47/src/target/apple.rs"
    );

    assert_eq!(
        paths.un_rewrite_target_dir_str(
            "error: couldn't read `/target/armv7-unknown-linux-musleabihf/release/build/pb-bd1e88e219ae6eda/out/hypercards.rs`: No such file or directory (os error 2)"
        ),
        "error: couldn't read `/some/path/armv7-unknown-linux-musleabihf/release/build/pb-bd1e88e219ae6eda/out/hypercards.rs`: No such file or directory (os error 2)"
    );
}
