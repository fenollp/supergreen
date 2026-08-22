//! Local (relative to `$PWD`) code gets inlined in the Containerfile itself,
//! rather than handed to the runner as a local build context.
//!
//! Build contexts are keyed on a machine-local path hint, so a stage that reads
//! one can't be shared through a registry cache. Inlining the code means the
//! Containerfile alone decides the stage's contents: the stage is then named
//! after a hash of these contents and caching works the same as for the crates
//! we `ADD` from crates.io.
//!
//! Files travel as `COPY` heredocs, grouped by destination directory and mode.
//! BuildKit's heredocs are byte-transparent (CRLF, NUL bytes and trailing
//! newlines all survive) but a few files can't be written that way:
//! see [`is_inlineable`]. Those travel in a deterministic tarball, base64'd into
//! one more heredoc, that stages mounting this one extract in a preliminary RUN
//! (see [`AsStage::prelude`]).

use std::{collections::BTreeMap, env, fs, os::unix::fs::PermissionsExt, process::Stdio};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio_tar::{EntryType, Header};

use crate::{
    build::SOURCE_DATE_EPOCH,
    cache::result::assert_tarball_header,
    dirs::{Paths, is_named_same_as_virtual_target_dir},
    stage::{AsBlock, AsStage, NamedStage, Stage},
};

/// The Containerfile parser rejects lines longer than 64KiB:
/// ```
/// failed to solve: unterminated heredoc
/// ```
const LINE_MAX: usize = 60_000;

/// Width of the base64 lines carrying the tarball.
const B64_WIDTH: usize = 76;

/// Prefix for the heredoc delimiters we pick ourselves.
const EOF: &str = "CARGOGREEN_EOF";

/// The runner receives a Containerfile as a single gRPC message, and gRPC caps
/// those at 16MiB. Framing roughly doubles what we write, so this is about as
/// much code as one can inline:
/// ```
/// ResourceExhausted: trying to send message larger than max (17920354 vs. 16777216)
/// ```
const WEIGHT_MAX: usize = 7 * 1024 * 1024;

/// Which of `$PWD` a crate gets to see.
///
/// Inlining is capped (see [`WEIGHT_MAX`]) so we can't just hand over all of
/// `$PWD` the way a build context did: a workspace's other members, its CI
/// files and its docs are none of this crate's business. What's left is the
/// crate's own directory, plus `$PWD`'s own files — manifests, lockfile, and
/// the `README.md` that `#![doc = include_str!("../README.md")]` reaches for.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Picks {
    /// The package's directory, relative to `$PWD` (empty when they're one and the same)
    pkg: Utf8PathBuf,

    /// Directories to take whole, relative to `$PWD`, for crate roots living outside the package
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    also: Vec<Utf8PathBuf>,

    /// What `cargo package --list` counts as the package's, relative to `$PWD`.
    /// Sorted. Empty when cargo wouldn't say, which takes `pkg` whole.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    listed: Vec<Utf8PathBuf>,
}

impl Picks {
    /// Whether walking into `rel` could turn anything up
    #[must_use]
    fn descend(&self, rel: &Utf8Path) -> bool {
        let towards = |dir: &Utf8Path| rel.starts_with(dir) || dir.starts_with(rel);
        towards(&self.pkg) || self.also.iter().any(|dir| towards(dir))
    }

