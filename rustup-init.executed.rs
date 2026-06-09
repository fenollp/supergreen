// rustup-init binary, executed path for:
//   rustup-init --verbose -y --no-modify-path --profile minimal \
//               --default-toolchain 1.95 --default-host x86_64-unknown-linux-gnu
//
// Source is from rust-lang/rustup @ main, sources retained in original order.
// `// NOT TAKEN: …` flags branches whose guard is false under our args/env.
// Bodies of deep helpers (manifest download, archive extraction, settings IO,
// shell-rc editing) are summarised in `/* … */` blocks rather than inlined,
// because reproducing every callee would be tens of thousands of lines.
//
// After parsing, clap fills RustupInit as:
//     verbose                       = true
//     quiet                         = false
//     no_prompt                     = true     // from -y
//     default_host                  = Some("x86_64-unknown-linux-gnu")
//     default_toolchain             = Some(Some("1.95"-parsed))
//     profile                       = Profile::Minimal
//     component                     = []
//     target                        = []
//     no_update_default_toolchain   = false
//     no_modify_path                = true
//     self_replace                  = false
//     dump_testament                = false


// ────────────────────────────────────────────────────────────────────────────
// src/bin/rustup-init.rs
// ────────────────────────────────────────────────────────────────────────────

fn main() -> Result<ExitCode> {
    // #[cfg(windows)] pre_rustup_main_init();         // NOT TAKEN: unix build
    let process = Process::os();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(process.io_thread_count()?.into())
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        // #[cfg(feature = "otel")] let _telemetry_guard = …;  // NOT TAKEN
        tracing_log::LogTracer::init()?;
        let (subscriber, console_filter) = log::tracing_subscriber(&process);
        tracing::subscriber::set_global_default(subscriber)?;
        run_rustup(&process, console_filter).await
    });
    match result {
        // Err(e) => …                                  // NOT TAKEN on happy path
        Ok(utils::ExitCode(c)) => std::process::exit(c),
    }
}

async fn run_rustup(process, console_filter) -> Result<utils::ExitCode> {
    // if process.var("RUSTUP_TRACE_DIR").is_ok() { open_trace_file!(…)?; }  // NOT TAKEN
    let result = run_rustup_inner(process, console_filter).await;
    // if process.var("RUSTUP_TRACE_DIR").is_ok() { close_trace_file!(); }   // NOT TAKEN
    result
}

async fn run_rustup_inner(process, console_filter) -> Result<utils::ExitCode> {
    do_recursion_guard(process)?;             // passes (count=0, max not exceeded)
    let current_dir = process.current_dir().context(RustupError::LocatingWorkingDir)?;
    utils::current_exe()?;
    match process.name().as_deref() {
        // Some("rustup") => …                                          // NOT TAKEN
        Some(n) if n.starts_with("rustup-setup") || n.starts_with("rustup-init") => {
            setup_mode::main(current_dir, process, console_filter).await   // ← taken
        }
        // Some(n) if n.starts_with("rustup-gc-") => …                  // NOT TAKEN
        // Some(n) => proxy_mode::main(…)                               // NOT TAKEN
        // None => Err(CliError::NoExeName.into())                      // NOT TAKEN
    }
}

fn do_recursion_guard(process) -> Result<()> {
    let recursion_count = process.var("RUST_RECURSION_COUNT").ok()
        .and_then(|s| s.parse().ok()).unwrap_or(0);
    // if recursion_count > RUST_RECURSION_COUNT_MAX { return Err(…) }  // NOT TAKEN
    Ok(())
}


// ────────────────────────────────────────────────────────────────────────────
// src/cli/setup_mode.rs
// ────────────────────────────────────────────────────────────────────────────

