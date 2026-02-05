use std::{
    collections::{BTreeMap, HashSet},
    env,
    fs::{self},
    future::Future,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, error, info, warn};
use tokio::process::Command;

use crate::{
    build::{Effects, ERRCODE, STDERR, STDOUT},
    buildrs_wrapper::rewrite_main,
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    ext::CommandExt,
    green::Green,
    logging::{self, maybe_log},
    md::{BuildContext, Md, MountExtern, NamedMount},
    pwd, relative,
    runner::Runner,
    rustc_arguments::{as_rustc, RustcArgs},
    stage::{AsStage, Stage, RST, RUST},
    PKG, VSN,
};

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

#[macro_export]
macro_rules! ENV {
    () => {
        "CARGOGREEN"
    };
}

pub(crate) async fn main(
    green: Green,
    arg0: Option<String>,
    args: Vec<String>,
    vars: BTreeMap<String, String>,
) -> Result<()> {
    let argz = args.iter().take(3).map(AsRef::as_ref).collect::<Vec<_>>();

    let argv = |times| args.clone().into_iter().skip(times).collect();

    // TODO: find a better heuristic to ensure `rustc` is rustc
    match &argz[..] {
        [rustc, "--crate-name", crate_name, ..] if rustc.ends_with("rustc") =>
             wrap_rustc(green, crate_name, argv(1), call_rustc(rustc, argv(1))).await,
        [driver, rustc, "-"|"--crate-name", ..] if rustc.ends_with("rustc") => {
            // TODO: wrap driver? + rustc
            // driver: e.g. $RUSTUP_HOME/toolchains/stable-x86_64-unknown-linux-gnu/bin/clippy-driver
            // cf. https://github.com/rust-lang/rust-clippy/tree/da27c979e29e78362b7a2a91ebcf605cb01da94c#using-clippy-driver
             call_rustc(driver, argv(2)).await
         }
        [_driver, rustc, ..] if rustc.ends_with("rustc") =>
             call_rustc(rustc, argv(2)).await,
        [rustc, ..] if rustc.ends_with("rustc") =>
             call_rustc(rustc, argv(1)).await,
        _ => panic!("RUSTC_WRAPPER={arg0:?}'s input unexpected:\n\targz = {argz:?}\n\targs = {args:?}\n\tenvs = {vars:?}\n"),
    }
}

#[test]
fn passthrough_getting_rust_target_specific_information() {
    #[rustfmt::skip]
    let first_few_args = &[
        "$PWD/rustcbuildx/rustcbuildx",
        "$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc",
        "-",
        "--crate-name", "___",
        "--print=file-names",
        "--crate-type", "bin",
        "--crate-type", "rlib",
        "--crate-type", "dylib",
        "--crate-type", "cdylib",
        "--crate-type", "staticlib",
        "--crate-type", "proc-macro",
        "--print=sysroot",
        "--print=split-debuginfo",
        "--print=crate-name",
        "--print=cfg",
    ]
    .into_iter()
    .take(4)
    .map(ToOwned::to_owned)
    .collect::<Vec<String>>();

    let first_few_args =
        first_few_args.iter().skip(1).take(3).map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        match &first_few_args[..] {
            [_rustc, "-", ..] | [_rustc, _ /*driver*/, "-", ..] => 1,
            [_rustc, "--crate-name", _crate_name, ..] => 2,
            _ => 3,
        },
        1
    );
}

/// NOTE: not running inside Docker: local install SHOULD match Docker image setup
/// Meaning: it's up to the user to craft their desired $CARGOGREEN_BASE_IMAGE
async fn call_rustc(rustc: &str, args: Vec<String>) -> Result<()> {
    let mut cmd = Command::new(rustc);
    let cmd = cmd.kill_on_drop(true).args(args);
    let status = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn {}: {e}", cmd.show()))?
        .wait()
        .await
        .map_err(|e| anyhow!("Failed to wait {}: {e}", cmd.show()))?;
    if !status.success() {
        bail!("Failed in call_rustc")
    }
    Ok(())
}

