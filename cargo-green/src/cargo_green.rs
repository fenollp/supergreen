use std::mem;

use anyhow::{Error, Result, anyhow, bail};
use camino::Utf8PathBuf;
use log::{debug, info, warn};

use crate::{
    PKG, VSN,
    all_our_envs::{
        CARGOGREEN_BUILDER_IMAGE, CARGOGREEN_EXPERIMENT, CARGOGREEN_FINAL_PATH,
        CARGOGREEN_SYNTAX_IMAGE, find_unknowns,
    },
    base_image::{BASE_IMAGE, BASE_IMAGE_LOCKED},
    checkouts,
    containerfile::Containerfile,
    cratesio::{self},
    dirs::{Dirs, cargo_home},
    doctor,
    experiments::EXPERIMENTS,
    r#final::Final,
    green::{Green, validate_csv},
    image_uri::{SYNTAX_IMAGE_LOCKED, fetch_digest},
    lockfile::{find_lockfile, locked_crates, locked_gits},
    md::merge_blocks,
    redact::Vars,
    remote::RemoteDaemon,
    runner::Runner,
    rustup::Pinned,
    stage::Stage,
    supergreen,
    sys::fs,
};

pub(crate) struct Startup {
    pub(crate) toolchain: Pinned,
    pub(crate) is_install: bool,
    pub(crate) pwd: Utf8PathBuf,
    pub(crate) env: Vars,
    pub(crate) verbose: bool,
    pub(crate) runner: Runner,
    pub(crate) dirs: Option<Dirs>,
}

pub(crate) async fn main(
    Startup { toolchain, is_install, pwd, env, verbose, runner, dirs }: Startup,
    remote: Option<&RemoteDaemon>,
) -> Result<(Green, Option<Error>)> {
    let mut green = Green::new_from_env_then_manifest(is_install, &pwd, env)?;

    // Needed by every cmd call below
    green.verbose = verbose;
    green.color = green.env.color();

    // Fail early rather than once the build is done
    // TODO? provide a way to export final as flatpack
    if let Some(path) = green.env(CARGOGREEN_FINAL_PATH!()) {
        green.r#final.path = Some(Final::path_from_env(path, CARGOGREEN_FINAL_PATH)?);
    }

    green.runner = runner;

    green.paths.cargo_home = cargo_home()?;
    if supergreen::doctoring() {
        doctor::first_step(green.color, &green.paths)?; // Doctor reports on these, rather than fixing them
    } else {
        green.paths.setup()?;
    }
    green.paths.cwd = pwd;
    green.paths.dirs = dirs;

    green.pool = remote.and_then(|r| r.pooled(runner, green.paths.dirs.as_ref()));

    validate_csv(&mut green.experiment, CARGOGREEN_EXPERIMENT!(), &green.env)?;
    let (known, nopes): (Vec<_>, Vec<_>) = mem::take(&mut green.experiment)
        .into_iter()
        .partition(|ex| EXPERIMENTS.contains(&ex.as_str()));
    green.experiment = known;
    if !nopes.is_empty() {
        green.warn(&format!(
            "{CARGOGREEN_EXPERIMENT} contains unknown experiment names, ignoring: {nopes:?}"
        ));
    }

    match init(&mut green, &toolchain, remote).await {
        Err(e) if supergreen::doctoring() => Ok((green, Some(e))),
        res => res.map(|()| (green, None)),
    }
}

/// Finish bootstrapping
/// We stopped short earlier so `doctor` can report /all while staying read-only/
async fn init(green: &mut Green, toolchain: &Pinned, remote: Option<&RemoteDaemon>) -> Result<()> {
    if let Some(builder_image) = green.env(CARGOGREEN_BUILDER_IMAGE!()) {
        let img = builder_image
            .try_into()
            .map_err(|e| anyhow!("{CARGOGREEN_BUILDER_IMAGE}={builder_image:?} {e}"))?;
        // Don't use 'maybe_lock_image', only 'fetch_digest': cmd uses builder.
        green.builder.image = Some(fetch_digest(&green.runner, &img).await?);
    }

    let builder = green.runner_env(BUILDX_BUILDER!()).map(ToOwned::to_owned);
    if !supergreen::creating_builder() {
        green.maybe_setup_builder(builder.as_deref(), remote).await?;
        green.maybe_inspect_builder().await?;
    }

    if let Some(syntax) = green.env(CARGOGREEN_SYNTAX_IMAGE!()) {
        green.syntax =
            syntax.try_into().map_err(|e| anyhow!("{CARGOGREEN_SYNTAX_IMAGE}={syntax:?} {e}"))?;
    }
    if green.syntax.is_empty() {
        // TODO: dynamically lock, if network is up.
        green.syntax = SYNTAX_IMAGE_LOCKED.clone();
    }
    // Use local hashed image if one matching exists locally
    green.syntax = green.maybe_lock_image(&green.syntax).await?;
    // otherwise default to a hash found through some Web API
    green.syntax = fetch_digest(&green.runner, &green.syntax).await?;

    if !green.base.image.locked() {
        if green.base.image.as_str() == BASE_IMAGE.as_str() {
            // TODO: dynamically lock, if network is up.
            green.base.image = BASE_IMAGE_LOCKED.clone();
        }
        let base = green.maybe_lock_image(&green.base.image).await?;
        green.base.image = fetch_digest(&green.runner, &base).await?;
    }

    green.toolchain = Some(toolchain.clone());
    green.rust_base = Some(green.make_rust_base(None)?);

    let unknowns = find_unknowns(&green.env);
    if !unknowns.is_empty() {
        bail!("Ignored environment variable(s): {}", unknowns.join(" "))
    }

    Ok(())
}

