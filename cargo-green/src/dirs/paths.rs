use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{
    base_image::rewrite_rustup_home, cratesio::rewrite_cratesio_index, dirs::Dirs, wrap::safeify,
};

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Paths {
    /// Memoized $CARGO_HOME
    #[doc(hidden)]
    pub(crate) cargo_home: Utf8PathBuf,

    /// Memoized $CARGO_TARGET_DIR
    #[doc(hidden)]
    pub(crate) host_target_dir: Option<Utf8PathBuf>,

    /// XDG base directories
    #[doc(hidden)]
    pub(crate) dirs: Option<Dirs>,
}

impl Paths {
    pub(crate) fn rewrite_env(&self, val: &str) -> Result<String> {
        let val = safeify(val)?;
        let val = self.rewrite_str(&val);
        // TODO: in rustc's args: replace last WORKDIR with $PWD (--out-dir ..., OUT_DIR=..., maybe others)
        Ok(val)
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn rewrite(&self, path: &Utf8Path) -> String {
        let path = self.rewrite_target_dir(path);
        let path = rewrite_cratesio_index(path.as_str());
        let path = rewrite_rustup_home(&path);
        let path = self.rewrite_cargo_home(&path);
        path
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn rewrite_str(&self, txt: &str) -> String {
        let txt = self.rewrite_target_dir_str(txt);
        let txt = rewrite_cratesio_index(&txt);
        let txt = rewrite_rustup_home(&txt);
        let txt = self.rewrite_cargo_home(&txt);
        txt
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn un_rewrite_str(&self, txt: &str) -> String {
        let txt = self.un_rewrite_target_dir_str(txt);
        let txt = self.un_rewrite_cargo_home(&txt);
        txt
    }
}

#[test]
fn test_rewrite_env() {
    let paths = Paths {
        cargo_home: "/some/other/path".into(),
        host_target_dir: Some("/some/path".into()),
        ..Default::default()
    };

    assert_eq!(
        "https://github.com/dtolnay/anyhow",
        paths.rewrite_env("https://github.com/dtolnay/anyhow").unwrap()
    );

    assert_eq!(
        "$CARGO_HOME/registry/src/index.crates.io+zstd.1.5.7/zstd/lib",
        paths
            .rewrite_env("/some/other/path/registry/src/index.crates.io+zstd.1.5.7/zstd/lib")
            .unwrap()
    );

    assert_eq!(
        "/target/release/build/zstd-safe-f387b30b22c9cb23/out",
        paths.rewrite_env("/some/path/release/build/zstd-safe-f387b30b22c9cb23/out").unwrap()
    );
}
