use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{base_image::rewrite_rustup_home, dirs::Dirs, wrap::safeify};

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Paths {
    /// Memoized $CARGO_HOME
    #[doc(hidden)]
    pub(crate) cargo_home: Utf8PathBuf,

    /// Memoized initial $PWD
    #[doc(hidden)]
    pub(crate) cwd: Utf8PathBuf,

    /// Memoized $CARGO_TARGET_DIR
    #[doc(hidden)]
    pub(crate) host_target_dir: Option<Utf8PathBuf>,

    /// XDG base directories
    #[doc(hidden)]
    pub(crate) dirs: Option<Dirs>,

    /// Where the plugin chitchats with its wrappers (see [`crate::chitchat`])
    #[doc(hidden)]
    pub(crate) chitchat_sock: Option<String>,
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
        let path = rewrite_rustup_home(path.as_str());
        let path = self.rewrite_cargo_home(&path);
        let path = self.rewrite_cwd_str(&path);
        path
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn rewrite_str(&self, txt: &str) -> String {
        let txt = self.rewrite_target_dir_str(txt);
        let txt = rewrite_rustup_home(&txt);
        let txt = self.rewrite_cargo_home(&txt);
        let txt = self.rewrite_cwd_str(&txt);
        txt
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn un_rewrite_str(&self, txt: &str) -> String {
        let txt = self.un_rewrite_target_dir_str(txt);
        let txt = self.un_rewrite_cargo_home(&txt);
        let txt = self.un_rewrite_cwd_str(&txt);
        txt
    }
}

#[test]
fn registry_dirs_names_are_kept() {
    let paths = Paths {
        cargo_home: "/h/.cargo".into(),
        cwd: "/an/other/path".into(),
        host_target_dir: Some("/some/path".into()),
        ..Default::default()
    };

    for index in ["index.crates.io-1949cf8c6b5b557f", "index.crates.io-6f17d22bba15001f"] {
        let host = format!("/h/.cargo/registry/src/{index}/anyhow-1.0.100/src/lib.rs");
        let guest = format!("$CARGO_HOME/registry/src/{index}/anyhow-1.0.100/src/lib.rs");
        assert_eq!(paths.rewrite(host.as_str().into()), guest);
        assert_eq!(paths.rewrite_str(&host), guest);
        let in_build = format!("/usr/local/cargo/registry/src/{index}/anyhow-1.0.100/src/lib.rs");
        assert_eq!(paths.un_rewrite_str(&in_build), host);
    }
}

#[test]
fn test_rewrite_env() {
    let paths = Paths {
        cargo_home: "/some/other/path".into(),
        cwd: "/an/other/path".into(),
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