    /// Whether the file at `rel` goes in
    #[must_use]
    fn keep(&self, rel: &Utf8Path) -> bool {
        if rel.parent().map(|parent| parent.as_str().is_empty()).unwrap_or(true) {
            return true; // $PWD's own files
        }
        if self.also.iter().any(|dir| rel.starts_with(dir)) {
            return true;
        }
        if !rel.starts_with(&self.pkg) {
            return false;
        }
        self.listed.is_empty() || self.listed.binary_search_by(|had| (**had).cmp(rel)).is_ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Relative {
    stage: Stage,

    /// Host directory holding the code
    pwd: Utf8PathBuf,

    /// Which of `pwd` this stage holds
    picks: Picks,

    /// Where `pwd` shows up within containers (e.g. `/work`)
    dst: Utf8PathBuf,

    /// Whether some of `pwd` travels through the embedded tarball
    blobs: bool,

    /// Paths under `pwd` to mount from this stage: the shallowest ones that
    /// don't hide a file the tarball writes.
    roots: Vec<Utf8PathBuf>,
}

impl AsBlock for Relative {
    fn as_block(&self) -> Result<Option<String>> {
        let Self { stage, pwd, picks, .. } = self;

        // Re-read the code: contents are too big (and not always UTF-8) to be kept in an Md.
        let scan = Scan::of(pwd, picks)?;

        // Reading a different tree than the one we named this stage after would
        // publish someone else's code under our hash: cached forever, everywhere.
        let hash = scan.hash();
        if *stage != Stage::local(&hash)? {
            bail!("Code under {pwd} changed while building: {stage} is now cwd-{hash}")
        }

        Ok(Some(scan.block(stage)))
    }
}

impl AsStage<'_> for Relative {
    fn name(&self) -> &Stage {
        &self.stage
    }

    fn mounts(&self) -> Vec<(Option<Utf8PathBuf>, Utf8PathBuf, bool)> {
        let Self { pwd, roots, .. } = self;
        roots.iter().map(|root| (Some(format!("/{root}").into()), pwd.join(root), true)).collect()
    }

    fn prelude(&self) -> Option<String> {
        let Self { stage, dst, blobs, .. } = self;
        if !*blobs {
            return None;
        }
        // NOTE: both `base64` and `tar` are given by coreutils as well as busybox.
        let tarball = tarball(stage);
        Some(format!(
            r#"
RUN \
  --mount=from={stage},source=/{tarball},dst=/tmp/{tarball} \
    mkdir -p {dst} \
 && base64 -d /tmp/{tarball} | tar -xf - -C {dst}
"#
        ))
    }
}

pub(crate) async fn as_stage(
    paths: &Paths,
    pwd: &Utf8Path,
    pkg_manifest_dir: &Utf8Path,
    input: &Utf8Path,
) -> Result<NamedStage> {
    let picks = pick(pwd, pkg_manifest_dir, input).await;
    info!("inlining {pwd} through {picks:?}");

    let scan = Scan::of(pwd, &picks)?;

    // Better to say which code doesn't fit than to have the runner drop the connection.
    let weight = scan.weight();
    if weight > WEIGHT_MAX {
        bail!(
            r#"
    Can't inline the {weight}B of code under {pwd} ({WEIGHT_MAX}B at most).
    Heaviest files:
{heaviest}
    Trim the package down (Cargo.toml's `exclude`, or .gitignore) and run your command again.
"#,
            heaviest = scan.heaviest(5)
        )
    }

    let stage = Stage::local(&scan.hash())?;
    info!(
        "{stage}: {weight}B over {} inlined file(s), {} tarball'd, {} mount(s)",
        scan.texts.len(),
        scan.blobs.len(),
        scan.roots.len()
    );

    Ok(NamedStage::Relative(Relative {
        stage,
        pwd: pwd.to_owned(),
        picks,
        dst: paths.rewrite(pwd).into(),
        blobs: !scan.blobs.is_empty(),
        roots: scan.roots,
    }))
}

async fn pick(pwd: &Utf8Path, pkg_manifest_dir: &Utf8Path, input: &Utf8Path) -> Picks {
    let Ok(pkg) = pkg_manifest_dir.strip_prefix(pwd).map(ToOwned::to_owned) else {
        // Nothing says the package is under $PWD, so fall back to taking all of it.
        warn!("{pkg_manifest_dir} lies outside {pwd}: inlining all of it");
        return Picks::default();
    };

    // `--crate-name`'s file usually sits in the package, but a [[bin]] path may point elsewhere.
    let mut also = vec![];
    if let Some(root) = input.parent().filter(|root| !root.starts_with(&pkg)) {
        also.push(root.to_owned());
    }

    let mut listed: Vec<_> = cargo_lists(pkg_manifest_dir)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|file| pkg.join(file))
        // `cargo package` makes up some of what it lists (Cargo.lock, Cargo.toml.orig,
        // .cargo_vcs_info.json, and a README.md read from outside the package).
        .filter(|rel| fs::symlink_metadata(pwd.join(rel)).is_ok())
        .collect();
    listed.sort();
    listed.dedup();

    Picks { pkg, also, listed }
}

/// Asks cargo which files it counts as the package's: this honours .gitignore
/// along with Cargo.toml's `include` and `exclude`.
///
/// Best-effort: whatever goes wrong, the whole package directory goes in.
async fn cargo_lists(pkg_manifest_dir: &Utf8Path) -> Option<Vec<String>> {
    let cargo = env::var(CARGO!()).unwrap_or_else(|_| "cargo".to_owned());
    let manifest_path = pkg_manifest_dir.join("Cargo.toml");

    let mut cmd = Command::new(&cargo);
    cmd.kill_on_drop(true)
        // We are cargo's $RUSTC_WRAPPER: don't let it call back into us.
        .env_remove(RUSTC_WRAPPER!())
        .env_remove(CARGOGREEN_PLUGINSETTINGS!())
        .env_remove(CARGOGREEN_EXECUTEBUILDSCRIPT!())
        .args(["package", "--list", "--frozen", "--allow-dirty"])
        .arg(format!("--manifest-path={manifest_path}"))
        .stdin(Stdio::null());

    let call = format!("{cargo} package --list --manifest-path={manifest_path}");
    debug!("Calling {call}");

    let output = match cmd.output().await {
        Ok(output) => output,
        Err(e) => {
            warn!("Failed to spawn `{call}`: {e}");
            return None;
        }
    };
    if !output.status.success() {
        warn!("`{call}` failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        return None;
    }

    let listing = match String::from_utf8(output.stdout) {
        Ok(listing) => listing,
        Err(e) => {
            warn!("`{call}` did not answer in utf-8: {e}");
            return None;
        }
    };
    Some(listing.lines().map(ToOwned::to_owned).filter(|file| !file.is_empty()).collect())
}

#[must_use]
fn tarball(stage: &Stage) -> String {
    format!("{stage}.tar.b64")
}

/// A file that goes in as a `COPY` heredoc
#[derive(Debug)]
struct Text {
    path: Utf8PathBuf,
    exec: bool,
    data: String,
}

/// Something a `COPY` heredoc can't spell out
#[derive(Debug)]
enum Blob {
    File {
        path: Utf8PathBuf,
        exec: bool,
        data: Vec<u8>,
    },
    Symlink {
        path: Utf8PathBuf,
        target: String,
    },
    /// A directory no file lands in: heredocs only ever create parents.
    Dir {
        path: Utf8PathBuf,
    },
}

impl Blob {
    #[must_use]
    fn path(&self) -> &Utf8Path {
        match self {
            Self::File { path, .. } | Self::Symlink { path, .. } | Self::Dir { path } => path,
        }
    }
}

#[derive(Debug, Default)]
struct Scan {
    texts: Vec<Text>,
    blobs: Vec<Blob>,
    roots: Vec<Utf8PathBuf>,
}

impl Scan {
    fn of(pwd: &Utf8Path, picks: &Picks) -> Result<Self> {
        let mut scan = Self::default();
        let (_, roots) = scan.walk(pwd, picks, Utf8Path::new(""), true)?;
        scan.roots = roots;
        Ok(scan)
    }

    /// Walks `pwd/rel`, returning whether it holds tarball'd entries along with
    /// the paths to mount for it. A directory that holds none is mounted whole
    /// by our caller, which drops the paths we return here.
    fn walk(
        &mut self,
        pwd: &Utf8Path,
        picks: &Picks,
        rel: &Utf8Path,
        top: bool,
    ) -> Result<(bool, Vec<Utf8PathBuf>)> {
        let dir = if rel.as_str().is_empty() { pwd.to_owned() } else { pwd.join(rel) };

        let mut fnames = dir
            .read_dir_utf8()
            .map_err(|e| anyhow!("Failed reading dir {dir}: {e}"))?
            .map(|entry| {
                let entry = entry.map_err(|e| anyhow!("Failed reading an entry of {dir}: {e}"))?;
                let Some(fname) = entry.path().file_name() else {
                    bail!("unexpected root (/) for {entry:?}")
                };
                Ok(fname.to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        fnames.sort(); // deterministic iteration

        let empty = fnames.is_empty();
        let mut tarballed = false;
        let mut roots = vec![];

        for fname in fnames {
            if excluded(&dir, &fname, top) {
                continue;
            }

            let path = dir.join(&fname);
            let child = rel.join(&fname);

            let md =
                fs::symlink_metadata(&path).map_err(|e| anyhow!("Failed to `stat {path}`: {e}"))?;

            if md.is_dir() {
                if !picks.descend(&child) {
                    continue;
                }
                let (child_tarballed, child_roots) = self.walk(pwd, picks, &child, false)?;
                if child_tarballed {
                    tarballed = true;
                    roots.extend(child_roots);
                } else if !child_roots.is_empty() {
                    roots.push(child);
                }
                // Otherwise nothing of `child` made it in, so there's nothing to mount for it.
                continue;
            }

            if !picks.keep(&child) {
                continue;
            }

            if md.is_symlink() {
                let target = fs::read_link(&path)
                    .map_err(|e| anyhow!("Failed to `readlink {path}`: {e}"))?;
                let Some(target) = target.to_str().map(ToOwned::to_owned) else {
                    bail!("Symlink {path} does not point to a utf-8 path")
                };
                self.blobs.push(Blob::Symlink { path: child, target });
                tarballed = true;
            } else {
                let data =
                    fs::read(&path).map_err(|e| anyhow!("Failed reading (RO) {path}: {e}"))?;
                let exec = md.permissions().mode() & 0o111 != 0;
                if is_inlineable(&data, &child) {
                    let data = String::from_utf8(data).expect("PROOF: is_inlineable checked");
                    self.texts.push(Text { path: child.clone(), exec, data });
                    roots.push(child);
                } else {
                    debug!("tarballing {child}");
                    self.blobs.push(Blob::File { path: child, exec, data });
                    tarballed = true;
                }
            }
        }

        if empty && !rel.as_str().is_empty() {
            // An empty dir has to come from the tarball, so it must not be shadowed by a mount.
            self.blobs.push(Blob::Dir { path: rel.to_owned() });
            return Ok((true, vec![]));
        }

        Ok((tarballed, roots))
    }

    /// How much of a Containerfile this scan takes up, near enough
    #[must_use]
    fn weight(&self) -> usize {
        let texts: usize = self.texts.iter().map(|text| text.data.len()).sum();
        let blobs: usize = self
            .blobs
            .iter()
            .map(|blob| match blob {
                Blob::File { data, .. } => BLOCK + data.len().div_ceil(3) * 4,
                Blob::Symlink { .. } | Blob::Dir { .. } => BLOCK,
            })
            .sum();
        texts + blobs
    }

    #[must_use]
    fn heaviest(&self, n: usize) -> String {
        let mut weighed: Vec<(usize, &Utf8Path)> = self
            .texts
            .iter()
            .map(|text| (text.data.len(), text.path.as_path()))
            .chain(self.blobs.iter().map(|blob| {
                let weight = match blob {
                    Blob::File { data, .. } => data.len(),
                    Blob::Symlink { .. } | Blob::Dir { .. } => 0,
                };
                (weight, blob.path())
            }))
            .collect();
        weighed.sort_by(|a, b| b.cmp(a));
        weighed
            .into_iter()
            .take(n)
            .map(|(weight, path)| format!("      {weight:>12}B {path}\n"))
            .collect()
    }

    /// Names the stage after everything it holds: same code, same stage, same cache.
    #[must_use]
    fn hash(&self) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(self.texts.len() + self.blobs.len());
        for Text { path, exec, data } in &self.texts {
            lines.push(format!(
                "{path}\tf{}\t{}\n",
                u8::from(*exec),
                sha256::digest(data.as_str())
            ));
        }
        for blob in &self.blobs {
            let path = blob.path();
            lines.push(match blob {
                Blob::File { exec, data, .. } => {
                    format!("{path}\tf{}\t{}\n", u8::from(*exec), sha256::digest(data.as_slice()))
                }
                Blob::Symlink { target, .. } => format!("{path}\tl\t{target}\n"),
                Blob::Dir { .. } => format!("{path}\td\t\n"),
            });
        }
        lines.sort();
        sha256::digest(lines.concat())[..16].to_owned()
    }

    #[must_use]
    fn block(&self, stage: &Stage) -> String {
        let mut block = format!("FROM scratch AS {stage}\n");

        // Files whose contents hold a line equal to their own name would end
        // their heredoc early: those get a delimiter of our own instead.
        let mut groups: BTreeMap<(&str, bool), Vec<&Text>> = BTreeMap::new();
        let mut singles: Vec<&Text> = vec![];
        for text in &self.texts {
            if text.data.lines().any(|line| line == fname(&text.path)) {
                singles.push(text);
            } else {
                let dir = text.path.parent().map(Utf8Path::as_str).unwrap_or_default();
                groups.entry((dir, text.exec)).or_default().push(text);
            }
        }

        for ((dir, exec), texts) in &groups {
            let sep = if dir.is_empty() { "" } else { "/" };
            let delims: String =
                texts.iter().map(|text| format!("<<\"{}\" ", fname(&text.path))).collect();
            block.push_str(&format!("COPY {}{delims}/{dir}{sep}\n", chmod(*exec)));
            for text in texts {
                block.push_str(&text.data);
                block.push_str(fname(&text.path));
                block.push('\n');
            }
        }

        for Text { path, exec, data } in singles {
            let delim = delimiting(data);
            block.push_str(&format!("COPY {}<<\"{delim}\" /{path}\n", chmod(*exec)));
            block.push_str(data);
            block.push_str(&delim);
            block.push('\n');
        }

        if !self.blobs.is_empty() {
            let b64 = base64(&self.tarball(), B64_WIDTH);
            let delim = delimiting(&b64);
            block.push_str(&format!("COPY <<\"{delim}\" /{}\n", tarball(stage)));
            block.push_str(&b64);
            block.push_str(&delim);
            block.push('\n');
        }

        block
    }

    /// A tarball that only depends on the files it holds: no mtimes, no ownership.
    #[must_use]
    fn tarball(&self) -> Vec<u8> {
        let mut blobs: Vec<&Blob> = self.blobs.iter().collect();
        blobs.sort_by(|a, b| a.path().cmp(b.path()));

        let mut tar = vec![];
        for blob in blobs {
            let path = blob.path().as_str();
            match blob {
                Blob::File { exec, data, .. } => {
                    entry(&mut tar, path, EntryType::Regular, mode(*exec), None, data);
                }
                Blob::Symlink { target, .. } => {
                    entry(&mut tar, path, EntryType::Symlink, 0o777, Some(target), &[]);
                }
                Blob::Dir { .. } => {
                    entry(&mut tar, &format!("{path}/"), EntryType::Directory, 0o755, None, &[]);
                }
            }
        }
        tar.resize(tar.len() + 2 * BLOCK, 0); // End-of-archive marker
        tar
    }
}

#[must_use]
fn fname(path: &Utf8Path) -> &str {
    path.file_name().expect("PROOF: walked files all have a name")
}

#[must_use]
fn mode(exec: bool) -> u32 {
    if exec { 0o755 } else { 0o644 }
}

/// NOTE: `--chmod` also applies to the directories a `COPY` creates, so `0644`
/// would make them unreachable to anyone but root. Files default to `0644`.
#[must_use]
fn chmod(exec: bool) -> &'static str {
    if exec { "--chmod=0755 " } else { "" }
}

#[must_use]
fn excluded(dir: &Utf8Path, fname: &str, top: bool) -> bool {
    if fname == ".git" && dir.join(fname).is_dir() {
        debug!("excluding {dir}/{fname} dir");
        return true; // Skip copying .git dirs, including submodules'
    }
    if top && is_named_same_as_virtual_target_dir(fname) {
        debug!("excluding {dir}/{fname} or it will clash with internal target dir");
        return true;
    }
    if dir.join(fname).join("CACHEDIR.TAG").exists() {
        debug!("excluding {dir}/{fname} dir");
        return true; // Test for existence of ./target/CACHEDIR.TAG See https://bford.info/cachedir/
    }
    false
}

/// Whether a heredoc can spell this file out, byte for byte.
#[must_use]
fn is_inlineable(data: &[u8], path: &Utf8Path) -> bool {
    // A heredoc's contents are the lines before its delimiter: always newline-terminated.
    if !data.is_empty() && !data.ends_with(b"\n") {
        return false;
    }
    // Longer lines make the parser lose track of the delimiter.
    if data.split(|byte| *byte == b'\n').any(|line| line.len() >= LINE_MAX) {
        return false;
    }
    // A Containerfile is a Rust String on our side, and TOML text in Mds.
    if str::from_utf8(data).is_err() {
        return false;
    }
    // COPY splits its arguments on whitespace and expands $vars in them.
    // The grouped form also spells the file name out as a heredoc delimiter.
    !path.as_str().chars().any(|c| c.is_whitespace() || c.is_control() || "\"'`$\\".contains(c))
}

/// A delimiter that `data` can't contain a line of.
#[must_use]
fn delimiting(data: &str) -> String {
    (0..)
        .map(|n| if n == 0 { EOF.to_owned() } else { format!("{EOF}_{n}") })
        .find(|delim| !data.lines().any(|line| line == delim))
        .expect("PROOF: data can't hold every suffix")
}

const BLOCK: usize = 512;

/// A tar header's name (and link name) field only holds this many bytes.
const NAME_MAX: usize = 100;

/// Appends a GNU tar entry, spilling over into the `@LongLink` entries that the
/// name and link name fields are too small for.
fn entry(
    tar: &mut Vec<u8>,
    path: &str,
    kind: EntryType,
    mode: u32,
    link: Option<&str>,
    data: &[u8],
) {
    if let Some(link) = link.filter(|link| link.len() > NAME_MAX) {
        long_link(tar, EntryType::GNULongLink, link);
    }
    if path.len() > NAME_MAX {
        long_link(tar, EntryType::GNULongName, path);
    }

    let mut header = header(kind, mode, data.len());
    // Whatever we set here is ignored when an @LongLink entry precedes us.
    header.set_path(truncated(path)).expect("PROOF: at most 100 bytes");
    if let Some(link) = link {
        header.set_link_name(truncated(link)).expect("PROOF: at most 100 bytes");
    }
    header.set_cksum();
    assert_tarball_header(&header);
    tar.extend_from_slice(header.as_bytes());
    blocks(tar, data);
}

fn long_link(tar: &mut Vec<u8>, kind: EntryType, value: &str) {
    let mut value = value.as_bytes().to_vec();
    value.push(0); // GNU stores these NUL-terminated

    let mut header = header(kind, 0o644, value.len());
    // NOTE: `set_path` drops `.` components, so write the conventional name in ourselves.
    header.as_mut_bytes()[..b"././@LongLink".len()].copy_from_slice(b"././@LongLink");
    header.set_cksum();

    tar.extend_from_slice(header.as_bytes());
    blocks(tar, &value);
}

/// The tail of `txt` that a header field can hold
#[must_use]
fn truncated(txt: &str) -> &str {
    let mut cut = txt.len().saturating_sub(NAME_MAX);
    while !txt.is_char_boundary(cut) {
        cut += 1;
    }
    &txt[cut..]
}

#[must_use]
fn header(kind: EntryType, mode: u32, size: usize) -> Header {
    let mut header = Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(size as u64);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(SOURCE_DATE_EPOCH);
    header.set_username("").expect("PROOF: empty name");
    header.set_groupname("").expect("PROOF: empty name");
    header.set_device_major(0).expect("PROOF: GNU header");
    header.set_device_minor(0).expect("PROOF: GNU header");
    header
}

fn blocks(tar: &mut Vec<u8>, data: &[u8]) {
    tar.extend_from_slice(data);
    let padding = (BLOCK - data.len() % BLOCK) % BLOCK;
    tar.resize(tar.len() + padding, 0);
}

#[must_use]
fn base64(data: &[u8], width: usize) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes =
            [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        encoded.push(ALPHABET[(n >> 18) as usize & 0b11_1111]);
        encoded.push(ALPHABET[(n >> 12) as usize & 0b11_1111]);
        encoded.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 0b11_1111] } else { b'=' });
        encoded.push(if chunk.len() > 2 { ALPHABET[n as usize & 0b11_1111] } else { b'=' });
    }

    let mut wrapped = String::with_capacity(encoded.len() + encoded.len().div_ceil(width));
    for line in encoded.chunks(width) {
        wrapped.push_str(str::from_utf8(line).expect("PROOF: base64 is ASCII"));
        wrapped.push('\n');
    }
    wrapped
}

