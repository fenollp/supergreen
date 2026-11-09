use anyhow::{Result, anyhow, ensure};
use camino::Utf8Path;

use super::{Check, SETUP, can_write_in, what_is_at};
use crate::{base_image::CARGO_HOME, dirs::Paths, sys::fs};

pub(super) const CARGO_HOME_LINK: Check = Check {
    name: "$CARGO_HOME is followable from the host",
    expected: "/usr/local/cargo, the $CARGO_HOME of builds, leads to the host's $CARGO_HOME, or can be made to by the next `cargo green` call",
    why: "Inside builds, cargo's home is always /usr/local/cargo, whatever it is on your machine. Paths that come out of builds point there, and this link is what makes them lead somewhere on your machine. `cargo green` refuses to start when it is missing.",
    fixes: &[SETUP, "If it leads elsewhere, remove it first: `sudo rm /usr/local/cargo`"],
    builder: false,
};

/// Predicts what every `cargo green` call starts with, leaving the link alone: one that's missing
/// until then is no issue, as long as it can be made without root.
pub(super) fn cargo_home_is_followable(paths: &Paths) -> Result<()> {
    let guest = Utf8Path::new(CARGO_HOME);
    if !fs().exists(guest) && !fs().is_symlink(guest) {
        return can_write_in(guest.parent().unwrap_or(guest))
            .map_err(|e| anyhow!("{e:#}\n{}", what_is_at(guest)));
    }
    leads_to(guest, &paths.cargo_home).map_err(|e| anyhow!("{e:#}\n{}", what_is_at(guest)))
}

fn leads_to(link: &Utf8Path, target: &Utf8Path) -> Result<()> {
    let resolved = fs().canonicalize(link).map_err(|e| anyhow!("{link}: {e}"))?;
    let target = fs().canonicalize(target).map_err(|e| anyhow!("{target}: {e}"))?;
    ensure!(resolved == target, "{link} leads to {resolved}, rather than to {target}");
    Ok(())
}

#[cfg(test)]
mod home_is_where_the_symlink_is {
    use anyhow::Result;

    use super::{cargo_home_is_followable, leads_to};
    use crate::{
        dirs::Paths,
        doctor::first_step,
        sys::{Fs, Sys, fake::FakeFs},
    };

    fn paths() -> Paths {
        Paths { cargo_home: "/h".into(), ..Default::default() }
    }

    const GUEST: &str = "/usr/local/cargo";

    #[test]
    fn guest_link_missing_is_fine_when_it_can_be_made() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h");

        cargo_home_is_followable(&paths())?;
        assert!(!fs.exists(GUEST.into()), "left alone");
        assert_eq!(fs.written(), [] as [camino::Utf8PathBuf; 0], "the probe is gone");
        Ok(())
    }

    #[test]
    fn guest_link_missing_stops_doctor_when_it_cannot_be_made() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h");
        fs.readonly("/usr/local");

        let err = cargo_home_is_followable(&paths()).unwrap_err().to_string();
        assert!(err.contains("Can't write in /usr/local"), "{err}");
        assert!(err.contains("/usr/local/cargo is missing"), "{err}");

        let err = first_step(false, &paths()).unwrap_err().to_string();
        assert_eq!(err, "Fix this, then run `cargo green supergreen doctor` again");
    }

    #[test]
    fn guest_link_leading_elsewhere_stops_doctor() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h");
        fs.mkdir("/elsewhere");
        fs.symlink_dir("/elsewhere".into(), GUEST.into())?;

        let err = cargo_home_is_followable(&paths()).unwrap_err().to_string();
        assert!(err.contains("rather than to /h"), "{err}");
        assert!(err.contains("/usr/local/cargo links to /elsewhere"), "{err}");
        assert!(first_step(false, &paths()).is_err());
        Ok(())
    }

    #[test]
    fn guest_link_broken_stops_doctor() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h");
        fs.symlink_dir("/gone".into(), GUEST.into())?;

        assert!(cargo_home_is_followable(&paths()).is_err());
        Ok(())
    }

    #[test]
    fn guest_link_right_lets_doctor_go_on() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h");
        fs.symlink_dir("/h".into(), GUEST.into())?;

        cargo_home_is_followable(&paths())?;
        first_step(false, &paths())
    }

    #[test]
    fn links_get_followed() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/a");
        fs.mkdir("/b");
        fs.symlink_dir("/a".into(), "/link".into())?;

        leads_to("/link".into(), "/a".into())?;
        let err = leads_to("/link".into(), "/b".into()).unwrap_err().to_string();
        assert!(err.contains("rather than to"), "{err}");
        assert!(leads_to("/nope".into(), "/a".into()).is_err());
        Ok(())
    }
}
