// Expansion of the `DistributableToolchain::install(options).await` subtree
// reached from maybe_install_rust(), for:
//
//   rustup-init --verbose -y --no-modify-path --profile minimal \
//               --default-toolchain 1.95 --default-host x86_64-unknown-linux-gnu
//
// on x86_64-linux-gnu, fresh install, no prior $RUSTUP_HOME, network OK.
//
// Source is rust-lang/rustup @ commit 4b2c0919 (the snapshot deepwiki indexes).
// `// NOT TAKEN: …` flags branches whose guard is false here.
// Deep callees (TOML parse, archive extraction, syscalls) are summarised in
// `/* … */` blocks because reproducing them would balloon this into 10K+ lines.
//
// At entry, DistOptions looks like:
//     toolchain          = ToolchainDesc { channel: Version(1.95), date: None,
//                                          target: "x86_64-unknown-linux-gnu" }
//     profile            = Profile::Minimal
//     update_hash        = $RUSTUP_HOME/update-hashes/1.95-x86_64-unknown-linux-gnu
//     dl_cfg             = DownloadCfg { dist_server: "https://static.rust-lang.org",
//                                        download_dir: $RUSTUP_HOME/downloads,
//                                        notify_handler, process, tmp_cx, ... }
//     force              = true        // from maybe_install_rust
//     allow_downgrade    = false
//     exists             = false
//     old_date_version   = None        // fresh install
//     components         = &[]
//     targets            = &[]


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/toolchain/distributable.rs                                          ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl<'a> DistributableToolchain<'a> {
    pub(crate) async fn install(
        options: DistOptions<'a, '_>,
    ) -> anyhow::Result<(UpdateStatus, Self)> {
        let (cfg, toolchain) = (options.cfg, options.toolchain);
        let status = InstallMethod::Dist(options).install().await?;
        Ok((status, Self::new(cfg, toolchain.clone())?))
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/install.rs                                                          ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl InstallMethod<'_, '_> {
    pub(crate) async fn install(self) -> Result<UpdateStatus> {
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(self.cfg().process.io_thread_count()?.into())
            .build_global();
        /* Rayon is bound to a thread count from process.io_thread_count(), which
           reads RUSTUP_IO_THREADS or defaults to physical core count clamped at 8.
           Used later by remove_dir_all and the Threaded diskio executor. */

        match &self {
            // Dist with no old_date_version → fresh install
            InstallMethod::Dist(DistOptions { old_date_version: None, .. }) => {
                debug!("installing toolchain {}", self.dest_basename());
                // → DEBUG installing toolchain 1.95-x86_64-unknown-linux-gnu
            }
            // _ => debug!("updating existing install for '{}'", ...)   // NOT TAKEN
        }
        debug!("toolchain directory: {}", self.dest_path().display());
        // → DEBUG toolchain directory: /home/<user>/.rustup/toolchains/1.95-x86_64-unknown-linux-gnu

        let updated = self.run(&self.dest_path()).await?;
        // run() does the dist-fetch + unpack and returns true if anything happened.

        let status = match updated {
            // false → UpdateStatus::Unchanged                                  // NOT TAKEN
            true => match &self {
                // Dist with old_date_version Some → Updated(version)           // NOT TAKEN
                InstallMethod::Dist { .. } => UpdateStatus::Installed,
            },
        };

        match Toolchain::exists(self.cfg(), &self.local_name())? {
            true => Ok(status),
            // false → Err(ToolchainNotInstallable)                             // NOT TAKEN
        }
    }

    async fn run(&self, path: &Path) -> Result<bool> {
        // if path.exists() { match self { Dist => {} _ => uninstall(path)? } }
        //                                                          // NOT TAKEN: path doesn't exist
        match self {
            // Copy / Link arms NOT TAKEN
            InstallMethod::Dist(opts) => {
                let prefix = &InstallPrefix::from(path.to_owned());
                let maybe_new_hash = opts.install_into(prefix).await?;

                if let Some(hash) = maybe_new_hash {
                    utils::write_file("update hash", &opts.update_hash, &hash)?;
                    /* Writes the SHA256 of channel-rust-1.95.toml to
                       $RUSTUP_HOME/update-hashes/1.95-x86_64-unknown-linux-gnu so
                       future `rustup update` calls can short-circuit when the
                       remote manifest hash is unchanged. */
                    Ok(true)
                } /* else { Ok(false) }                              // NOT TAKEN */
            }
        }
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/mod.rs — DistOptions::install_into                             ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl<'cfg, 'a> DistOptions<'cfg, 'a> {
    pub(crate) async fn install_into(&self, prefix: &InstallPrefix) -> Result<Option<String>> {
        let fresh_install = !prefix.path().exists();          // true

        // if fresh_install && self.update_hash.exists() { warn!("removing stray hash..."); ... }
        //                                                   // NOT TAKEN (no stray hash on fresh install)

        let mut fetched = String::new();
        let mut first_err: Option<anyhow::Error> = None;

        let backtrack = self.toolchain.channel == Channel::Nightly
                     && self.toolchain.date.is_none();
        // backtrack = false: channel is Version(1.95), not Nightly.

        // backtrack_limit setup happens but the limit is never consulted because
        // we don't enter the backtracking branch below.

        let _first_manifest = date_from_manifest_date("2014-12-20").unwrap();
        let _old_manifest = self.old_date_version.as_ref()
            .and_then(|(d, _)| date_from_manifest_date(d));     // None on fresh install

        // ╭──────────────────────────────────────────────────────────────────╮
        // │  The rest of install_into is a loop that, for nightlies missing  │
        // │  components, walks dates backward (a "backtracking" loop). For a │
        // │  pinned non-nightly like "1.95" the loop body runs once.         │
        // │  Effectively, for our args, it reduces to:                       │
        // ╰──────────────────────────────────────────────────────────────────╯

        // 1) Download channel-rust-1.95.toml + .sha256, verify, parse.
        let manifest_url = self.toolchain.manifest_v2_url(
            &self.dl_cfg.tmp_cx.dist_server,            // "https://static.rust-lang.org"
            self.dl_cfg.process,
        );
        // → https://static.rust-lang.org/dist/channel-rust-1.95.toml
        //
        // (manifest_v2_url defers to manifest_v1_url, which on our path picks the
        //  "{root}/channel-rust-{channel}" branch — no date prefix, no
        //  /staging — and then appends ".toml".)

        info!("downloading toolchain manifest");
        let manifest_dl = self.dl_cfg.dl_v2_manifest(
            None,                                       // No prior hash file
            &self.toolchain,
            self.cfg,
        ).await?;
        let (new_manifest, manifest_hash) = manifest_dl
            .expect("fresh install must yield Some(manifest)");
        // See § DownloadCfg below — fetches .toml + .toml.sha256, verifies SHA256,
        // parses the TOML into ManifestV2.

        // 2) Open the on-disk install prefix.
        let manifestation = Manifestation::open(
            prefix.clone(),
            self.toolchain.target.clone(),
        )?;
        /* Manifestation::open verifies the rust-installer metadata format if a
           prior install exists in `prefix`, otherwise just records the prefix +
           target triple. For us, prefix doesn't exist yet so it just constructs
           an empty Components handle. */

        // 3) Build the Changes set. For a *fresh* install via this code path the
        //    profile's components are passed through `explicit_add_components`
        //    (alongside any --component args; ours has none) plus rust-std for
        //    each requested --target (ours has none).
        let profile_components = new_manifest.get_profile_components(
            self.profile,                               // Profile::Minimal
            &self.toolchain.target,
        )?;
        /* get_profile_components looks up the "profiles" table in the manifest
           and returns the Component list for the named profile, fully resolved
           to (short_name, target) pairs. For Minimal on x86_64-linux-gnu that's
           exactly: rustc, rust-std (host target), cargo.  No rust-docs, no
           clippy, no rustfmt. */

        let changes = Changes {
            explicit_add_components: profile_components,   // → [rustc, rust-std, cargo]
            remove_components: vec![],
        };

        // 4) Apply.  Manifestation::update is the workhorse below.
        info!(
            "syncing channel updates for '{}'",
            self.toolchain.to_string(),
        );
        let status = manifestation.update(
            new_manifest.clone(),
            changes,
            /* force_update    = */ self.force,         // true
            /* download_cfg    = */ &self.dl_cfg,
            /* toolchain_str   = */ self.toolchain.manifest_name(),  // "1.95"
            /* implicit_modify = */ false,
        ).await?;
        // → UpdateStatus::Changed

        fetched = manifest_hash;

        // No backtrack branch entered, no `first_err` accumulated.
        // The (omitted) tail of install_into returns Ok(Some(fetched)).
        let _ = (backtrack, first_err);
        Ok(Some(fetched))
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/download.rs — fetched manifest                                 ║
// ╚══════════════════════════════════════════════════════════════════════════╝
//
// dl_v2_manifest's executed shape (reconstructed; not directly quoted):
//
// 1.  Builds the URL pair:
//         https://static.rust-lang.org/dist/channel-rust-1.95.toml
//         https://static.rust-lang.org/dist/channel-rust-1.95.toml.sha256
//
// 2.  Downloads the .sha256 file into a temp file in $RUSTUP_HOME/tmp,
//     reads its first 64 hex chars → expected_hash.
//
//     Each network fetch goes through src/download/mod.rs:
//       • Backend = Reqwest unless RUSTUP_USE_CURL=1 (NOT TAKEN here).
//       • download_file_with_resume(): opens (or resumes) a .partial file,
//         streams chunks, updating a Sha256 as bytes arrive.
//       • On EOF, finalises hash; if a hash is expected, fails on mismatch.
//
// 3.  Downloads the .toml into the hash-keyed cache at
//         $RUSTUP_HOME/downloads/<expected_hash>
//     (so repeat installs of the same toolchain skip re-download). Computes
//     Sha256 streamingly; rejects on mismatch with the .sha256 contents.
//
// 4.  Reads the .toml off disk, calls Manifest::parse(s)? (src/dist/manifest.rs)
//     which is essentially `toml::from_str::<ManifestV2>(s)` plus a few
//     post-deserialisation cleanups (e.g. populating component renames).
//
// 5.  Returns Some((parsed_manifest, expected_hash)).


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/manifestation.rs — Manifestation::open                         ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl Manifestation {
    pub fn open(prefix: InstallPrefix, triple: TargetTriple) -> Result<Self> {
        Ok(Self {
            installation: Components::open(prefix)?,
            /* Components::open: probes prefix/lib/rustlib for the rust-installer
               metadata directory layout. On a fresh prefix it just records the
               base path; no files are read. */
            target_triple: triple,
        })
    }


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/manifestation.rs — Manifestation::update (THE big function)   ║
// ╚══════════════════════════════════════════════════════════════════════════╝

    pub async fn update(
        self,
        new_manifest:    Manifest,
        changes:         Changes,
        force_update:    bool,                          // true
        download_cfg:    &DownloadCfg<'_>,
        toolchain_str:   String,                        // "1.95"
        implicit_modify: bool,                          // false
    ) -> Result<UpdateStatus> {
        let prefix = self.installation.prefix();
        let rel_installed_manifest_path = prefix.rel_manifest_file(DIST_MANIFEST);
        let installed_manifest_path = prefix.path().join(&rel_installed_manifest_path);
        // → .../1.95-…/lib/rustlib/multirust-channel-manifest.toml

        let config = self.read_config()?;               // → None on fresh install
        let mut update = Update::new(&self, &new_manifest, &changes, &config)?;
        /* Update::new (see below) walks the manifest and produces:
             components_to_install   = [rustc, rust-std, cargo]   (all explicit)
             components_to_uninstall = []
             final_component_list    = same as components_to_install
        */

        // if update.nothing_changes() { return Ok(Unchanged) }   // NOT TAKEN

        if let Err(_e) = update.unavailable_components(&new_manifest, &toolchain_str) {
            // unavailable_components checks each target_pkg.available; for our profile
            // on x86_64-linux-gnu they're all available, so NOT TAKEN.
            unreachable!();
        }

        let components = update.components_to_install
            .into_iter()
            .filter_map(|component| ComponentBinary::new(component, &new_manifest, download_cfg))
            .collect::<Result<Vec<_>>>()?;
        /* For each Component, looks up the matching `[pkg.<name>.target.<triple>]`
           in the manifest, pulls out the (.tar.xz) URL, SHA256, and CompressionKind. */

        const DEFAULT_CONCURRENT_DOWNLOADS: usize = 2;
        let concurrent_downloads = download_cfg.process.concurrent_downloads()
            .unwrap_or(DEFAULT_CONCURRENT_DOWNLOADS);
        // → 2 unless RUSTUP_CONCURRENT_DOWNLOADS overrides

        const DEFAULT_MAX_RETRIES: usize = 3;
        let max_retries: usize = download_cfg.process.var("RUSTUP_MAX_RETRIES").ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_MAX_RETRIES);            // → 3

        let mut tx = Transaction::new(
            prefix.clone(),
            download_cfg.tmp_cx.clone(),
            download_cfg.permit_copy_rename,
        );
        /* Transaction is the atomic-ish change tracker: every file written under
           prefix is recorded so commit() / rollback() (in the Drop impl) can
           clean up. */

        tx = self.maybe_handle_v2_upgrade(&config, tx)?;
        /* Walks self.installation.list(); looks_like_v1 = config.is_none() &&
           !installed_components.is_empty(). On fresh install installed list is
           empty, so this returns tx unchanged. */

        // if !update.components_to_uninstall.is_empty() && self.installation.list()?.is_empty() {
        //     info!("recovering from a partially installed toolchain");
        // }                                                       // NOT TAKEN

        // for component in update.components_to_uninstall { ... } // NOT TAKEN (empty)

        if !components.is_empty() {                                // taken: 3 components
            // if components.len() > 2 { info!("downloading {} components", ...) }
            // → info: downloading 3 components
            if components.len() > 2 {
                info!("downloading {} components", components.len());
            } /* else { info!("downloading component {}", ...) }   // NOT TAKEN */

            let mut stream = InstallEvents::new(components.into_iter(), Arc::new(self));
            let mut transaction = Some(tx);

            tx = loop {
                // Refill the download FuturesUnordered up to concurrent_downloads (2).
                while stream.components.len() > 0
                   && stream.downloads.len() < concurrent_downloads
                {
                    if let Some(bin) = stream.components.next() {
                        stream.downloads.push(bin.download(max_retries));
                        /* ComponentBinary::download (below): retries up to
                           max_retries times on BrokenPartialFile/DownloadingFile,
                           streams the .tar.xz to a hash-keyed file under
                           $RUSTUP_HOME/downloads/, verifies SHA256, then opens
                           the file ready for unpacking. */
                    }
                }

                stream.try_install(&mut transaction);
                /* Pops the next ComponentInstall off install_queue and calls
                   spawn_blocking(|| ci.install(tx, manifestation)).  That moves
                   the Transaction into a thread-pool task; only one install
                   runs at a time (the `transaction.take()` gating ensures it). */

                match stream.next().await {
                    // A download finished: ComponentInstall enqueued, yield None
                    // so the outer while-let refills downloads.
                    None => continue,
                    // An install finished: receive the tx back.
                    Some(Ok(tx_back)) => match stream.is_done() {
                        true => break tx_back,
                        false => transaction = Some(tx_back),
                    },
                    // Some(Err(e)) => return Err(e),               // NOT TAKEN
                }
            };

            download_cfg.clean(&stream.cleanup_downloads)?;
            /* Removes the hash-keyed cache files for everything just installed
               (so successful installs don't leave the download cache fat). */
            drop(stream);
        }

        // ── Write the new manifest into the toolchain ────────────────────
        let new_manifest_str = new_manifest.clone().stringify()?;
        tx.modify_file(rel_installed_manifest_path)?;
        utils::write_file("manifest", &installed_manifest_path, &new_manifest_str)?;
        // → .../lib/rustlib/multirust-channel-manifest.toml

        // ── Write the config (component/target inventory) ────────────────
        let new_config = Config {
            components: update.final_component_list,
            ..Config::default()
        };
        let config_str = new_config.stringify()?;
        let rel_config_path = prefix.rel_manifest_file(CONFIG_FILE);
        let config_path = prefix.path().join(&rel_config_path);
        tx.modify_file(rel_config_path)?;
        utils::write_file("dist config", &config_path, &config_str)?;
        // → .../lib/rustlib/multirust-config.toml

        // ── Commit ────────────────────────────────────────────────────────
        tx.commit();
        /* Walks every staged change in the transaction; for each, deletes the
           backup that would be used for rollback. Returns nothing — failure
           after commit would just leak the backups, not corrupt the install. */

        Ok(UpdateStatus::Changed)
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/manifestation.rs — Update::new (component diff)                ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl Update {
    fn new(
        manifestation: &Manifestation,
        new_manifest:  &Manifest,
        changes:       &Changes,
        config:        &Option<Config>,                 // None
    ) -> Result<Self> {
        let rust_package = new_manifest.get_package("rust")?;
        let rust_target_package = rust_package
            .get_target(Some(&manifestation.target_triple))?;

        changes.check_invariants(config)?;
        /* For each c in explicit_add_components asserts it isn't also in
           remove_components; for each c in remove_components asserts config is
           Some and contains it.  On our path: add list non-empty, remove empty,
           so passes trivially. */

        let mut starting_list = config.as_ref()
            .map(|c| c.components.clone())
            .unwrap_or_default();                       // = []
        let installed_components = manifestation.installation.list()?;  // = []
        let looks_like_v1 = config.is_none() && !installed_components.is_empty();
        // looks_like_v1 = false
        // if looks_like_v1 { starting_list.append(...) }              // NOT TAKEN

        let mut result = Self::default();

        // Seed result.final_component_list with the explicit add list.
        for component in &changes.explicit_add_components {
            result.final_component_list.push(component.clone());
        }
        // → [rustc, rust-std, cargo]

        // for existing in &starting_list { ... }                       // empty, body NOT TAKEN

        let old_manifest = manifestation.load_manifest()?;              // None
        let just_modifying_existing_install = old_manifest.as_ref() == Some(new_manifest);
        // = false

        // if just_modifying_existing_install { ... }                   // NOT TAKEN
        // else:
        result.components_to_uninstall = starting_list;                 // = []
        result.components_to_install.clone_from(&result.final_component_list);
        // → [rustc, rust-std, cargo]

        Ok(result)
    }

    // nothing_changes() / unavailable_components() / drop_components_to_install():
    // called by Manifestation::update but their bodies don't trigger on our path.
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/manifestation.rs — ComponentBinary::download                   ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl<'a> ComponentBinary<'a> {
    async fn download(self, max_retries: usize) -> Result<(ComponentInstall, &'a str)> {
        use tokio_retry::{RetryIf, strategy::FixedInterval};

        let url = self.download_cfg.url(&self.binary.url)?;
        /* Substitutes DEFAULT_DIST_SERVER ("https://static.rust-lang.org") in the
           manifest's per-component URL with the configured dist_server (which is
           also static.rust-lang.org unless RUSTUP_DIST_SERVER overrode it). */

        let installer = RetryIf::spawn(
            FixedInterval::from_millis(0).take(max_retries),   // up to 3 retries
            || self.download_cfg.download(&url, &self.binary.hash, &self.status),
            /* DownloadCfg::download fetches the .tar.xz into
               $RUSTUP_HOME/downloads/<sha256>, streaming through a Sha256 hasher
               so the file is rejected if the manifest-quoted hash doesn't match. */
            |e: &anyhow::Error| matches!(
                e.downcast_ref::<RustupError>(),
                Some(RustupError::BrokenPartialFile)
                | Some(RustupError::DownloadingFile { .. })
            ),
        ).await
         .with_context(|| RustupError::ComponentDownloadFailed(
             self.manifest.name(&self.component)))?;

        let install = ComponentInstall {
            status: self.status,
            compression: self.binary.compression,       // CompressionKind::XZ for current Rust
            installer,
            short_name: self.manifest.short_name(&self.component).to_owned(),
            component: self.component,
            temp_dir: self.download_cfg.tmp_cx.new_directory()?,
            io_executor: get_executor(
                unpack_ram(IO_CHUNK_SIZE, self.download_cfg.process.unpack_ram()?),
                self.download_cfg.process.io_thread_count()?,
            ),
            /* get_executor returns either an Immediate (synchronous) or Threaded
               executor depending on io_thread_count(): with our default the
               Threaded variant runs unpack in a thread pool, with FileBuffer
               buckets sized 4K..16M and a RAM budget read from RUSTUP_UNPACK_RAM
               or sysinfo. */
        };
        Ok((install, &self.binary.hash))
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  src/dist/manifestation.rs — ComponentInstall::install                   ║
// ╚══════════════════════════════════════════════════════════════════════════╝

impl ComponentInstall {
    fn install(self, tx: Transaction, manifestation: Arc<Manifestation>) -> Result<Transaction> {
        let pkg_name       = self.component.name_in_manifest();
        let short_pkg_name = self.component.short_name_in_manifest();

        let reader = self.status.unpack(utils::buffered(&self.installer)?);
        /* `reader` is a BufReader<File> wrapped in a progress-reporting adapter
           (DownloadStatus::unpack) that calls notify_handler with bytes-read
           updates so the user sees an unpack progress meter. */

        let package = DirectoryPackage::compressed(
            reader,
            self.compression,                           // XZ
            self.temp_dir,
            self.io_executor,
        )?;
        /* DirectoryPackage::compressed pipes the bytes through xz2::Decoder, the
           result through tar::Archive, and unpacks the archive into
           self.temp_dir using the IO executor.  For rustc that's the rust-installer
           layout: components/<name>/manifest.in listing each file relative to the
           prefix, components/<name>/<file> for each payload. */

        // if !package.contains(&pkg_name, Some(short_pkg_name)) {
        //     return Err(CorruptComponent(...).into());
        // }                                                           // NOT TAKEN

        self.status.installing();
        let tx = package.install(
            &manifestation.installation,
            &pkg_name,
            Some(short_pkg_name),
            tx,
        );
        /* DirectoryPackage::install walks the component's manifest.in line by
           line; for each entry kind:
             file <rel>         → tx.move_file from temp_dir to prefix
             dir  <rel>         → tx.move_dir
             symlink <rel> <target> → tx.create_symlink
           Each move records a backup path inside tx so a later abort would
           restore prefix to its pre-component state.  Also updates Components
           metadata under prefix/lib/rustlib so future component removes know
           which files belong to which package. */

        self.status.installed();
        tx
    }
}


// ╔══════════════════════════════════════════════════════════════════════════╗
// ║  Net effect for `--profile minimal --default-toolchain 1.95`             ║
// ╚══════════════════════════════════════════════════════════════════════════╝
//
//   GET https://static.rust-lang.org/dist/channel-rust-1.95.toml.sha256
//   GET https://static.rust-lang.org/dist/channel-rust-1.95.toml
//   GET https://static.rust-lang.org/dist/rust-std-1.95.0-x86_64-unknown-linux-gnu.tar.xz
//   GET https://static.rust-lang.org/dist/rustc-1.95.0-x86_64-unknown-linux-gnu.tar.xz
//   GET https://static.rust-lang.org/dist/cargo-1.95.0-x86_64-unknown-linux-gnu.tar.xz
//
//   Cached under  $RUSTUP_HOME/downloads/<sha256>
//   Unpacked into $RUSTUP_HOME/toolchains/1.95-x86_64-unknown-linux-gnu/{bin,lib,share,etc,libexec}
//   Manifest copy $RUSTUP_HOME/toolchains/1.95-…/lib/rustlib/multirust-channel-manifest.toml
//   Config        $RUSTUP_HOME/toolchains/1.95-…/lib/rustlib/multirust-config.toml
//   Update hash   $RUSTUP_HOME/update-hashes/1.95-x86_64-unknown-linux-gnu
//
// At most 2 component archives are downloaded in parallel (DEFAULT_CONCURRENT_DOWNLOADS);
// at most 1 is unpacked at a time (single Transaction gates install).
// Profile=Minimal cuts rust-docs, clippy, rustfmt vs. the default — that's the
// concrete observable effect of `--profile minimal` here.
