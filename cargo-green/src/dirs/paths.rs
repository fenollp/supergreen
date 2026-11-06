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

    /// Memoized initial $PWD
    #[doc(hidden)]
    pub(crate) cwd: Utf8PathBuf,

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
        let path = self.rewrite_cwd_str(&path);
        path
    }

    #[expect(clippy::let_and_return)]
    #[must_use]
    pub(crate) fn rewrite_str(&self, txt: &str) -> String {
        let txt = self.rewrite_target_dir_str(txt);
        let txt = rewrite_cratesio_index(&txt);
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

#[cfg(test)]
mod props {
    use proptest::prelude::*;

    use super::Paths;

    /// Host roots, made to overlap: the project under home, the target dir under it or not,
    /// `$CARGO_HOME` under home or (as on CI) under the project.
    fn paths() -> impl Strategy<Value = Paths> {
        ("/[ab]{1,2}(/[ab]{1,2})?", "[ab]{1,3}", any::<bool>(), any::<bool>()).prop_map(
            |(home, proj, target_inside, cargo_inside)| {
                let cwd = format!("{home}/{proj}");
                Paths {
                    cargo_home: if cargo_inside {
                        format!("{cwd}/.cargo")
                    } else {
                        format!("{home}/.cargo")
                    }
                    .into(),
                    host_target_dir: Some(
                        if target_inside { format!("{cwd}/target") } else { format!("{home}/tgt") }
                            .into(),
                    ),
                    cwd: cwd.into(),
                    ..Default::default()
                }
            },
        )
    }

    /// A text of host paths, as found in args, env values or diagnostics: any root, any
    /// number of times, bare or not.
    fn text(paths: &Paths, sep: &'static str) -> impl Strategy<Value = String> + use<> {
        let roots = vec![
            paths.cwd.to_string(),
            paths.target_dir().to_string(),
            paths.cargo_home.to_string(),
        ];
        let path = (proptest::sample::select(roots), "(/[a-z0-9_.-]{1,4}){0,3}");
        proptest::collection::vec(path, 1..=5).prop_map(move |paths| {
            paths.into_iter().map(|(r, rel)| r + &rel).collect::<Vec<_>>().join(sep)
        })
    }

    /// How the container reads `$CARGO_HOME`.
    fn expand(txt: &str) -> String {
        txt.replace("$CARGO_HOME", "/usr/local/cargo")
    }

    fn case_sep() -> impl Strategy<Value = (Paths, String)> {
        paths().prop_flat_map(move |p| (Just(p.clone()), text(&p, " ")))
    }

    proptest! {
        /// Diagnostics coming back from the container name the host's paths again. Also in
        /// lists of paths (as in `$PATH`).
        #[test]
        fn host_paths_round_trip(sep in prop_oneof![Just(" "), Just(":"), Just(",")], (paths, txt) in case_sep()) {
            let txt = txt.replace(' ', sep);
            prop_assert_eq!(paths.un_rewrite_str(&expand(&paths.rewrite_str(&txt))), txt);
        }

        /// No host path is left in what the container gets. Also in lists of paths.
        #[test]
        fn no_host_path_reaches_the_container(sep in prop_oneof![Just(" "), Just(":"), Just(",")], (paths, txt) in case_sep()) {
            let rewritten = paths.rewrite_str(&txt.replace(' ', sep));
            for root in [paths.cwd.as_str(), paths.target_dir().as_str(), paths.cargo_home.as_str()] {
                let leaked = rewritten.split(sep).find(|p| *p == root || p.starts_with(&format!("{root}/")));
                prop_assert!(leaked.is_none(), "{root} in {rewritten}");
            }
        }
    }
}
