use anyhow::{Result, bail};

use super::Check;
use crate::{dirs::Paths, sys::fs};

pub(super) const CRATESIO_CRATES: Check = Check {
    name: "crates.io sources have their crates",
    expected: "each $CARGO_HOME/registry/src/index.crates.io-<hash> dir has its $CARGO_HOME/registry/cache/index.crates.io-<hash> counterpart",
    why: "Crates are added to builds from the `.crate` files cargo downloaded, which are looked for next to the sources cargo unpacked from them. Sources without their `.crate` files can't be built.",
    fixes: &["`cargo fetch` in your project, or remove the sources dir: cargo unpacks it again"],
    builder: false,
};

pub(super) fn cratesio_sources_have_their_crates(paths: &Paths) -> Result<()> {
    let orphans: Vec<_> = paths
        .cratesio_indexes()?
        .into_iter()
        .filter(|index| !fs().is_dir(&paths.cratesio_cache_of(index)))
        .collect();
    if !orphans.is_empty() {
        let missing: Vec<_> =
            orphans.iter().map(|index| paths.cratesio_cache_of(index).to_string()).collect();
        bail!("Missing: {}", missing.join(", "))
    }
    Ok(())
}

#[cfg(test)]
mod where_are_the_crates {
    use super::cratesio_sources_have_their_crates;
    use crate::{
        dirs::Paths,
        sys::{Sys, fake::FakeFs},
    };

    const SRC: &str = "/h/registry/src/index.crates.io-1949cf8c6b5b557f";
    const CACHE: &str = "/h/registry/cache/index.crates.io-1949cf8c6b5b557f";

    fn paths() -> Paths {
        Paths { cargo_home: "/h".into(), ..Default::default() }
    }

    #[test]
    fn nothing_fetched_yet_is_fine() {
        let _guard = Sys::install(Sys { fs: FakeFs::new(), ..Sys::fake() });
        assert!(cratesio_sources_have_their_crates(&paths()).is_ok());
    }

    #[test]
    fn sources_with_their_crates_are_fine() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(SRC);
        fs.mkdir(CACHE);
        assert!(cratesio_sources_have_their_crates(&paths()).is_ok());
    }

    #[test]
    fn whatever_the_hash_is_called_it_is_looked_up() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h/registry/src/index.crates.io-6f17d22bba15001f");
        fs.mkdir("/h/registry/cache/index.crates.io-6f17d22bba15001f");
        assert!(cratesio_sources_have_their_crates(&paths()).is_ok());
    }

    #[test]
    fn sources_without_their_crates_are_not() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir(SRC);
        fs.mkdir(CACHE);
        fs.mkdir("/h/registry/src/index.crates.io-6f17d22bba15001f");

        let err = cratesio_sources_have_their_crates(&paths()).unwrap_err().to_string();
        assert_eq!(err, "Missing: /h/registry/cache/index.crates.io-6f17d22bba15001f");
    }

    #[test]
    fn other_registries_are_not_ours_to_check() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.mkdir("/h/registry/src/github.com-1ecc6299db9ec823");
        assert!(cratesio_sources_have_their_crates(&paths()).is_ok());
    }
}
