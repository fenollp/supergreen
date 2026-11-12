use anyhow::{Result, anyhow, ensure};
use camino::Utf8Path;

use super::{CHOWN, Check, buildkit::nonce};
use crate::sys::fs;

pub(super) const XDG_RESULTS: Check = Check {
    name: "results dir is usable",
    expected: "a dir of ours we can read, write and go through",
    why: "Each crate's build output is saved here as a tarball, so it can be reused instead of built again. If this dir isn't usable, those outputs can't be saved or read back.",
    fixes: &[CHOWN],
    builder: false,
};

pub(super) const XDG_BUILDKIT: Check = Check {
    name: "buildkit dir is usable",
    expected: "a dir of ours we can read, write and go through",
    why: "With the `cachebuildkit` experiment on, BuildKit's build cache gets saved to and reused from here. If this dir isn't usable, that cache can't be kept across builders.",
    fixes: &[CHOWN],
    builder: false,
};

pub(super) const XDG_VAR_RUN: Check = Check {
    name: "sockets dir is usable",
    expected: "a dir of ours we can read, write and go through, closed to everyone else (0700)",
    why: "Connections to remote daemons and the chatter between cargo-green and its wrappers go through sockets kept here. Whoever can reach these sockets can talk to your daemon, so nobody else should have access.",
    fixes: &[CHOWN, "`chmod 700 DIR`"],
    builder: false,
};

pub(super) fn dir_is_usable(dir: &Utf8Path, exact_mode: Option<u32>) -> Result<()> {
    // NOTE: modes and owners are those of the dir a link leads to
    let real = fs().canonicalize(dir).map_err(|e| anyhow!("{dir}: {e}"))?;
    ensure!(fs().is_dir(&real), "{dir} isn't a directory");

    let mode = fs().mode(&real).map_err(|e| anyhow!("{dir}: {e}"))?;
    match exact_mode {
        Some(exact) => ensure!(mode == exact, "{dir} has mode {mode:o}, not {exact:o}"),
        None => {
            ensure!(mode & 0o700 == 0o700, "{dir} has mode {mode:o}, lacking rwx for its owner")
        }
    }

    let probe = dir.join(format!(".cargo-green-doctor-{}", nonce()?));
    fs().write(&probe, "").map_err(|e| anyhow!("Can't write in {dir}: {e}"))?;
    let us = fs().uid(&probe);
    fs().remove_file(&probe).map_err(|e| anyhow!("Can't remove from {dir}: {e}"))?;
    let (us, owner) = (us?, fs().uid(&real)?);
    ensure!(owner == us, "{dir} belongs to uid {owner}, rather than to us ({us})");
    Ok(())
}

#[cfg(test)]
mod knock_knock {
    use anyhow::Result;

    use super::dir_is_usable;
    use crate::sys::{Fs, Sys, fake::FakeFs};

    #[test]
    fn dirs_get_probed() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        let dir = "/d".into();
        fs.mkdir("/d");

        fs.create_dir_all_mode(dir, 0o700)?;
        dir_is_usable(dir, Some(0o700))?;
        assert_eq!(fs.written(), [] as [camino::Utf8PathBuf; 0], "the probe is gone");

        fs.create_dir_all_mode(dir, 0o755)?;
        dir_is_usable(dir, None)?;
        let err = dir_is_usable(dir, Some(0o700)).unwrap_err().to_string();
        assert!(err.contains("has mode 755, not 700"), "{err}");

        fs.create_dir_all_mode(dir, 0o055)?;
        let err = dir_is_usable(dir, None).unwrap_err().to_string();
        assert!(err.contains("lacking rwx for its owner"), "{err}");

        fs.file("/f", "");
        let err = dir_is_usable("/f".into(), None).unwrap_err().to_string();
        assert!(err.contains("isn't a directory"), "{err}");
        assert!(dir_is_usable("/nope".into(), None).is_err());
        Ok(())
    }

    #[test]
    fn dirs_of_others_are_refused() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/d");
        fs.chown("/d", 0);
        let err = dir_is_usable("/d".into(), None).unwrap_err().to_string();
        assert!(err.contains("belongs to uid 0, rather than to us"), "{err}");
    }

    #[test]
    fn dirs_behind_links_are_probed_where_they_lead() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/real");
        fs.create_dir_all_mode("/real".into(), 0o700)?;
        fs.symlink_dir("/real".into(), "/link".into())?;
        dir_is_usable("/link".into(), Some(0o700))
    }
}
