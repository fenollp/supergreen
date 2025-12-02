use std::{
    collections::BTreeMap,
    env,
    fs::{self},
    future::Future,
    iter::once,
};

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use log::{debug, error, info, warn};
use tokio::process::Command;

use crate::{
    build::{Effects, ERRCODE, STDERR, STDOUT},
    checkouts,
    cratesio::{self, rewrite_cratesio_index},
    ext::CommandExt,
    green::Green,
    logging::{self, maybe_log},
    md::{BuildContext, Md, MdId, MountExtern},
    pwd,
    runner::Runner,
    rustc_arguments::{as_rustc, RustcArgs},
    stage::{Stage, RST, RUST},
    PKG, VSN,
};

// NOTE: this RUSTC_WRAPPER program only ever gets called by `cargo`, so we save
//       ourselves some trouble and assume std::path::{Path, PathBuf} are UTF-8.

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
        full_krate_id.replace(' ', "-"),
        pwd,
        args,
        out_dir_var,
        st,
        fallback,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

fn crate_out_dir(out_dir_var: Option<Utf8PathBuf>) -> Result<Option<Utf8PathBuf>> {
    let Some(crate_out) = out_dir_var else { return Ok(None) };
    assert_eq!(crate_out.file_name(), Some("out"), "BUG: unexpected $OUT_DIR={crate_out} format");

    info!("listing (RO) crate_out contents {crate_out}");
    let listing = fs::read_dir(&crate_out)
        .map_err(|e| anyhow!("Failed reading crate_out dir {crate_out}: {e}"))?;

    let count = listing
        .map_while(Result::ok)
        .inspect(|f| {
            info!(
                "metadata for {f:?}: {:?}",
                f.metadata().map(|fmd| format!(
                    "created:{c:?} accessed:{a:?} modified:{m:?}",
                    c = fmd.created(),
                    a = fmd.accessed(),
                    m = fmd.modified(),
                ))
            );
        })
        .count();

    // Dir empty => mount can be dropped
    if count == 0 {
        return Ok(None);
    }

    Ok(Some(crate_out))
}

fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("corrupted $CARGO_HOME path: {e}"))
}