#[test]
fn picks_narrow_down_to_the_package() {
    let picks = Picks {
        pkg: "cargo-green".into(),
        also: vec![],
        listed: vec!["cargo-green/Cargo.toml".into(), "cargo-green/src/main.rs".into()],
    };

    // $PWD's own files always go in: the crate MAY `include_str!("../README.md")`
    assert!(picks.keep("Cargo.toml".into()));
    assert!(picks.keep("Cargo.lock".into()));
    assert!(picks.keep("README.md".into()));

    assert!(picks.keep("cargo-green/src/main.rs".into()));
    assert!(!picks.keep("cargo-green/src/gitignored.rs".into())); // cargo didn't list it
    assert!(!picks.keep("recipes/some@1.0.0.Dockerfile".into()));
    assert!(!picks.keep("hack/clis.sh".into()));

    // Only walk what could hold something
    assert!(picks.descend("".into()));
    assert!(picks.descend("cargo-green".into()));
    assert!(picks.descend("cargo-green/src".into()));
    assert!(!picks.descend("recipes".into()));
    assert!(!picks.descend("hack".into()));
}

#[test]
fn picks_take_the_package_whole_when_cargo_wont_say() {
    let picks = Picks { pkg: "cargo-green".into(), also: vec![], listed: vec![] };

    assert!(picks.keep("cargo-green/src/main.rs".into()));
    assert!(picks.keep("cargo-green/whatever".into()));
    assert!(!picks.keep("recipes/some@1.0.0.Dockerfile".into()));
}