pub async fn main(current_dir, process, console_filter) -> Result<utils::ExitCode> {
    let RustupInit {
        verbose, quiet, no_prompt, default_host, default_toolchain,
        profile, component, target, no_update_default_toolchain,
        no_modify_path, self_replace, dump_testament,
    } = match RustupInit::try_parse() {
        Ok(args) => args,                            // ← taken
        // Err(e) if [DisplayHelp, DisplayVersion].contains(&e.kind()) => …   // NOT TAKEN
        // Err(e) => return Err(…)                                            // NOT TAKEN
    };

    // if self_replace      { return self_update::self_replace(process); }    // NOT TAKEN
    // if dump_testament    { return common::dump_testament(process); }       // NOT TAKEN
    // if profile == Profile::Complete { warn!(...) }                         // NOT TAKEN (Minimal)

    update_console_filter(process, &console_filter, /* quiet */ false, /* verbose */ true);
    // → sets the console EnvFilter to DEBUG for the rustup target.

    let opts = InstallOpts {
        default_host_tuple:  Some("x86_64-unknown-linux-gnu".into()),
        default_toolchain:   Some(/* parsed 1.95 */ ..),
        profile:             Profile::Minimal,
        no_modify_path:      true,
        no_update_toolchain: false,                  // ← from no_update_default_toolchain
        components:          &[],
        targets:             &[],
    };

    let mut cfg = Cfg::from_env(current_dir, /* quiet */ false, false, process)?;
    /* Cfg::from_env: resolves RUSTUP_HOME / CARGO_HOME (defaulting to ~/.rustup,
       ~/.cargo), loads settings.toml if it exists (it doesn't on a fresh
       install), prepares the toolchain dir layout in memory. */

    self_update::install(/* no_prompt */ true, opts, &mut cfg).await
}


// ────────────────────────────────────────────────────────────────────────────
// src/cli/self_update.rs
// ────────────────────────────────────────────────────────────────────────────

pub(crate) async fn install(
    no_prompt: bool,                 // true
    mut opts:  InstallOpts<'_>,
    cfg:       &mut Cfg<'_>,
) -> Result<ExitCode> {
    let mut exit_code = ExitCode::SUCCESS;

    opts.validate(cfg.process).map_err(|e| { /* anyhow!(…) */ })?;
    /* validate(): warn_if_host_is_emulated, then resolve "1.95" against the
       supplied host triple; succeeds, traces "Successfully resolved …". */

    if cfg.process.var_os("RUSTUP_INIT_SKIP_EXISTENCE_CHECKS")
        .is_none_or(|s| s != "yes")                  // ← true (var unset)
    {
        check_existence_of_rustc_or_cargo_in_path(no_prompt, cfg.process)?;
        check_existence_of_settings_file(cfg)?;
    }

    #[cfg(unix)] {
        exit_code &= unix::do_anti_sudo_check(no_prompt, cfg.process)?;
        /* If $HOME is inconsistent with getpwuid(geteuid()), bail or warn.
           Under a normal non-sudo invocation: returns ExitCode::SUCCESS. */
    }

    let mut term = cfg.process.stdout();

    // #[cfg(windows)] windows::maybe_install_msvc(…).await?;     // NOT TAKEN

    // if !no_prompt { /* big interactive customise/confirm loop */ }
    //                                                            // NOT TAKEN: -y set no_prompt=true
    //
    // The skipped block would have:
    //   • emitted pre_install_msg_unix_no_modify_path!() (because of --no-modify-path),
    //   • shown the current_install_opts(...) summary,
    //   • called common::confirm_advanced(...) and looped on Yes/No/Advanced,
    //   • on Advanced, run opts.customize(process)? to re-prompt host triple,
    //     toolchain, profile, and PATH preference.

    let no_modify_path = opts.no_modify_path;        // = true
    if let Err(e) = maybe_install_rust(opts, cfg).await {
        // report_error / windows ensure_prompt / return FAILURE   // NOT TAKEN on success
    }

    let cargo_home = canonical_cargo_home(cfg.process)?;
    /* If CARGO_HOME == $HOME/.cargo, returns the literal "$HOME/.cargo";
       otherwise the real path. */

    // #[cfg(windows)] let cargo_home = cargo_home.replace('\\', r"\\");   // NOT TAKEN
    // #[cfg(windows)] let msg = …;                                        // NOT TAKEN

    #[cfg(not(windows))]
    let source_env_lines = shell::build_source_env_lines(cfg.process);
    /* Walks the user's shells (bash/zsh/fish/nu/…) and produces the
       `. "$HOME/.cargo/env"` / `source …env.fish` / etc. lines to print. */

    #[cfg(not(windows))]
    let msg = if no_modify_path {                    // ← true
        format!(post_install_msg_unix_no_modify_path!(),
                cargo_home = cargo_home,
                source_env_lines = source_env_lines)
    } /* else { post_install_msg_unix!() } */ ;      // else NOT TAKEN

    md(&mut term, msg);                              // render the post-install markdown

    #[cfg(unix)]
    warn_if_default_linker_missing(cfg.process);
    /* Uses cc-rs to figure out which `cc`-ish binary would be used for the
       host triple, then searches PATH for it; warns if missing. */

    // #[cfg(windows)] if !no_prompt { windows::ensure_prompt(...) }   // NOT TAKEN

    Ok(exit_code)
}


