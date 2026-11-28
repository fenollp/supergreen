use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use cargo_lock::{Lockfile, Package, SourceId, Version};
use cargo_metadata::MetadataCommand;
use pico_args::Arguments;

// TODO: when cargo installing or building without a lockfile
// we can wrap the version picking process to favor cache-hot versions
// Then this'll help: https://github.com/pubgrub-rs/pubgrub

pub(crate) fn locked_crates(
    manifest_path_lockfile: &Utf8Path,
) -> Result<Vec<(String, String, String)>> {
    let lockfile = Lockfile::load(manifest_path_lockfile)?;

    let packages = lockfile
        .packages
        .into_iter()
        .filter(|pkg| pkg.source.as_ref().is_none_or(SourceId::is_default_registry))
        .filter(|pkg| pkg.checksum.is_some())
        .map(|Package { name, version, checksum, .. }| {
            (name.to_string(), version.to_string(), checksum.unwrap().to_string())
        })
        .collect::<Vec<_>>();

    Ok(packages)
}

/// URL and commit of each git dependency the builder can fetch on its own: it has no SSH keys.
pub(crate) fn locked_gits(manifest_path_lockfile: &Utf8Path) -> Result<Vec<(String, String)>> {
    Ok(gits(Lockfile::load(manifest_path_lockfile)?))
}

fn gits(lockfile: Lockfile) -> Vec<(String, String)> {
    let gits: BTreeSet<_> = lockfile
        .packages
        .into_iter()
        .filter_map(|pkg| pkg.source)
        .filter(SourceId::is_git)
        .filter(|source| matches!(source.url().scheme(), "https" | "http" | "git"))
        .filter_map(|source| Some((source.url().to_string(), source.precise()?.to_owned())))
        .collect();
    gits.into_iter().collect()
}

/// A `cargo update` may (un)bump unrelated crates, in a flaky way.
///
/// Open issues:
/// * [cargo update -p SPEC can silently downgrade dependencies of other packages when multi-major ranges are used](https://github.com/rust-lang/cargo/issues/5529)
/// * [unrelated dependency changed in Cargo.lock when updating another dependency](https://github.com/rust-lang/cargo/issues/14115)
/// * [Cargo downgrades transitive dependency that should not change](https://github.com/rust-lang/cargo/issues/14446)
/// * [Unstable lockfile between cargo update runs](https://github.com/rust-lang/cargo/issues/15513)
const FLAKY: &[&str] = &[
    // Keep these sorted
    "windows-sys",
];

/// Whether some package, in both lockfiles, now depends on other versions of [`FLAKY`] crates.
#[must_use]
pub(crate) fn changes_flaky_transitives(before: &Lockfile, after: &Lockfile) -> bool {
    type Deps<'a> = BTreeSet<(&'a str, &'a Version)>;

    fn by_dependent(lockfile: &Lockfile) -> BTreeMap<(&str, &Version), Deps<'_>> {
        lockfile
            .packages
            .iter()
            .map(|pkg| {
                let flaky = pkg
                    .dependencies
                    .iter()
                    .map(|dep| (dep.name.as_str(), &dep.version))
                    .filter(|(name, _)| FLAKY.contains(name))
                    .collect();
                ((pkg.name.as_str(), &pkg.version), flaky)
            })
            .collect()
    }

    let after = by_dependent(after);
    by_dependent(before)
        .into_iter()
        .any(|(dependent, was)| after.get(&dependent).is_some_and(|now| *now != was))
}

#[test]
fn flaky_deps_changes() {
    let lockfile = |rustix: &[&str], tempfile: &[&str]| {
        let pkg = |name, version, deps| {
            format!(
                "[[package]]\nname = {name:?}\nversion = {version:?}\ndependencies = {deps:?}\n"
            )
        };
        let toml = format!(
            r#"version = 4
{rustix}
{tempfile}
{libc1}
{libc2}
{flaky1}
{flaky2}
"#,
            rustix = pkg("rustix", "1.0.0", rustix),
            tempfile = pkg("tempfile", "3.0.0", tempfile),
            libc1 = pkg("libc", "0.2.1", &[]),
            libc2 = pkg("libc", "0.2.2", &[]),
            flaky1 = pkg("windows-sys", "0.52.0", &[]),
            flaky2 = pkg("windows-sys", "0.61.2", &[]),
        );
        toml.parse::<Lockfile>().unwrap()
    };
    let (old, new) = (&["windows-sys 0.52.0"], &["windows-sys 0.61.2"]);
    let before = lockfile(new, old);

    assert!(!changes_flaky_transitives(&before, &before));
    assert!(changes_flaky_transitives(&before, &lockfile(old, old)), "a downgrade");
    assert!(changes_flaky_transitives(&before, &lockfile(new, new)), "an upgrade");
    assert!(changes_flaky_transitives(&before, &lockfile(new, &[])), "a removal");

    let before = lockfile(&["libc 0.2.1"], &[]);
    assert!(!changes_flaky_transitives(&before, &lockfile(&["libc 0.2.2"], &[])), "not flaky");
}