#[test]
fn picks_filter_a_single_crate_repo() {
    // The package IS $PWD: cargo's listing is all that narrows things down
    let picks = Picks {
        pkg: "".into(),
        also: vec![],
        listed: vec!["src/lib.rs".into(), "src/main.rs".into()],
    };

    assert!(picks.keep("Cargo.toml".into())); // $PWD's own files, listed or not
    assert!(picks.keep("src/lib.rs".into()));
    assert!(!picks.keep("src/scratch.rs".into())); // gitignored, so cargo left it out
    assert!(!picks.keep("node_modules/whatever.js".into()));
    assert!(picks.descend("src".into()));
}

#[test]
fn picks_default_to_all_of_pwd() {
    let picks = Picks::default();

    assert!(picks.descend("".into()));
    assert!(picks.descend("anything".into()));
    assert!(picks.keep("Cargo.toml".into()));
    assert!(picks.keep("anything/at/all.rs".into()));
}

#[test]
fn picks_reach_out_for_crate_roots_outside_the_package() {
    let picks = Picks {
        pkg: "bins".into(),
        also: vec!["shared".into()],
        listed: vec!["bins/main.rs".into()],
    };

    assert!(picks.keep("bins/main.rs".into()));
    assert!(picks.keep("shared/lib.rs".into())); // `also` isn't cargo's to list
    assert!(picks.descend("shared".into()));
    assert!(!picks.keep("elsewhere/lib.rs".into()));
}

