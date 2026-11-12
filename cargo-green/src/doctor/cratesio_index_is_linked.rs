use anyhow::{Result, anyhow};

use super::{Check, can_write_in, what_is_at};
use crate::{dirs::Paths, sys::fs};

pub(super) const CRATESIO_INDEX_LINK: Check = Check {
    name: "crates.io index link is up to date",
    expected: "the unhashed crates.io index dir links to the youngest hashed one, or can be made to by the next `cargo green` call",
    why: "Cargo unpacks crates in a dir whose name ends with a hash, and that hash can change with cargo's version. Builds use the name without the hash so they can share their cache, and this link makes those paths lead somewhere on your machine. `cargo green` refuses to start when it can't make this link.",
    fixes: &[
        "Move away whatever took its place, or give $CARGO_HOME/registry/src back to yourself",
    ],
    builder: false,
};

pub(super) fn cratesio_index_is_linked(paths: &Paths) -> Result<()> {
    let Some((link, _)) = paths.cratesio_index_link()? else { return Ok(()) };
    let makeable = if !fs().is_symlink(&link) && fs().exists(&link) {
        Err(anyhow!("Something else is in the way"))
    } else {
        can_write_in(&paths.cratesio_home())
    };
    makeable.map_err(|e| anyhow!("{e:#}\n{}", cratesio_index_state(paths)))
}

/// What's at the link and which indexes it could lead to
#[must_use]
fn cratesio_index_state(paths: &Paths) -> String {
    let indexes = match paths.cratesio_indexes() {
        Ok(indexes) if indexes.is_empty() => "No hashed index dirs were found".to_owned(),
        Ok(indexes) => {
            let names = indexes.iter().filter_map(|dir| dir.file_name()).collect::<Vec<_>>();
            format!("Hashed index dirs: {}", names.join(", "))
        }
        Err(e) => format!("{e:#}"),
    };
    format!("{}\n{indexes}", what_is_at(&paths.cratesio_link()))
}

#[cfg(test)]
mod index_finger {
    use anyhow::Result;

    use super::cratesio_index_is_linked;
    use crate::{
        dirs::Paths,
        doctor::first_step,
        sys::{Fs, Sys, fake::FakeFs},
    };

    const SRC: &str = "/h/registry/src";
    const HASHED: &str = "/h/registry/src/index.crates.io-1949cf8c6b5b557f";
    const LINK: &str = "/h/registry/src/index.crates.io";

    fn paths() -> Paths {
        Paths { cargo_home: "/h".into(), ..Default::default() }
    }

    #[test]
    fn nothing_to_link_to_yet() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        cratesio_index_is_linked(&paths())
    }

    #[test]
    fn missing_is_fine_and_stays_missing() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);

        cratesio_index_is_linked(&paths())?;
        assert!(!fs.exists(LINK.into()), "left alone");
        assert_eq!(fs.written(), [] as [camino::Utf8PathBuf; 0], "the probe is gone");
        Ok(())
    }

    #[test]
    fn stale_is_fine_and_stays_stale() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);
        fs.symlink_dir("/elsewhere".into(), LINK.into())?;

        cratesio_index_is_linked(&paths())?;
        assert_eq!(fs.read_link(LINK.into())?, "/elsewhere", "left alone");
        Ok(())
    }

    #[test]
    fn something_in_the_way_fails_and_says_so() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);
        fs.mkdir(LINK);

        let err = cratesio_index_is_linked(&paths()).unwrap_err().to_string();
        assert!(err.contains("Something else is in the way"), "{err}");
        assert!(err.contains(&format!("{LINK} is a real directory")), "{err}");
        assert!(err.contains("Hashed index dirs: index.crates.io-1949cf8c6b5b557f"), "{err}");
    }

    #[test]
    fn unwritable_fails_and_says_so() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);
        fs.readonly(SRC);

        let err = cratesio_index_is_linked(&paths()).unwrap_err().to_string();
        assert!(err.contains(&format!("Can't write in {SRC}")), "{err}");
        assert!(err.contains(&format!("{LINK} is missing")), "{err}");
    }

    #[test]
    fn first_step_stops_doctor_with_what_to_do() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);
        fs.mkdir(LINK);

        let err = first_step(&paths()).unwrap_err().to_string();
        assert_eq!(err, "Fix this, then run `cargo green supergreen doctor` again");
    }

    #[test]
    fn first_step_lets_doctor_go_on() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(HASHED);
        first_step(&paths())
    }
}
