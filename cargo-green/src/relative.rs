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

use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, info};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct Relative {
    stage: Stage,

    /// Host directory holding the code
    pwd: Utf8PathBuf,

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
        let Self { stage, pwd, .. } = self;

        // Re-read the code: contents are too big (and not always UTF-8) to be kept in an Md.
        let scan = Scan::of(pwd)?;

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

pub(crate) async fn as_stage(paths: &Paths, pwd: &Utf8Path) -> Result<NamedStage> {
    info!("inlining {}files under {pwd}", if pwd.join(".git").is_dir() { "git " } else { "" });

    let scan = Scan::of(pwd)?;
    let stage = Stage::local(&scan.hash())?;
    info!(
        "{stage}: {} inlined file(s), {} tarball'd, {} mount(s)",
        scan.texts.len(),
        scan.blobs.len(),
        scan.roots.len()
    );

    Ok(NamedStage::Relative(Relative {
        stage,
        pwd: pwd.to_owned(),
        dst: paths.rewrite(pwd).into(),
        blobs: !scan.blobs.is_empty(),
        roots: scan.roots,
    }))
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
    fn of(pwd: &Utf8Path) -> Result<Self> {
        let mut scan = Self::default();
        let (_, roots) = scan.walk(pwd, Utf8Path::new(""), true)?;
        scan.roots = roots;
        Ok(scan)
    }

    /// Walks `pwd/rel`, returning whether it holds tarball'd entries along with
    /// the paths to mount for it. A directory that holds none is mounted whole
    /// by our caller, which drops the paths we return here.
    fn walk(
        &mut self,
        pwd: &Utf8Path,
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

        let mut kept = 0;
        let mut tarballed = false;
        let mut roots = vec![];

        for fname in fnames {
            if excluded(&dir, &fname, top) {
                continue;
            }
            kept += 1;

            let path = dir.join(&fname);
            let child = rel.join(&fname);

            let md =
                fs::symlink_metadata(&path).map_err(|e| anyhow!("Failed to `stat {path}`: {e}"))?;

            if md.is_symlink() {
                let target = fs::read_link(&path)
                    .map_err(|e| anyhow!("Failed to `readlink {path}`: {e}"))?;
                let Some(target) = target.to_str().map(ToOwned::to_owned) else {
                    bail!("Symlink {path} does not point to a utf-8 path")
                };
                self.blobs.push(Blob::Symlink { path: child, target });
                tarballed = true;
            } else if md.is_dir() {
                let (child_tarballed, child_roots) = self.walk(pwd, &child, false)?;
                if child_tarballed {
                    tarballed = true;
                    roots.extend(child_roots);
                } else {
                    roots.push(child);
                }
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

        if kept == 0 && !rel.as_str().is_empty() {
            // An empty dir has to come from the tarball, so it must not be shadowed by a mount.
            self.blobs.push(Blob::Dir { path: rel.to_owned() });
            return Ok((true, vec![]));
        }

        Ok((tarballed, roots))
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