/// Unlike [`find_lockfile`], doesn't take `--package` for a path:
/// here, PKGs in `cargo update [PKG]...` are crate specs.
#[must_use]
pub(crate) fn load_workspace_lockfile() -> Option<Lockfile> {
    let manifest_path = manifest_path_arg(&mut Arguments::from_env()).ok()?;
    let manifest_path = workspace_manifest_path(manifest_path).ok()?;
    let lockfile_path = manifest_path.with_extension("lock");
    Lockfile::load(lockfile_path).ok()
}

pub(crate) fn find_lockfile(pwd: &Utf8Path) -> Result<Utf8PathBuf> {
    let manifest_path = find_manifest_path(pwd)?;
    let candidate = manifest_path.with_extension("lock");
    if candidate.exists() {
        return Ok(candidate);
    }

    // Fallback on workspace lockfile
    let manifest_path = workspace_manifest_path(None)?;
    Ok(manifest_path.with_extension("lock"))
}

fn package_arg(args: &mut Arguments) -> Result<Option<String>> {
    args.opt_value_from_str(["-p", "--package"])
        .map_err(|e| anyhow!("Failed parsing cargo args: {e}"))
}

fn manifest_path_arg(args: &mut Arguments) -> Result<Option<String>> {
    args.opt_value_from_str("--manifest-path")
        .map_err(|e| anyhow!("Failed parsing cargo args: {e}"))
}

/// FIXME: when cargo install-ing, root crate's code isn't local (yet)
/// Meaning accessing its TOML metadata nor its locked deps is possible.
/// cc <https://github.com/rust-lang/cargo/issues/9700>
pub(crate) fn find_manifest_path(pwd: &Utf8Path) -> Result<Utf8PathBuf> {
    let mut args = Arguments::from_env();
    if let Some(manifest_path) = manifest_path_arg(&mut args)? {
        return Ok(manifest_path.into());
    }
    // This is probably not correct
    if let Some(package) = package_arg(&mut args)? {
        let manifest_path = pwd.join(package).join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(manifest_path);
        }
    }

    workspace_manifest_path(None)
}

fn workspace_manifest_path(manifest_path: Option<String>) -> Result<Utf8PathBuf> {
    let mut metadata = MetadataCommand::new();
    metadata.no_deps();
    if let Some(manifest_path) = manifest_path {
        metadata.manifest_path(manifest_path);
    }
    let metadata = metadata.exec().map_err(|e| anyhow!("Failed running cargo metadata: {e}"))?;
    Ok(metadata.workspace_root.join("Cargo.toml"))
}

#[test]
fn the_workspace_manifest_not_a_member_s() -> Result<()> {
    // Tests run from the member's dir, where there's no lockfile
    let manifest_path = workspace_manifest_path(None)?;
    assert!(manifest_path.with_extension("lock").exists(), "{manifest_path}");
    Ok(())
}

#[cfg(test)]
mod git_going {
    use cargo_lock::Lockfile;

    use super::gits;

    #[test]
    fn each_fetchable_repo_and_commit_once() {
        let package = |name: &str, source: &str| {
            format!(
                "[[package]]\nname = \"{name}\"\nversion = \"1.0.0\"\nsource = \"{source}\"\n\n"
            )
        };
        let lockfile = [
            package(
                "a",
                "git+https://github.com/o/repo?branch=dev#1111111111111111111111111111111111111111",
            ),
            package(
                "b",
                "git+https://github.com/o/repo?branch=dev#1111111111111111111111111111111111111111",
            ),
            package(
                "c",
                "git+https://github.com/o/repo.git?tag=v1#2222222222222222222222222222222222222222",
            ),
            package(
                "d",
                "git+ssh://git@github.com/o/private#3333333333333333333333333333333333333333",
            ),
            package("e", "registry+https://github.com/rust-lang/crates.io-index"),
        ]
        .concat();
        assert_eq!(
            gits(lockfile.parse::<Lockfile>().unwrap()),
            [
                ("https://github.com/o/repo".to_owned(), "1".repeat(40)),
                ("https://github.com/o/repo.git".to_owned(), "2".repeat(40)),
            ]
        );
    }
}