impl Green {
    /// cargo green supergreen sync
    /// * no cargo.lock needed, but use deps if available
    /// * pull images as they are given (maybe locked)
    /// * TODO: push caches (tags?) ~ find last containerfile and rerun that build with cacheto
    ///
    /// Git dependencies are pulled apart from the rest: the builder may lack what it takes to
    /// access some (credentials), including ones this build has no use for.
    ///
    /// Besides the host's, pulls the bases of `targets`. Wrappers make the base of whatever they
    /// build for, so a target missing here only isn't pulled ahead of time.
    pub(crate) async fn prebuild(
        &self,
        require_lockfile: bool,
        is_install: bool,
        targets: &[String],
    ) -> Result<()> {
        info!("{PKG}@{VSN} original args: {:?} pwd={:?}", self.env, self.paths.cwd);

        let mut packages = vec![];
        let mut gits = vec![];
        if !is_install {
            let locked = find_lockfile(&self.paths.cwd).and_then(|manifest_path_lockfile| {
                debug!("using lockfile at {manifest_path_lockfile}");
                let packages = locked_crates(&manifest_path_lockfile)?;
                Ok((packages, locked_gits(&manifest_path_lockfile)?))
            });
            match locked {
                Ok((locked, locked_git)) => {
                    packages = locked;
                    gits = locked_git;
                    info!("found {} packages", packages.len());
                    info!("found {} git repositories", gits.len());
                }
                // TODO: probably drop `require_lockfile`
                Err(e) if require_lockfile => return Err(e),
                Err(_) => {}
            }
        }

        let stage = Stage::new("prebuild").unwrap();
        let mut containerfile = self.new_containerfile();

        let mut bases = vec![self.rust_base().clone()];
        for target in targets {
            let base = self.rust_base_for(Some(target))?;
            if !bases.contains(&base) {
                bases.push(base);
            }
        }
        containerfile.pushln(&merge_blocks(bases.iter().map(|base| base.script.as_str())));

        let stager = |i| format!("{stage}-{i}");
        let mut leaves = 0;
        // 127: https://github.com/docker/docs/issues/8230
        for (i, pkgs) in packages.chunks(127).enumerate() {
            leaves = i;

            containerfile.push(&format!("FROM scratch AS {}\n", stager(i)));

            let (name, version, hash) = &pkgs[0];
            let name_dash_version = format!("{name}-{version}");
            debug!("will fetch crate {name_dash_version}");
            containerfile.pushln(cratesio::add_step(name, &name_dash_version, hash).trim());

            for (name, version, hash) in &pkgs[1..] {
                let name_dash_version = format!("{name}-{version}");
                debug!("will fetch crate {name_dash_version}");
                containerfile.pushln(cratesio::add_step(name, &name_dash_version, hash).trim());
            }
        }

        containerfile.push(&format!("FROM scratch AS {stage}\n"));
        if leaves > 0 {
            for leaf in 0..=leaves {
                containerfile
                    .push(&format!("COPY --link --from={stg} / /{stg}\n", stg = stager(leaf)));
            }
        }
        for base in &bases {
            containerfile.push(&format!("COPY --link --from={stg} / /{stg}\n", stg = base.stage));
        }

        self.prebuild_once(containerfile, &stage).await?;

        if !gits.is_empty()
            && let Err(e) = self.prebuild_once(self.git_containerfile(&gits, &stage), &stage).await
        {
            self.warn(&format!("Some git dependencies weren't pulled ahead of time: {e}"));
        }
        Ok(())
    }