#[test]
fn weight_flags_the_heaviest() {
    let scan = Scan {
        texts: vec![
            Text { path: "small.rs".into(), exec: false, data: "x\n".to_owned() },
            Text { path: "big.rs".into(), exec: false, data: "x".repeat(1000) + "\n" },
        ],
        blobs: vec![Blob::File { path: "logo.png".into(), exec: false, data: vec![0; 300] }],
        roots: vec![],
    };

    // base64 pads 300 bytes out to 400, plus a tar header
    assert_eq!(scan.weight(), 2 + 1001 + BLOCK + 400);

    pretty_assertions::assert_eq!(
        scan.heaviest(2),
        "              1001B big.rs\n               300B logo.png\n".to_owned()
    );
}

#[test]
fn base64_matches_coreutils() {
    // $ printf 'hello, world!\n' | base64 -w4
    assert_eq!(base64(b"hello, world!\n", 4), "aGVs\nbG8s\nIHdv\ncmxk\nIQo=\n");
    assert_eq!(base64(b"", 76), "");
    assert_eq!(base64(b"a", 76), "YQ==\n");
    assert_eq!(base64(b"ab", 76), "YWI=\n");
    assert_eq!(base64(b"abc", 76), "YWJj\n");
    assert!(base64(&[0xff; 200], 76).lines().all(|line| line.len() <= 76));
}