async fn wrap_rustc(
    green: Green,
    crate_name: &str,
    arguments: Vec<String>,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    assert!(env::var_os(ENV!()).is_none(), "It's turtles all the way down!");
    env::set_var(ENV!(), "1");

    let pwd = pwd();

    let out_dir_var = env::var("OUT_DIR").ok().map(Utf8PathBuf::from);

    let (st @ RustcArgs { mdid, .. }, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;

    let buildrs = ["build_script_build", "build_script_main"].contains(&crate_name);
    // NOTE: krate_name != crate_name: Gets named build_script_build + s/-/_/g + may actually be a different name
    let krate_name = env::var("CARGO_PKG_NAME").expect("$CARGO_PKG_NAME");

    let krate_version = env::var("CARGO_PKG_VERSION").expect("$CARGO_PKG_VERSION");

    let krate_manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    let krate_manifest_dir = Utf8Path::new(&krate_manifest_dir);

    let full_krate_id = {
        // X: for building build scripts
        let kind = if buildrs { 'X' } else { 'N' }; // exe or normal
        format!("{kind} {krate_name} {krate_version} {mdid}")
    };

    logging::setup(&full_krate_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

    do_wrap_rustc(
        green,
        &krate_name,
        krate_manifest_dir,
        buildrs,
        Stage::dep(&full_krate_id.replace(' ', "-"))?,
        pwd,
        args,
        out_dir_var,
        st,
        fallback,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("corrupted $CARGO_HOME path: {e}"))
}

fn git_mount(cargo_home: &Utf8Path, path: &Utf8Path) -> Option<Utf8PathBuf> {
    if path.starts_with(cargo_home.join("git/checkouts")) {
        return Some(path.components().take(cargo_home.components().count() + 2 + 2).collect());
    }
    None
}

#[test]
fn gitmount() {
    assert_eq!(
        Some("/home/pete/.cargo/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into()),
        git_mount(
            "/home/pete/.cargo".into(),
            "/home/pete/.cargo/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into()
        )
    );
    assert_eq!(
        Some("$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2".into()),
        git_mount(
            "$CARGO_HOME".into(),
            "$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/blip/blop".into()
        )
    );
}

// WORKDIR /home/pete/.cargo/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2
// T 26/02/05 23:31:35.322 N simple 0.1.0 0000000000000000 ❯         CARGO_MANIFEST_DIR=/home/pete/.cargo/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/examples/simple \
// T 26/02/05 23:31:35.322 N simple 0.1.0 0000000000000000 ❯         CARGO_MANIFEST_PATH=/home/pete/.cargo/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/examples/simple/Cargo.toml \
// simple/src/lib.rs

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    green: Green,
    krate_name: &str,
    krate_manifest_dir: &Utf8Path,
    buildrs: bool,
    rustc_stage: Stage,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { externs, mdid, incremental, input, out_dir, target_path }: RustcArgs,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let mut md: Md = mdid.into();
    md.push_block(&RUST, green.base.image_inline.clone().unwrap());

    fs::create_dir_all(&out_dir).map_err(|e| anyhow!("Failed to `mkdir -p {out_dir}`: {e}"))?;
    let incremental = green.incremental().then_some(incremental).flatten();
    if let Some(ref incremental) = incremental {
        fs::create_dir_all(incremental)
            .map_err(|e| anyhow!("Failed to `mkdir -p {incremental}`: {e}"))?;
    }

    let cargo_home = cargo_home()?;

    info!("picked {rustc_stage} for {input}");

    let mut rustc_block = format!("FROM {RST} AS {rustc_stage}\n");
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/sh", "-eux", "-c"]));
    rustc_block.push_str(&format!("WORKDIR {out_dir}\n"));
    if !pwd.starts_with(cargo_home.join("registry/src")) {
        // Essentially match the same-ish path that points to crates-io paths.
        // let workdir = git_mount(&cargo_home, &pwd).unwrap_or_else(|| pwd.clone());
        let workdir = &pwd;
        rustc_block.push_str(&format!("WORKDIR {workdir}\n"));
    }

    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let code_stage = if input.starts_with(cargo_home.join("registry/src")) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        cratesio::named_stage(krate_name, krate_manifest_dir).await?
    } else if krate_manifest_dir.starts_with(cargo_home.join("git/checkouts")) {
        // Input is of a git checked out dep

        let workdir = git_mount(&cargo_home, krate_manifest_dir).unwrap();
        checkouts::as_stage(&workdir, krate_manifest_dir).await?
    } else if input.is_relative() {
        // Input is local code

        relative::as_stage(mdid, &pwd).await?
    } else {
        bail!("BUG: unhandled input {input:?} ({krate_manifest_dir})")
    };
    md.push_stage(&code_stage);
    rustc_block.push_str("RUN \\\n");
    for (src, dst, swappity) in code_stage.mounts() {
        let name = code_stage.name();
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        let rw = if buildrs { ",rw" } else { "" }; //FIXME
        rustc_block.push_str(&format!("  --mount=from={name}{mount}{rw} \\\n"));
    }

    let input = rewrite_cratesio_index(&input);

    let incremental_stage = Stage::incremental(mdid)?;
    let out_stage = Stage::output(mdid)?;

    if let Some((name, uri)) = code_stage.context() {
        info!("loading {name:?}: {uri}");
        md.contexts = [BuildContext { name, uri }].into();
        info!("loading 1 build context");
    }

    let mds = md.assemble_build_dependencies(externs, out_dir_var, &target_path)?;
    for MountExtern { from, xtern } in md.externs() {
        let dst = target_path.join("deps").join(xtern);
        rustc_block.push_str(&format!("  --mount=from={from},dst={dst},source=/{xtern} \\\n"));
    }
    for NamedMount { name, mount } in &md.mounts {
        rustc_block.push_str(&format!("  --mount=from={name},dst={mount},source=/ \\\n"));
    }

    if buildrs {
        // TODO: this won't work with e.g. tokio-decorated main fns (async + decorator needs duplicating)
        // TODO: replace this Rust patching by simply shell patching ==> must work on macOS x-compiling for eg. Linux
        rustc_block.push_str(&rewrite_main(mdid, &input));
    }

    let args = args.into_iter().map(|arg| safeify(&arg).unwrap()).collect::<Vec<_>>().join(" ");
    md.run_block(
        &rustc_stage,
        &out_stage,
        &out_dir,
        format!("rustc {args} {input}"),
        &green.set_envs,
        buildrs,
        rustc_block,
    )?;

    if let Some(ref incremental) = incremental {
        let mut incremental_block = format!("FROM scratch AS {incremental_stage}\n");
        incremental_block.push_str(&format!("COPY --link --from={rustc_stage} {incremental} /\n"));
        md.push_block(&incremental_stage, incremental_block);
    }

    md.out_block(&out_stage, &rustc_stage, &out_dir, false);

    let containerfile_path = md.finalize(&green, &target_path, krate_name, &mds)?;

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    md.do_build(&green, fallback, &containerfile_path, &out_stage, &out_dir, &target_path).await?;

    if let Some(incremental) = incremental {
        if let (_, _, _, Err(e)) = green
            .build_out(&containerfile_path, &incremental_stage, &md.contexts, &incremental)
            .await
        {
            warn!("Error building incremental data: {e}");
            return Err(e);
        }
    }

    Ok(())
}

impl Md {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn run_block(
        &mut self,
        stage: &Stage,
        out_stage: &Stage,
        out_dir: &Utf8Path,
        call: String,
        green_set_envs: &[String],
        buildrs: bool,
        mut block: String,
    ) -> Result<()> {
        // Log a possible toolchain file contents (TODO: make per-crate base.image out of this)
        if false {
            block.push_str("    { cat ./rustc-toolchain{,.toml} 2>/dev/null || true ; } && \\\n");
        }

        block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));
        let mut set = HashSet::from(["CARGO".to_owned()]);
        for (var, val) in env::vars().filter_map(|kv| fmap_env(kv, buildrs)) {
            let val = safeify(&val)?;
            block.push_str(&format!("        {var}={val} \\\n"));
            set.insert(var.clone());
        }
        block.push_str(&format!("        {}=1 \\\n", ENV!()));

        for (var, val) in &self.set_envs {
            if set.contains(var) {
                continue;
            }
            warn!("setting rustc-env: ${var}={val:?}");
            let val = safeify(val)?;
            block.push_str(&format!("        {var}={val} \\\n"));
            set.insert(var.to_owned());
        }

        for var in green_set_envs {
            if set.contains(var) {
                continue;
            }
            if let Ok(val) = env::var(var) {
                warn!("passing ${var}={val:?} env through");
                let val = safeify(&val)?;
                block.push_str(&format!("        {var}={val} \\\n"));
                set.insert(var.to_owned());
            }
        }

        // TODO: keep only paths that we explicitly mount or copy
        if false {
            // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
            for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
                let Ok(val) = env::var(var) else { continue };
                if set.contains(var) {
                    continue;
                }
                debug!("system env set (skipped): ${var}={val:?}");
                if !val.is_empty() {
                    let val = safeify(&val)?;
                    block.push_str(&format!("#       {var}={val:?} \\\n"));
                }
            }
        }

        block.push_str(&format!("      {call} \\\n"));
        block.push_str(&format!("        1>          {out_dir}/{out_stage}-{STDOUT} \\\n"));
        block.push_str(&format!("        2>          {out_dir}/{out_stage}-{STDERR} \\\n"));
        block.push_str(&format!("        || echo $? >{out_dir}/{out_stage}-{ERRCODE}\\\n"));

        // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
        let pattern = if buildrs { "*".to_owned() } else { format!("*-{}*", self.this()) };
        block.push_str(&format!("  ; find {out_dir}/{pattern} -print0 | xargs -0 touch --no-dereference --date=@$SOURCE_DATE_EPOCH\n"));

        self.push_block(stage, block);
        Ok(())
    }

    /// TODO? in Dockerfile, when using outputs:
    /// => skip the COPY (--mount=from=out-08c4d63ed4366a99) use the stage directly
    //FIXME? unpub
    pub(crate) fn out_block(
        &mut self,
        stage: &Stage,
        prev: &Stage,
        out_dir: &Utf8Path,
        buildrs: bool,
    ) {
        let mut block = format!("FROM scratch AS {stage}\n");
        if buildrs {
            block.push_str(&format!("COPY --link --from={prev} {out_dir} /\n"));
        } else {
            let mdid = self.this();
            block.push_str(&format!("COPY --link --from={prev} {out_dir}/*-{mdid}* /\n"));
        }
        self.push_block(stage, block);
    }

    //FIXME? unpub
    pub(crate) async fn do_build(
        &mut self,
        green: &Green,
        fallback: impl Future<Output = Result<()>>,
        containerfile_path: &Utf8Path,
        stage: &Stage,
        out_dir: &Utf8Path,
        target_path: &Utf8Path,
    ) -> Result<()> {
        if green.runner == Runner::None {
            info!("Runner disabled, falling back...");
            return fallback.await;
        }

        let (call, envs, Effects { written, stdout, stderr, cargo_rustc_env }, built) =
            green.build_out(containerfile_path, stage, &self.contexts, out_dir).await;

        green
            .maybe_write_final_path(containerfile_path, &self.contexts, &call, &envs)
            .map_err(|e| anyhow!("Failed producing final path: {e}"))?;

        let md_path = self.this().path(target_path);

        if !written.is_empty()
            || !stdout.is_empty()
            || !stderr.is_empty()
            || !cargo_rustc_env.is_empty()
        {
            self.writes = written;
            self.stdout = stdout;
            self.stderr = stderr;
            // self.cargo_rustc_env = cargo_rustc_env;
            info!("re-opening (RW) crate's md {md_path}");
            self.write_to(&md_path)?;
        }

        let final_stage = format!(
            "FROM scratch\n{}\n",
            self.writes
                .iter()
                .filter_map(|f| f.file_name())
                .filter(|f| !f.ends_with(".d"))
                .filter(|f| f != &format!("{stage}-{STDOUT}"))
                .filter(|f| f != &format!("{stage}-{STDERR}"))
                .filter(|f| f != &format!("{stage}-{ERRCODE}"))
                .map(|f| (f, f.replace(&format!("-{}", self.this()), "")))
                .map(|(src, dst)| format!("COPY --link --from={stage} /{src} /{dst}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        green
            .maybe_append_to_final_path(&md_path, final_stage)
            .map_err(|e| anyhow!("Failed finishing final path: {e}"))?;

        if let Err(e) = built {
            if maybe_log().is_none() {
                warn!("Falling back due to {e}");
                // Bubble up actual error & outputs
                return fallback.await.inspect(|()| {
                    eprintln!("BUG: {PKG} should not have encountered this error: {e}")
                });
            }

            return Err(e);
        }

        Ok(())
    }
}

fn fmap_env((var, val): (String, String), buildrs: bool) -> Option<(String, String)> {
    let (pass, skip, only_buildrs) = pass_env(&var);
    if pass || (buildrs && only_buildrs) {
        if skip {
            debug!("not forwarding env: {var}={val}");
            return None;
        }
        if var == "CARGO_ENCODED_RUSTFLAGS" {
            let dec: Vec<_> = rustflags::from_env().collect();
            debug!("env is set: {var}={val} ({dec:?})");
        } else {
            debug!("env is set: {var}={val}");
        }
        let val = match var.as_str() {
            // "CARGO_PKG_DESCRIPTION" => "FIXME".to_owned(),
            "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" => {
                rewrite_cratesio_index(Utf8Path::new(&val)).to_string()
            }
            "TERM" => return None,
            "RUSTC" => "rustc".to_owned(), // Rewrite host rustc so the base_image one can be used
            // "CARGO_TARGET_DIR" | "CARGO_BUILD_TARGET_DIR" => {
            //     virtual_target_dir(Utf8Path::new(&val)).to_string()
            // }
            // // TODO: a constant $CARGO_TARGET_DIR possible solution is to wrap build script as it runs,
            // // ie. controlling all outputs. This should help: https://github.com/trailofbits/build-wrap/blob/d7f43b76e655e43755f68e28e9d729b4ed1dd115/src/wrapper.rs#L29
            // //(dbcc)=> Dirty typenum v1.12.0: stale, https://github.com/rust-lang/cargo/blob/7987d4bfe683267ba179b42af55891badde3ccbf/src/cargo/core/compiler/fingerprint/mod.rs#L2030
            // //=> /tmp/clis-dbcc_2-2-1/release/deps/typenum-32188cb0392f25b9.d
            // "OUT_DIR" => virtual_target_dir(Utf8Path::new(&val)).to_string(),
            "CARGO_TARGET_DIR" | "CARGO_BUILD_TARGET_DIR" => return None,
            "OUT_DIR" => val,
            _ => val,
        };
        return Some((var, val));
    }
    None
}

// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates
#[must_use]
fn pass_env(var: &str) -> (bool, bool, bool) {
    // Thanks https://github.com/cross-rs/cross/blob/44011c8854cb2eaac83b173cc323220ccdff18ea/src/docker/shared.rs#L969
    let passthrough = [
        "http_proxy",
        "TERM",
        "RUSTDOCFLAGS",
        "RUSTFLAGS",
        "BROWSER",
        "HTTPS_PROXY",
        "HTTP_TIMEOUT",
        "https_proxy",
        "QEMU_STRACE",
        // Not here but set in RUN script: CARGO, PATH, ...
        "OUT_DIR", // (Only set during compilation.)
    ];
    // TODO: vvv drop what can be dropped vvv
    let skiplist = [
        "CARGO_BUILD_JOBS",
        "CARGO_BUILD_RUSTC",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTDOC",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_HOME",      // TODO? drop
        "CARGO_MAKEFLAGS", // TODO: probably drop
        "CARGO_TARGET_DIR",
        "LD_LIBRARY_PATH", // TODO: probably drop
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ];
    let buildrs_only = [
        "DEBUG",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "OUT_DIR",
        "PROFILE",
        "RUSTC",
        "RUSTC_LINKER",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "RUSTDOC",
        "TARGET",
    ];
    (
        var.starts_with("CARGO_") || passthrough.contains(&var),
        skiplist.contains(&var),
        var.starts_with("DEP_") || buildrs_only.contains(&var),
    )
}

fn safeify(val: &str) -> Result<String> {
    String::from_utf8(shell_quote::Sh::quote_vec(val))
        .map_err(|e| anyhow!("Failed escaping env value {val:?}: {e}"))
        .map(|s| s.replace("\n", "\\\n"))
        .map(|s| if s == "''" { "".to_owned() } else { s })
}

#[test]
fn test_safeify() {
    assert_eq!(safeify("$VAR=val").unwrap(), r#"'$VAR=val'"#.to_owned());
    assert_eq!(
        safeify("the compiler's `proc_macro` API to.").unwrap(),
        r#"the' compiler'\'s' `proc_macro` API to.'"#.to_owned()
    );
    assert_eq!(
        safeify("$VAR=v\na\nl").unwrap(),
        r#"'$VAR=v\
a\
l'"#
        .to_owned()
    );
}