// ── check_existence_of_rustc_or_cargo_in_path ───────────────────────────────

fn check_existence_of_rustc_or_cargo_in_path(no_prompt, process) -> Result<()> {
    // if process.var_os("RUSTUP_INIT_SKIP_PATH_CHECK") == Some("yes") { … }   // NOT TAKEN
    if let Err(path) = rustc_or_cargo_exists_in_path(process) {
        // warn! × 7, then ignorable_error(...)         // NOT TAKEN: no prior install
    }
    Ok(())
}

fn rustc_or_cargo_exists_in_path(process) -> Result<()> {
    fn ignore_paths(path: &PathBuf) -> bool {
        !path.components().any(|c| c == Component::Normal(".cargo".as_ref()))
    }
    if let Some(paths) = process.var_os("PATH") {     // ← Some on Linux
        let paths = env::split_paths(&paths).filter(ignore_paths);
        for path in paths {
            let rustc = path.join(format!("rustc{EXE_SUFFIX}"));
            let cargo = path.join(format!("cargo{EXE_SUFFIX}"));
            // if rustc.exists() || cargo.exists() { return Err(…) }   // NOT TAKEN (assumption)
        }
    }
    Ok(())
}


// ── check_existence_of_settings_file ────────────────────────────────────────

fn check_existence_of_settings_file(cfg) -> Result<()> {
    let rustup_dir = cfg.process.rustup_home()?;
    let settings_file_path = rustup_dir.join("settings.toml");
    if !utils::path_exists(&settings_file_path) {     // ← true: file absent on fresh install
        return Ok(());                                // ← taken, function returns here
    }
    // everything below NOT TAKEN
}


// ── maybe_install_rust ──────────────────────────────────────────────────────

async fn maybe_install_rust(opts: InstallOpts<'_>, cfg: &mut Cfg<'_>) -> Result<()> {
    install_bins(cfg.process)?;

    #[cfg(unix)]
    unix::do_write_env_files(cfg.process)?;
    /* For each detected shell, write its env script under $CARGO_HOME
       (env / env.fish / env.nu). Idempotent: skips ones already present
       with matching content. */

    // if !opts.no_modify_path { do_add_to_path(cfg.process)?; }   // NOT TAKEN (--no-modify-path)

    if cfg.process.var_os("RUSTUP_HOME").is_none() {  // ← true (unset)
        let home = cfg.process.home_dir()
            .map(|p| p.join(".rustup"))
            .ok_or_else(|| anyhow::anyhow!("could not find home dir to put .rustup in"))?;
        fs::create_dir_all(home).context("unable to create ~/.rustup")?;
    }

    let (components, targets) = (opts.components, opts.targets);   // both empty
    let toolchain = opts.install(cfg)?;                            // returns Some(1.95-…)
    if let Some(desc) = &toolchain {
        let options = DistOptions::new(components, targets, desc,
                                       cfg.get_profile()?,         // → Minimal
                                       true, cfg)?;
        let status = if Toolchain::exists(cfg, &desc.into())? {
            // Update path:                                         // NOT TAKEN (no toolchain yet)
            // warn!("Updating existing toolchain, profile choice will be ignored");
            // let toolchain = DistributableToolchain::new(cfg, desc.clone())?;
            // InstallMethod::Dist(options.for_update(&toolchain, false)?).await?
            unreachable!()
        } else {
            DistributableToolchain::install(options).await?.0      // ← taken
            /* The big one. Fetches and verifies channel manifest from
               https://static.rust-lang.org/dist, resolves the "minimal" profile
               components (rustc, rust-std, cargo), downloads & SHA-256-checks
               each .tar.xz, extracts into $RUSTUP_HOME/toolchains/1.95-…,
               writes manifest metadata. Returns an UpdateStatus describing
               which components landed. */
        };

        check_proxy_sanity(cfg.process, components, desc)?;
        /* For each c in components ∩ {"cargo","rustc"} runs `c +<desc> --version`.
           Components is empty → loop doesn't iterate → returns Ok. */

        cfg.set_default(Some(&desc.into()))?;
        /* Writes default_toolchain = "1.95-x86_64-unknown-linux-gnu"
           into $RUSTUP_HOME/settings.toml. */

        writeln!(cfg.process.stdout().lock())?;
        common::show_channel_update(cfg, PackageUpdate::Toolchain(desc.clone()), Ok(status))?;
        /* Renders the table like:
              1.95-x86_64-unknown-linux-gnu installed - rustc 1.95.0 (…)
           to stdout. */
    }
    Ok(())
}