#[test]
fn delimiters_dodge_contents() {
    assert_eq!(delimiting("some code\n"), EOF);
    assert_eq!(delimiting(&format!("{EOF}\n")), format!("{EOF}_1"));
    assert_eq!(delimiting(&format!("{EOF}\n{EOF}_1\n")), format!("{EOF}_2"));
    // Only whole lines end a heredoc
    assert_eq!(delimiting(&format!("x{EOF}\n")), EOF);
}

#[test]
fn inlineable_files() {
    let rs = Utf8Path::new("src/lib.rs");

    assert!(is_inlineable(b"", rs));
    assert!(is_inlineable(b"fn main() {}\n", rs));
    assert!(is_inlineable("let x = \"$HOME ${PATH} \\n\";\n".as_bytes(), rs));
    assert!(is_inlineable(b"has\r\ncrlf\r\n", rs)); // heredocs keep CRLF

    assert!(!is_inlineable(b"no trailing newline", rs));
    assert!(!is_inlineable(&[b'x'; LINE_MAX], rs));
    assert!(!is_inlineable(&[0xff, b'\n'], rs)); // not utf-8
    assert!(!is_inlineable(b"ok\n", "src/two words.rs".into()));
    assert!(!is_inlineable(b"ok\n", "src/$HOME.rs".into()));
}

