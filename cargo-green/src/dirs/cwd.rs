use camino::{Utf8Path, Utf8PathBuf};

use crate::dirs::{Paths, replace_tokens};

const VIRTUAL_CWD: &str = "/work/";

impl Paths {
    pub(crate) fn un_rewrite_cwd_str(&self, txt: &str) -> String {
        let cwd = format!("{}/", self.cwd);
        let txt = replace_tokens(txt, VIRTUAL_CWD, &cwd, false);
        replace_tokens(&txt, VIRTUAL_CWD.trim_end_matches('/'), self.cwd.as_str(), true)
    }

    pub(crate) fn rewrite_cwd_str(&self, txt: &str) -> String {
        let txt = replace_tokens(txt, &format!("{}/", self.cwd), VIRTUAL_CWD, false);
        replace_tokens(&txt, self.cwd.as_str(), VIRTUAL_CWD.trim_end_matches('/'), true)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn rewrite_cwd(&self, path: &Utf8Path) -> Utf8PathBuf {
        match path.strip_prefix(&self.cwd) {
            Ok(rel) if rel.as_str().is_empty() => VIRTUAL_CWD.trim_end_matches('/').into(),
            Ok(rel) => Utf8Path::new(VIRTUAL_CWD).join(rel),
            Err(_) => path.to_owned(),
        }
    }
}

#[test]
fn virtual_pwd_pins_local_paths() {
    let cwd: Utf8PathBuf = "/some/path".into();
    let paths = Paths { cwd: cwd.clone(), ..Default::default() };

    assert_eq!(paths.rewrite_cwd(&cwd.join("src/lib.rs")), "/work/src/lib.rs");
    assert_eq!(paths.un_rewrite_cwd_str("/work/src/lib.rs"), cwd.join("src/lib.rs").as_str());

    // Bare $PWD, no trailing path segment
    assert_eq!(paths.rewrite_cwd(&cwd), "/work");
    assert_eq!(paths.rewrite_cwd_str(&format!("WORKDIR {cwd}")), "WORKDIR /work");
    assert_eq!(paths.un_rewrite_cwd_str("WORKDIR /work"), format!("WORKDIR {cwd}"));
    assert_eq!(
        paths.rewrite_cwd_str(&format!("CARGO_MANIFEST_DIR={cwd}")),
        "CARGO_MANIFEST_DIR=/work"
    );
    assert_eq!(
        paths.un_rewrite_cwd_str("CARGO_MANIFEST_DIR=/work"),
        format!("CARGO_MANIFEST_DIR={cwd}")
    );

    assert_eq!(
        paths.rewrite_cwd_str(&format!("CARGO_MANIFEST_DIR={cwd}/thing")),
        "CARGO_MANIFEST_DIR=/work/thing"
    );
    assert_eq!(
        paths.un_rewrite_cwd_str("CARGO_MANIFEST_DIR=/work/thing"),
        format!("CARGO_MANIFEST_DIR={cwd}/thing")
    );

    // $PWD as prefix of a longer name must not match
    assert_eq!(paths.rewrite_cwd_str(&format!("{cwd}.bak/x")), format!("{cwd}.bak/x"));
    assert_eq!(paths.rewrite_cwd_str(&format!("{cwd}2/x")), format!("{cwd}2/x"));
    assert_eq!(paths.un_rewrite_cwd_str("/workspace/x"), "/workspace/x");

    let unrelated = "$CARGO_HOME/registry/src/index.crates.io/anyhow-1.0.100/src/lib.rs";
    assert_eq!(paths.rewrite_cwd_str(unrelated), unrelated);
    assert_eq!(paths.un_rewrite_cwd_str(unrelated), unrelated);
}
