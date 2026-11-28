//! Which packages `cargo install` compiles: it keeps no lockfile around to read them from.
//!
//! Wrappers note them down as empty files named `<name>@<version>`. These live in the target dir:
//! for as long as the artifacts cargo then finds fresh, and calls no wrapper about.

use std::{collections::BTreeSet, io::ErrorKind};

use anyhow::{Result, anyhow};
use camino::Utf8Path;
use semver::Version;

use crate::sys::fs;

pub(crate) fn note(dir: &Utf8Path, name: &str, version: &str) -> Result<()> {
    let path = dir.join(format!("{name}@{version}"));
    fs().create_dir_all(dir)
        .and_then(|()| fs().write(&path, ""))
        .map_err(|e| anyhow!("Failed to write {path}: {e}"))
}

/// What was [`note`]d.
pub(crate) fn noted(dir: &Utf8Path) -> Result<BTreeSet<(String, Version)>> {
    let entries = match fs().read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => vec![],
        Err(e) => return Err(anyhow!("Failed to read {dir}: {e}")),
    };
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let (name, version) = entry.file_name()?.split_once('@')?;
            Some((name.to_owned(), Version::parse(version).ok()?))
        })
        .collect())
}

#[test]
fn notes_what_gets_built() {
    use crate::sys::Sys;

    let _guard = Sys::install(Sys::fake());
    let dir = Utf8Path::new("/target/pkgs");
    assert_eq!(noted(dir).unwrap(), BTreeSet::new());

    note(dir, "a", "1.0.0").unwrap();
    note(dir, "a", "1.0.0").unwrap();
    note(dir, "b", "0.1.0-rc.1+meta").unwrap();
    let version = |v| Version::parse(v).unwrap();
    assert_eq!(
        noted(dir).unwrap(),
        BTreeSet::from([
            ("a".to_owned(), version("1.0.0")),
            ("b".to_owned(), version("0.1.0-rc.1+meta")),
        ])
    );
}