    /// Fetches each of `gits`, as (URL, commit), in a stage of its own: just as builds do.
    fn git_containerfile(&self, gits: &[(String, String)], stage: &Stage) -> Containerfile {
        let mut containerfile = self.new_containerfile();
        for (i, (repo, commit)) in gits.iter().enumerate() {
            let add = checkouts::add_step(repo, commit);
            containerfile.push(&format!("FROM scratch AS git-{i}\n{add}"));
        }

        // 127: https://github.com/docker/docs/issues/8230
        let leaves = gits.len().div_ceil(127);
        let copy = |from: String| format!("COPY --link --from={from} / /{from}\n");
        for leaf in 0..leaves {
            containerfile.push(&format!("FROM scratch AS gits-{leaf}\n"));
            for i in (leaf * 127..gits.len()).take(127) {
                containerfile.push(&copy(format!("git-{i}")));
            }
        }
        containerfile.push(&format!("FROM scratch AS {stage}\n"));
        for leaf in 0..leaves {
            containerfile.push(&copy(format!("gits-{leaf}")));
        }
        containerfile
    }

    /// Builds `stage` of `containerfile`, unless that was done already.
    async fn prebuild_once(&self, containerfile: Containerfile, stage: &Stage) -> Result<()> {
        let sentinel = self.sentinel_path(&format!("{stage}-{}", containerfile.hashed()), "done");
        info!("checking the existence of {sentinel}");
        if sentinel.exists() {
            return Ok(());
        }

        let path = sentinel.with_extension("Dockerfile");
        containerfile.write_to(&path)?;

        if self.runner.is_none() {
            info!("Skipping prebuild (runner:{})", self.runner);
            return Ok(());
        }
        self.build_cacheonly(&path, stage)
            .await
            .inspect(|()| {
                if let Err(e) = fs().write(&sentinel, "") {
                    warn!("Failed creating sentinel {sentinel}: {e}")
                }
            })
            .map_err(|e| anyhow!("{path}\n\nUnable to prebuild: {e}"))
    }
}

#[cfg(test)]
mod all_your_base_are_belong_to_us {
    use anyhow::Result;

    use crate::{
        green::Green,
        runner::Runner,
        rustup::Pinned,
        stage::Stage,
        sys::{Sys, fake::FakeFs},
    };

    const HOST: &str = "x86_64-unknown-linux-gnu";
    const ARM: &str = "aarch64-unknown-linux-gnu";
    const MUSL: &str = "armv7-unknown-linux-musleabihf";

    #[test]
    fn fetches_git_dependencies_as_builds_do() {
        let commit = "df9b810011cd416b8e3fc02911f2f496acb8475e";
        let gits: Vec<_> = (0..128)
            .map(|i| (format!("https://github.com/o/repo{i}"), commit.to_owned()))
            .collect();
        let stage = Stage::new("prebuild").unwrap();

        let containerfile = Green::default().git_containerfile(&gits, &stage);
        let containerfile = containerfile.as_str();

        let add = format!(
            "FROM scratch AS git-0\nADD --keep-git-dir=false \\\n  https://github.com/o/repo0.git#{commit} /\n"
        );
        assert!(containerfile.contains(&add), "{containerfile}");
        assert!(
            containerfile
                .contains("FROM scratch AS gits-1\nCOPY --link --from=git-127 / /git-127\n")
        );
        assert!(containerfile.ends_with(
            "FROM scratch AS prebuild\nCOPY --link --from=gits-0 / /gits-0\nCOPY --link --from=gits-1 / /gits-1\n"
        ));
    }

    #[tokio::test]
    async fn builds_the_base_of_each_target_once() -> Result<()> {
        let toolchain = Some(Pinned::parse(&format!("1.99.0-{HOST}"))?);
        let mut green = Green { runner: Runner::None, toolchain, ..Default::default() };
        green.rust_base = Some(green.make_rust_base(None)?);
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        green.prebuild(false, true, &[ARM, HOST, MUSL, ARM].map(ToOwned::to_owned)).await?;

        let path = fs.written().into_iter().find(|p| p.extension() == Some("Dockerfile")).unwrap();
        let containerfile = fs.read(&path).unwrap();
        for target in [None, Some(ARM), Some(MUSL)] {
            let stg = green.rust_base_for(target)?.stage;
            assert_eq!(
                containerfile.matches(&format!(" AS {stg}\n")).count(),
                1,
                "{containerfile}"
            );
            let copy = format!("COPY --link --from={stg} / /{stg}\n");
            assert_eq!(containerfile.matches(&copy).count(), 1, "{containerfile}");
        }
        assert_eq!(containerfile.matches("COPY --link --from=rust-").count(), 3, "{containerfile}");
        Ok(())
    }
}