#[test]
fn tarball_is_deterministic_and_readable() {
    let scan = Scan {
        texts: vec![],
        blobs: vec![
            Blob::Dir { path: "empty".into() },
            Blob::Symlink { path: "link".into(), target: "../elsewhere".into() },
            Blob::File { path: "hack/logo.png".into(), exec: false, data: vec![0xff, 0x00] },
            Blob::File { path: "long/".to_owned().repeat(30).into(), exec: true, data: vec![42] },
        ],
        roots: vec![],
    };

    let tar = scan.tarball();
    assert_eq!(tar.len() % BLOCK, 0);
    assert_eq!(&tar[tar.len() - 2 * BLOCK..], &[0; 2 * BLOCK]);
    assert_eq!(tar, scan.tarball());

    // The long path doesn't fit a header: it gets its own GNULongName entry.
    assert!(tar.windows(13).any(|w| w == b"././@LongLink"));
}

#[test]
fn hash_ignores_scan_order() {
    let texts = || {
        vec![
            Text { path: "a.rs".into(), exec: false, data: "a\n".to_owned() },
            Text { path: "b.rs".into(), exec: false, data: "b\n".to_owned() },
        ]
    };
    let mut reversed = texts();
    reversed.reverse();

    let scan = Scan { texts: texts(), blobs: vec![], roots: vec![] };
    let other = Scan { texts: reversed, blobs: vec![], roots: vec![] };
    assert_eq!(scan.hash(), other.hash());
    assert_eq!(scan.hash().len(), 16);

    // ..but not what's in them
    let flipped = Scan {
        texts: vec![Text { path: "a.rs".into(), exec: true, data: "a\n".to_owned() }],
        blobs: vec![],
        roots: vec![],
    };
    assert_ne!(scan.hash(), flipped.hash());
}