#[expect(clippy::too_many_arguments)]
async fn do_wrap_rustc(
    green: Green,
    krate_name: &str,
    krate_manifest_dir: &Utf8Path,
    buildrs: bool,
    crate_id: String,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { externs, mdid, incremental, input, out_dir, target_path }: RustcArgs,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let crate_out = crate_out_dir(out_dir_var)?;

    let mut md: Md = mdid.into();
    md.push_block(&RUST, green.base.image_inline.clone().unwrap());

    fs::create_dir_all(&out_dir).map_err(|e| anyhow!("Failed to `mkdir -p {out_dir}`: {e}"))?;
    let incremental = green.incremental().then_some(incremental).flatten();
    if let Some(ref incremental) = incremental {
        fs::create_dir_all(incremental)
            .map_err(|e| anyhow!("Failed to `mkdir -p {incremental}`: {e}"))?;
    }

    let cargo_home = cargo_home()?;

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let input_mount = if input.starts_with(cargo_home.join("registry/src")) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        let (stage, src, dst, block) = cratesio::into_stage(krate_name, krate_manifest_dir).await?;
        md.push_block(&stage, block);

        Some((stage, Some(src), dst))
    } else if krate_manifest_dir.starts_with(cargo_home.join("git/checkouts")) {
        // Input is of a git checked out dep

        let (stage, dst, block) = checkouts::into_stage(krate_manifest_dir).await?;
        md.push_block(&stage, block);

        Some((stage, None, dst))
    } else if input.is_relative() {
        None // Input is local code
    } else {
        bail!("BUG: unhandled input {input:?} ({krate_manifest_dir})")
    };
    let rustc_stage = Stage::dep(&crate_id)?;
    info!("picked {rustc_stage} for {input}");
    let input = rewrite_cratesio_index(&input);

    let incremental_stage = Stage::incremental(mdid)?;
    let out_stage = Stage::output(mdid)?;

    let mut rustc_block = format!("FROM {RST} AS {rustc_stage}\n");
    rustc_block.push_str(&format!("SHELL {:?}\n", ["/bin/sh", "-eux", "-c"]));
    rustc_block.push_str(&format!("WORKDIR {out_dir}\n"));
    if !pwd.starts_with(cargo_home.join("registry/src")) {
        // Essentially match the same-ish path that points to crates-io paths.
        // Experiment showed that git-check'ed-out crates didn't like: // if !pwd.starts_with(&cargo_home) {
        rustc_block.push_str(&format!("WORKDIR {pwd}\n"));
    }

    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    let cwd = if let Some((name, src, dst)) = input_mount.as_ref() {
        rustc_block.push_str("RUN \\\n");
        let source = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        rustc_block.push_str(&format!("  --mount=from={name}{source},dst={dst} \\\n"));

        None
    } else {
        // NOTE: build contexts have to be directories, can't be files.
        //> failed to get build context path {$HOME/wefwefwef/supergreen.git/Cargo.lock <nil>}: not a directory

        let cwd_stage = Stage::local(mdid)?;

        info!("mounting {}files under {pwd}", if pwd.join(".git").is_dir() { "git " } else { "" });

        let (keep, lose): (Vec<_>, Vec<_>) = {
            let mut entries = fs::read_dir(&pwd)
                .map_err(|e| anyhow!("Failed reading dir {pwd:?}: {e}"))?
                .map(|entry| -> Result<_> {
                    let entry = entry?;
                    let fpath = entry.path();
                    let fpath: Utf8PathBuf = fpath
                        .try_into()
                        .map_err(|e| anyhow!("corrupted UTF-8 encoding with {entry:?}: {e}"))?;
                    let Some(fname) = fpath.file_name() else {
                        bail!("unexpected root (/) for {entry:?}")
                    };
                    Ok(fname.to_owned())
                })
                .collect::<Result<Vec<_>>>()?;
            entries.sort(); // deterministic iteration
            entries.into_iter().partition(|fname| {
                if fname == ".dockerignore" {
                    debug!("excluding {fname}");
                    return false;
                }
                if fname == ".git" && pwd.join(fname).is_dir() {
                    debug!("excluding {fname} dir");
                    return false; // Skip copying .git dir
                }
                if pwd.join(fname).join("CACHEDIR.TAG").exists() {
                    debug!("excluding {fname} dir");
                    return false; // Test for existence of ./target/CACHEDIR.TAG See https://bford.info/cachedir/
                }
                debug!("keeping {fname}");
                true
            })
        };

        rustc_block.push_str("RUN \\\n");
        for fname in keep {
            rustc_block.push_str(&format!(
                "  --mount=from={cwd_stage},dst={pwd}/{fname},source=/{fname} \\\n"
            ));
        }
        // TODO: do better than mounting depth=1 (exclude non-git files (.d file?))

        if !lose.is_empty() {
            let lose: String = lose
                .into_iter()
                .chain(once(".dockerignore".to_owned()))
                .map(|fname| format!("/{fname}\n"))
                .collect();
            fs::write(pwd.join(".dockerignore"), lose).unwrap();
            //FIXME: if exists: save + extend (then restore??) .dockerignore
            //TODO? add .gitignore in there?
            //TODO? exclude everything, only include `git ls-files`?
        }

        Some((cwd_stage, pwd.clone()))
    };

    if let Some(crate_out) = crate_out.as_deref() {
        let named = crate_out_name(crate_out);
        rustc_block.push_str(&format!("  --mount=from={named},dst={crate_out} \\\n"));
    }

    md.contexts = [cwd, crate_out.map(|crate_out| (crate_out_name(&crate_out), crate_out))]
        .into_iter()
        .flatten()
        .map(|(name, uri)| BuildContext { name, uri })
        .inspect(|BuildContext { name, uri }| info!("loading {name:?}: {uri}"))
        .collect();
    info!("loading {} build contexts", md.contexts.len());

    let mds = md.assemble_build_dependencies(externs, &target_path)?;
    for MountExtern { from, xtern } in md.externs() {
        let dst = target_path.join("deps").join(xtern);
        rustc_block.push_str(&format!("  --mount=from={from},dst={dst},source=/{xtern} \\\n"));
    }

    // Log a possible toolchain file contents (TODO: make per-crate base.image out of this)
    if false {
        rustc_block.push_str("    { cat ./rustc-toolchain{,.toml} 2>/dev/null || true ; } && \\\n");
    }

    rustc_block.push_str(&format!("    env CARGO={:?} \\\n", "$(which cargo)"));

    for (var, val) in env::vars().filter_map(|kv| fmap_env(kv, buildrs)) {
        let val = safeify(&val)?;
        rustc_block.push_str(&format!("        {var}={val} \\\n"));
    }
    rustc_block.push_str(&format!("        {}=1 \\\n", ENV!()));
    // => cargo upstream issue "pass env vars read/wrote by build script on call to rustc"
    // TODO whence https://github.com/rust-lang/cargo/issues/14444#issuecomment-2305891696
    for var in &green.set_envs {
        if let Ok(val) = env::var(var) {
            warn!("passing ${var}={val:?} env through");
            let val = safeify(&val)?;
            rustc_block.push_str(&format!("        {var}={val:?} \\\n"));
        }
    }
    // TODO: catch these cargo:rustc-env= to add to TOML+Dockerfile in extremis so downstream knows about these envs
    // 2025-04-05T09:42:41.5322589Z [typenum 1.12.0] cargo:rustc-env=TYPENUM_BUILD_CONSTS=/home/runner/instst/release/build/typenum-3cf9e442dfddd505/out/consts.rs
    // 2025-04-05T09:42:41.5748814Z [typenum 1.12.0] cargo:rustc-env=TYPENUM_BUILD_OP=/home/runner/instst/release/build/typenum-3cf9e442dfddd505/out/op.rs
    // https://doc.rust-lang.org/cargo/reference/build-scripts.html#outputs-of-the-build-script
    // https://github.com/ALinuxPerson/build_script?tab=readme-ov-file#examples
    // TODO: also maybe same for "rustc wrote"?

    // TODO: keep only paths that we explicitly mount or copy
    if false {
        // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
        for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
            let Ok(val) = env::var(var) else { continue };
            debug!("system env set (skipped): ${var}={val:?}");
            if !val.is_empty() {
                let val = safeify(&val)?;
                rustc_block.push_str(&format!("#       {var}={val:?} \\\n"));
            }
        }
    }

    rustc_block.push_str(&format!("      rustc '{}' {input} \\\n", args.join("' '")));
    rustc_block.push_str(&format!("        1>          {out_dir}/{out_stage}-{STDOUT} \\\n"));
    rustc_block.push_str(&format!("        2>          {out_dir}/{out_stage}-{STDERR} \\\n"));
    rustc_block.push_str(&format!("        || echo $? >{out_dir}/{out_stage}-{ERRCODE}\\\n"));
    // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
    rustc_block.push_str(&format!("  ; find {out_dir}/*-{mdid}* -print0 | xargs -0 touch --no-dereference --date=@$SOURCE_DATE_EPOCH\n"));
    md.push_block(&rustc_stage, rustc_block);

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
    /// TODO? in Dockerfile, when using outputs:
    /// => skip the COPY (--mount=from=out-08c4d63ed4366a99) use the stage directly
    fn out_block(&mut self, stage: &Stage, prev: &Stage, out_dir: &Utf8Path, flag: bool) {
        let mut block = format!("FROM scratch AS {stage}\n");
        if flag {
            block.push_str(&format!("COPY --link --from={prev} {out_dir}/* /\n"));
        } else {
            let mdid = self.this();
            block.push_str(&format!("COPY --link --from={prev} {out_dir}/*-{mdid}* /\n"));
        }
        self.push_block(stage, block);
    }

    async fn do_build(
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

        let (call, envs, Effects { written, stdout, stderr }, built) =
            green.build_out(containerfile_path, stage, &self.contexts, out_dir).await;

        green
            .maybe_write_final_path(containerfile_path, &self.contexts, &call, &envs)
            .map_err(|e| anyhow!("Failed producing final path: {e}"))?;

        let md_path = self.this().path(target_path);

        if !written.is_empty() || !stdout.is_empty() || !stderr.is_empty() {
            self.writes = written;
            self.stdout = stdout;
            self.stderr = stderr;
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
            "CARGO_MANIFEST_DIR" | "CARGO_MANIFEST_PATH" => {
                rewrite_cratesio_index(Utf8Path::new(&val)).to_string()
            }
            "TERM" => return None,
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

#[test]
fn crate_out_name_for_some_pkg() {
    let crate_out = Utf8Path::new("/home/maison/target/debug/build/quote-adce79444856d618/out");
    let res = crate_out_name(crate_out);
    assert_eq!(res, "crate_out-adce79444856d618".try_into().unwrap());
}

#[must_use]
fn crate_out_name(name: &Utf8Path) -> Stage {
    name.parent()
        .and_then(|x| x.file_name())
        .and_then(|x| x.rsplit_once('-'))
        .map(|(_, x)| Stage::crate_out(MdId::new(&format!("-{x}"))))
        .expect("PROOF: suffix is /out")
        .expect("PROOF: out dir path format")
}