// ── InstallOpts::install (called above) ─────────────────────────────────────

impl InstallOpts<'_> {
    fn install(self, cfg: &mut Cfg<'_>) -> Result<Option<ToolchainDesc>> {
        let Self {
            default_host_triple,           // Some("x86_64-unknown-linux-gnu")
            default_toolchain,             // Some(MaybeOfficialToolchainName::Some(1.95))
            profile,                       // Minimal
            no_modify_path: _,
            no_update_toolchain,           // false
            components,                    // []
            targets,                       // []
        } = self;

        cfg.set_profile(profile)?;         // persists profile = "minimal" in settings.toml

        if let Some(default_host_triple) = &default_host_triple {   // ← Some
            info!("setting default host triple to {}", default_host_triple);
            cfg.set_default_host_triple(default_host_triple.to_owned())?;
        } /* else { info!("default host triple is {}", cfg.get_default_host_triple()?) } */

        let user_specified_something = default_toolchain.is_some()  // true
            || !targets.is_empty()
            || !components.is_empty()
            || !no_update_toolchain;                                // true (= !false)

        // if matches!(default_toolchain, Some(MaybeOfficialToolchainName::None)) { … }
        //                                                          // NOT TAKEN: not "none"
        // else if user_specified_something || (!no_update_toolchain && cfg.find_default()?.is_none())
        //                                                          // ← taken (user_specified_something)
        Ok(match default_toolchain {
            Some(s) => {
                let toolchain_name = match s {
                    // MaybeOfficialToolchainName::None => unreachable!()        // NOT TAKEN
                    MaybeOfficialToolchainName::Some(n) => n,
                };
                Some(toolchain_name.resolve(&cfg.get_default_host_triple()?)?)
                // → ToolchainDesc { channel: "1.95", host: "x86_64-unknown-linux-gnu", date: None }
            }
            // None => match cfg.get_default()? { … }                            // NOT TAKEN
        })
        /* The trailing else { info!("updating existing rustup installation - leaving toolchains alone"); Ok(None) }
           is NOT TAKEN. */
    }
}


// ── install_bins → install_proxies (cargo-home wiring) ──────────────────────

fn install_bins(process) -> Result<()> {
    let bin_path     = process.cargo_home()?.join("bin");          // ~/.cargo/bin
    let this_exe_path = utils::current_exe()?;                      // the running rustup-init
    let rustup_path  = bin_path.join(format!("rustup{EXE_SUFFIX}"));// ~/.cargo/bin/rustup

    utils::ensure_dir_exists("bin", &bin_path)?;

    // if rustup_path.exists() { utils::remove_file("rustup-bin", &rustup_path)?; }
    //                                                              // NOT TAKEN (fresh install)

    utils::copy_file_symlink_to_source(&this_exe_path, &rustup_path)?;
    utils::make_executable(&rustup_path)?;
    install_proxies(process)
}