#[test]
fn block_groups_by_dir_and_mode() {
    let stage = Stage::local("0123456789abcdef").unwrap();
    let scan = Scan {
        texts: vec![
            Text { path: "Cargo.toml".into(), exec: false, data: "[package]\n".to_owned() },
            Text { path: "hack/gen.sh".into(), exec: true, data: "#!/bin/sh\n".to_owned() },
            Text { path: "hack/notes".into(), exec: false, data: "notes\n".to_owned() },
            Text { path: "src/lib.rs".into(), exec: false, data: "fn a() {}\n".to_owned() },
            Text { path: "src/main.rs".into(), exec: false, data: "fn main() {}\n".to_owned() },
        ],
        blobs: vec![],
        roots: vec![],
    };

    pretty_assertions::assert_eq!(
        scan.block(&stage),
        r#"
FROM scratch AS cwd-0123456789abcdef
COPY <<"Cargo.toml" /
[package]
Cargo.toml
COPY --chmod=0755 <<"gen.sh" /hack/
#!/bin/sh
gen.sh
COPY <<"lib.rs" <<"main.rs" /src/
fn a() {}
lib.rs
fn main() {}
main.rs
COPY <<"CARGOGREEN_EOF" /hack/notes
notes
CARGOGREEN_EOF
"#[1..]
            .to_owned()
    );
}