pub(crate) fn install_proxies(process) -> Result<()> {
    install_proxies_with_opts(process, process.var_os("RUSTUP_HARDLINK_PROXIES").is_some())
    // → second arg = false on our env
}

fn install_proxies_with_opts(process, force_hard_links: bool /* false */) -> Result<()> {
    let bin_path    = process.cargo_home()?.join("bin");
    let rustup_path = bin_path.join(format!("rustup{EXE_SUFFIX}"));
    let rustup      = Handle::from_path(&rustup_path)?;
    let mut tool_handles    = Vec::new();
    let mut link_afterwards = Vec::new();

    for tool in TOOLS {                              // rustc, cargo, rustdoc, …
        let tool_path = bin_path.join(format!("{tool}{EXE_SUFFIX}"));
        // if let Ok(handle) = Handle::from_path(&tool_path) { … }  // NOT TAKEN (no files yet)
        link_afterwards.push(tool_path);
    }

    let link_proxy = utils::symlink_or_hardlink_file; // because force_hard_links == false

    for tool in DUP_TOOLS {                          // rust-gdb, rust-lldb, …
        let tool_path = bin_path.join(format!("{tool}{EXE_SUFFIX}"));
        // if let Ok(handle) = … { … }                              // NOT TAKEN (no files yet)
        link_proxy(&rustup_path, &tool_path)?;
    }

    drop(tool_handles);
    for path in link_afterwards {
        link_proxy(&rustup_path, &path)?;
    }

    if !force_hard_links {                            // ← true (force=false)
        let path = bin_path.join(format!("{tool}{EXE_SUFFIX}", tool = TOOLS[0]));
        // if fs::File::open(path).is_err() {
        //     return install_proxies_with_opts(process, /*force=*/ true);
        // }                                                        // NOT TAKEN: symlink works on Linux
    }
    Ok(())
}


// ── warn_if_default_linker_missing (post-install courtesy check) ────────────

#[cfg(unix)]
fn warn_if_default_linker_missing(process) {
    let Some(path) = process.var_os("PATH") else {    // ← Some on Linux
        // warn!(no PATH), return                                   // NOT TAKEN
    };
    let cc_tool = TargetTriple::from_host(process).and_then(|triple| {
        cc::Build::new().opt_level(0).target(&triple).host(&triple)
            .try_get_compiler().ok()
    });
    let cc_binary = if let Some(cc_tool) = &cc_tool {
        Cow::Borrowed(cc_tool.path())                 // typically "cc"
    } else {
        Cow::Owned(format!("cc{EXE_SUFFIX}").into())
    };
    let found = env::split_paths(&path).any(|mut p| { p.push(&cc_binary); p.is_file() });
    // if !found { warn!("no default linker (`cc`) was found in your PATH"); warn!(…); }
    //                                                              // depends on environment
}


// ── canonical_cargo_home ────────────────────────────────────────────────────

fn canonical_cargo_home(process) -> Result<Cow<'static, str>> {
    let path = process.cargo_home()?;
    let default_cargo_home = process.home_dir().unwrap_or_else(|| PathBuf::from("."))
        .join(".cargo");
    Ok(if default_cargo_home == path {                // typically true
        // #[cfg(windows)] r"%USERPROFILE%\.cargo".into()           // NOT TAKEN
        "$HOME/.cargo".into()
    } /* else { path.to_string_lossy().into_owned().into() } */ )
}


// ── The post-install message macro that ends up rendered ────────────────────

macro_rules! post_install_msg_unix_no_modify_path { () => { r"
# Rust is installed now. Great!

To get started you need Cargo's bin directory ({cargo_home}/bin) in your `PATH`
environment variable. This has not been done automatically.

To configure your current shell, you need to source
the corresponding `env` file under {cargo_home}.

This is usually done by running one of the following (note the leading DOT):
{source_env_lines}"
}; }
