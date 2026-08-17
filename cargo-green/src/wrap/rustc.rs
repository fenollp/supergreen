use std::future::Future;

use anyhow::{Result, anyhow, bail};
use camino::{Utf8Path, Utf8PathBuf};
use log::{error, info, warn};

use crate::{
    PKG, VSN, checkouts,
    cratesio::{self},
    dirs::locate_path,
    green::Green,
    logging::{self},
    md::{BuildContext, Md, NamedMount},
    relative,
    rustc_arguments::{RustcArgs, as_rustc},
    stage::{AsStage, RST, RUST, Stage},
    sys::sys,
    wrap::{Vars, build_script::is_buildrs_executable, call_config, envs::safeify},
};

pub(crate) async fn wrap_rustc(
    green: Green,
    arguments: Vec<String>,
    vars: &Vars,
    pwd: Utf8PathBuf,
    fallback: impl Future<Output = Result<()>>,
) -> Result<()> {
    let out_dir_var = vars.get(OUT_DIR!()).map(Utf8PathBuf::from);

    let (st @ RustcArgs { mdid, .. }, args) = as_rustc(&pwd, &arguments, out_dir_var.as_deref())?;
    let mdid = mdid.expect("mdid set");

    let (crate_name, pkg_name, pkg_version, pkg_manifest_dir) = call_config(vars);

    let buildrs = crate_name.as_deref().map(is_buildrs_executable).unwrap_or_default();
    let kind = if buildrs { 'X' } else { 'N' }; // building buildrs eXe or Normal
    let full_pkg_id = format!("{kind} {pkg_name} {pkg_version} {mdid}");

    logging::setup(&full_pkg_id);

    info!("{PKG}@{VSN} original args: {arguments:?} pwd={pwd} st={st:?} green={green:?}");

    if green.runner.is_none() {
        if green.paths.reuse_out(&Stage::output(mdid)?, &st.out_dir).await? {
            return Ok(());
        }
        return fallback.await;
    }

    do_wrap_rustc(
        green,
        crate_name.as_deref(),
        &pkg_name,
        &pkg_manifest_dir,
        Stage::dep(&full_pkg_id.replace(' ', "-"))?,
        vars,
        pwd,
        args,
        out_dir_var,
        st,
    )
    .await
    .inspect_err(|e| error!("Error: {e}"))
}

#[expect(clippy::too_many_arguments)]
pub(crate) async fn do_wrap_rustc(
    green: Green,
    crate_name: Option<&str>,
    pkg_name: &str,
    pkg_manifest_dir: &Utf8Path,
    rustc_stage: Stage,
    vars: &Vars,
    pwd: Utf8PathBuf,
    args: Vec<String>,
    out_dir_var: Option<Utf8PathBuf>,
    RustcArgs { externs, mdid, incremental, input, out_dir, target_path }: RustcArgs,
) -> Result<()> {
    let mdid = mdid.expect("mdid set");
    let mut md: Md = mdid.into();

    md.buildrs = crate_name.map(is_buildrs_executable).unwrap_or_default();
    md.push_block(&RUST, &green.base.image_inline);

    let fs = sys().fs;
    fs.create_dir_all(&out_dir).map_err(|e| anyhow!("Failed to `mkdir -p {out_dir}`: {e}"))?;
    let incremental = green.incremental().then_some(incremental).flatten();
    if let Some(ref incremental) = incremental {
        fs.create_dir_all(incremental)
            .map_err(|e| anyhow!("Failed to `mkdir -p {incremental}`: {e}"))?;
    }

    info!("picked {rustc_stage} for {input}");

    let mut rustc_block = format!("FROM {RST} AS {rustc_stage}\n");

    rustc_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite_target_dir(&out_dir)));
    let not_a_cratesio_crate = !green.paths.is_cratesio(&pwd);
    if not_a_cratesio_crate {
        rustc_block.push_str(&format!("WORKDIR {}\n", green.paths.rewrite(&pwd)));
    }
    if let Some(ref incremental) = incremental {
        rustc_block.push_str(&format!("WORKDIR {incremental}\n"));
    }

    // TODO: support non-crates.io crates managers + proxies
    // TODO: use --secret mounts for private deps (and secret direct artifacts)
    let mut code_stage = if green.paths.is_cratesio(&input) {
        // Input is of a crate dep (hosted at crates.io)
        // Let's optimize this case by fetching & caching crate tarball

        cratesio::named_stage(&green.paths, pkg_name, pkg_manifest_dir).await?
    } else if green.paths.is_checkout(pkg_manifest_dir) {
        // Input is of a git checked out dep

        checkouts::as_stage(&green.paths, pkg_manifest_dir).await?
    } else if input.is_relative() {
        // Input is local code

        relative::as_stage(mdid, &pwd).await?
    } else {
        bail!("BUG: unhandled input {input:?} ({pkg_manifest_dir})")
    };
    md.push_stage(&code_stage);
    rustc_block.push_str("RUN \\\n");
    for (src, dst, swappity) in code_stage.mounts() {
        let name = code_stage.name();
        let dst = green.paths.rewrite(&dst);
        let src = src.as_deref().map(|src| format!(",source={src}")).unwrap_or_default();
        let mount = if swappity { format!(",dst={dst}{src}") } else { format!("{src},dst={dst}") };
        rustc_block.push_str(&format!("  --mount=from={name}{mount} \\\n"));
    }

    if let Some((name, uri)) = code_stage.context() {
        info!("loading {name:?}: {uri}");
        md.contexts = [BuildContext { name, uri }].into();
        info!("loading 1 build context");
    }

    let mds = md.assemble_build_dependencies(
        &mut green.paths.new_mds_cache(&target_path),
        externs,
        out_dir_var.map(|out_dir| green.paths.rewrite_target_dir(&out_dir)),
    )?;
    for NamedMount { name, mount } in md.externs() {
        let located = locate_path(
            |path| path.join("deps").join(mount),
            &target_path,
            green.paths.host_profile_dir(&target_path).as_deref(),
        );
        let dst = green.paths.rewrite_target_dir(&located);
        rustc_block.push_str(&format!("  --mount=from={name},dst={dst},source=/deps/{mount} \\\n"));
    }
    for NamedMount { name, mount } in &md.mounts {
        let base = mount.file_name().expect("PROOF: OUT_DIR mounts end in /out");
        rustc_block.push_str(&format!("  --mount=from={name},dst={mount},source=/{base} \\\n"));
    }

    let out_stage = Stage::output(mdid)?;

    let call = {
        let input = green.paths.rewrite_str(input.as_str());

        let args = args
            .into_iter()
            .map(|ref x| green.paths.rewrite_target_dir_str(x))
            .map(|arg| safeify(&arg).unwrap())
            .collect::<Vec<_>>()
            .join(" ");

        format!("rustc {args} {input}")
    };
    md.call_block(
        (&rustc_stage, rustc_block),
        crate_name,
        &green.paths,
        &green.set_envs,
        vars,
        &call,
        (&out_stage, not_a_cratesio_crate.then_some(&out_dir)),
    )?;

    let incremental_stage = Stage::incremental(mdid)?;
    if let Some(ref incremental) = incremental {
        let mut incremental_block = format!("FROM scratch AS {incremental_stage}\n");
        incremental_block.push_str(&format!("COPY --link --from={rustc_stage} {incremental} /\n"));
        md.push_block(&incremental_stage, &incremental_block);
    }

    md.out_block(&out_stage, &rustc_stage, &green.paths, &out_dir);

    let (md_path, containerfile_path) = md.finalize(&green, &target_path, pkg_name, &mds)?;

    // TODO: use tracing instead:
    // https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/struct.Subscriber.html
    // https://crates.io/crates/tracing-appender
    // https://github.com/tugglecore/rust-tracing-primer
    // TODO: `cargo green -v{N+1} ..` starts a TUI showing colored logs on above `cargo -v{N} ..`

    md.do_build(&green, &md_path, &containerfile_path, &out_stage, &out_dir).await?;

    if let Some(incremental) = incremental
        && let (_, _, _, _, Err(e)) = green
            .build_out(&containerfile_path, &incremental_stage, &md.contexts, &incremental)
            .await
    {
        warn!("Error building incremental data: {e}");
        return Err(e);
    }

    drop(code_stage); // Some impl cleans up files

    Ok(())
}

/// The whole wrapping pipeline, from rustc's argv to the Containerfile handed to
/// BuildKit, with every side effect faked.
///
/// This is what the `recipes/` corpus was for, at a size you can read: the crate under
/// test is local (so its source is a build context), it has one crates.io dependency
/// (so a dep stage gets mounted in), and the environment is fixed rather than ambient.
#[cfg(test)]
mod pipeline {
    use std::sync::Arc;

    use snapbox::str;

    use super::{Green, Vars, wrap_rustc};
    use crate::{
        base_image::BaseImage,
        build::Effects,
        dirs::Paths,
        r#final::Final,
        runner::Runner,
        sys::{
            Sys,
            fake::{FakeBuilds, FakeFs},
            install,
        },
        testing::assert_containerfile_eq,
    };

    const MDID: &str = "3333333333333333";
    const DEP: &str = "1111111111111111";
    const RUST_BLOCK: &str = "FROM docker.io/library/rust:1.99.0-slim AS rust-base";

    /// The Md a previously-built `libc` would have left in the target dir.
    const DEP_MD: &str = r#"
stamp = 1
this = "1111111111111111"
writes = ["liblibc-1111111111111111.rlib"]

[[stages]]

[stages.Script]
stage = "rust-base"
script = "FROM docker.io/library/rust:1.99.0-slim AS rust-base"

[[stages]]

[stages.Script]
stage = "cratesio-libc-0.2.177"
script = """
FROM scratch AS cratesio-libc-0.2.177
ADD --chmod=0664 --unpack --checksum=sha256:0000000000000000000000000000000000000000000000000000000000000000 \\
  https://static.crates.io/crates/libc/libc-0.2.177.crate /"""

[[stages]]

[stages.Script]
stage = "dep-n-libc-0.2.177-1111111111111111"
script = """
FROM rust-base AS dep-n-libc-0.2.177-1111111111111111
WORKDIR /target/debug/deps
RUN rustc --crate-name libc --edition=2021 $CARGO_HOME/registry/src/index.crates.io/libc-0.2.177/src/lib.rs"""

[[stages]]

[stages.Script]
stage = "out-1111111111111111"
script = """
FROM scratch AS out-1111111111111111
COPY --link --from=dep-n-libc-0.2.177-1111111111111111 /target/debug/deps /deps"""
"#;

    /// Just enough of what cargo sets, plus two that must NOT reach the container.
    fn vars() -> Vars {
        [
            ("CARGO_CRATE_NAME", "mycrate"),
            ("CARGO_MANIFEST_DIR", "/work"),
            ("CARGO_PKG_NAME", "mycrate"),
            ("CARGO_PKG_VERSION", "0.1.0"),
            ("CARGO_PKG_AUTHORS", "Someone <s@example.com>"),
            ("CARGO_HOME", "/home/u/.cargo"),
            ("HOME", "/home/u"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn green() -> Green {
        Green {
            runner: Runner::Docker,
            base: BaseImage { image_inline: RUST_BLOCK.to_owned(), ..Default::default() },
            r#final: Final { path: Some("/work/recipe.Dockerfile".into()) },
            experiment: vec!["finalpathnonprimary".to_owned()],
            paths: Paths {
                cargo_home: "/home/u/.cargo".into(),
                cwd: "/work".into(),
                host_target_dir: Some("/work/target".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[rustfmt::skip]
    fn arguments() -> Vec<String> {
        // As cargo invokes us, minus the leading rustc path that `wrap::rustc` strips.
        [
            "--crate-name", "mycrate",
            "--edition=2024",
            "src/main.rs",
            "--error-format=json",
            "--crate-type", "bin",
            "--emit=dep-info,link",
            "-C", "embed-bitcode=no",
            "-C", "debuginfo=2",
            "-C", &format!("metadata={MDID}"),
            "-C", &format!("extra-filename=-{MDID}"),
            "--out-dir", "/work/target/debug/deps",
            "-L", "dependency=/work/target/debug/deps",
            "--extern", &format!("libc=/work/target/debug/deps/liblibc-{DEP}.rlib"),
        ]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
    }

    /// Runs the pipeline, returning the generated Containerfile and the recipe.
    fn run() -> (String, String) {
        let fs = Arc::new(FakeFs::default());
        fs.file("/work/Cargo.toml", "[package]\nname = \"mycrate\"\n");
        fs.file("/work/Cargo.lock", "version = 4\n");
        fs.file("/work/src/main.rs", "fn main() {}\n");
        fs.mkdir("/work/.git");
        fs.file("/work/target/CACHEDIR.TAG", "Signature: 8a477f597d28d172\n");
        fs.file(format!("/work/target/debug/{DEP}.toml"), &DEP_MD[1..]);
        let builds = Arc::new(FakeBuilds {
            // What the runner reports rustc produced, so the recipe gets a final stage.
            effects: Effects {
                written: vec![format!("mycrate-{MDID}").into(), format!("mycrate-{MDID}.d").into()],
                ..Effects::default()
            },
            ..FakeBuilds::default()
        });
        let _guard = install(Sys {
            fs: Arc::clone(&fs) as _,
            builds: Arc::clone(&builds) as _,
            ..Sys::fake()
        });

        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(wrap_rustc(green(), arguments(), &vars(), "/work".into(), async {
                unreachable!("the runner is not none, so there is no fallback to rustc")
            }))
            .unwrap();

        let containerfile = format!("/work/target/debug/mycrate-{MDID}.Dockerfile");
        assert_eq!(builds.built(), [containerfile.as_str()]);
        (fs.read(&containerfile).unwrap(), fs.read("/work/recipe.Dockerfile").unwrap())
    }

    #[test]
    fn a_local_crate_with_one_dependency() {
        assert_containerfile_eq!(
            run().0,
            str![[r#"
# syntax=docker.io/docker/dockerfile:1
# check=error=true
# Generated by https://github.com/fenollp/supergreen v0.27.0

FROM docker.io/library/rust:1.99.0-slim AS rust-base
ARG SOURCE_DATE_EPOCH=42


FROM scratch AS cratesio-libc-0.2.177
ADD --chmod=0664 --unpack --checksum=sha256:0000000000000000000000000000000000000000000000000000000000000000 \
  https://static.crates.io/crates/libc/libc-0.2.177.crate /
FROM rust-base AS dep-n-libc-0.2.177-1111111111111111
WORKDIR /target/debug/deps
RUN rustc --crate-name libc --edition=2021 $CARGO_HOME/registry/src/index.crates.io/libc-0.2.177/src/lib.rs
FROM scratch AS out-1111111111111111
COPY --link --from=dep-n-libc-0.2.177-1111111111111111 /target/debug/deps /deps

FROM rust-base AS dep-n-mycrate-0.1.0-3333333333333333
WORKDIR /target/debug/deps
WORKDIR /work
RUN \
  --mount=from=cwd-3333333333333333,dst=/work/Cargo.lock,source=/Cargo.lock \
  --mount=from=cwd-3333333333333333,dst=/work/Cargo.toml,source=/Cargo.toml \
  --mount=from=cwd-3333333333333333,dst=/work/src,source=/src \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/liblibc-1111111111111111.rlib,source=/deps/liblibc-1111111111111111.rlib \
    env CARGO_CRATE_NAME=mycrate \
        CARGO_MANIFEST_DIR=/work \
        CARGO_PKG_AUTHORS=Someone' <s@example.com>' \
        CARGO_PKG_NAME=mycrate \
        CARGO_PKG_VERSION=0.1.0 \
        CARGOGREEN=1 \
      rustc --crate-name mycrate --crate-type bin --edition 2024 --emit dep-info,link --error-format json --extern libc'=/target/debug/deps/liblibc-1111111111111111.rlib' --out-dir /target/debug/deps -C debuginfo'=2' -C embed-bitcode'=no' -C extra-filename'=-3333333333333333' -C metadata'=3333333333333333' -L dependency'=/target/debug/deps' src/main.rs \
        1>          /target/debug/out-3333333333333333-stdout \
        2>          /target/debug/out-3333333333333333-stderr \
        || echo $? >/target/debug/out-3333333333333333-errcode\
  ; find /target/debug/deps/ /target/debug/out-3333333333333333-* -name '*-3333333333333333*' -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/debug/out-3333333333333333-errcode
FROM scratch AS out-3333333333333333
COPY --link --from=dep-n-mycrate-0.1.0-3333333333333333 /target/debug/deps /deps
COPY --link --from=dep-n-mycrate-0.1.0-3333333333333333 /target/debug/out-3333333333333333-* /

"#]]
        );
    }

    /// The crate's own source is mounted from a build context, never `COPY`ed in, and
    /// the dep arrives as a mount from its output stage rather than a rebuild.
    #[test]
    fn the_recipe_is_the_containerfile_plus_the_artifacts() {
        assert_containerfile_eq!(
            run().1,
            str![[r##"
# syntax=docker.io/docker/dockerfile:1
# check=error=true
# Generated by https://github.com/fenollp/supergreen v0.27.0

FROM docker.io/library/rust:1.99.0-slim AS rust-base
ARG SOURCE_DATE_EPOCH=42


FROM scratch AS cratesio-libc-0.2.177
ADD --chmod=0664 --unpack --checksum=sha256:0000000000000000000000000000000000000000000000000000000000000000 \
  https://static.crates.io/crates/libc/libc-0.2.177.crate /
FROM rust-base AS dep-n-libc-0.2.177-1111111111111111
WORKDIR /target/debug/deps
RUN rustc --crate-name libc --edition=2021 $CARGO_HOME/registry/src/index.crates.io/libc-0.2.177/src/lib.rs
FROM scratch AS out-1111111111111111
COPY --link --from=dep-n-libc-0.2.177-1111111111111111 /target/debug/deps /deps

FROM rust-base AS dep-n-mycrate-0.1.0-3333333333333333
WORKDIR /target/debug/deps
WORKDIR /work
RUN \
  --mount=from=cwd-3333333333333333,dst=/work/Cargo.lock,source=/Cargo.lock \
  --mount=from=cwd-3333333333333333,dst=/work/Cargo.toml,source=/Cargo.toml \
  --mount=from=cwd-3333333333333333,dst=/work/src,source=/src \
  --mount=from=out-1111111111111111,dst=/target/debug/deps/liblibc-1111111111111111.rlib,source=/deps/liblibc-1111111111111111.rlib \
    env CARGO_CRATE_NAME=mycrate \
        CARGO_MANIFEST_DIR=/work \
        CARGO_PKG_AUTHORS=Someone' <s@example.com>' \
        CARGO_PKG_NAME=mycrate \
        CARGO_PKG_VERSION=0.1.0 \
        CARGOGREEN=1 \
      rustc --crate-name mycrate --crate-type bin --edition 2024 --emit dep-info,link --error-format json --extern libc'=/target/debug/deps/liblibc-1111111111111111.rlib' --out-dir /target/debug/deps -C debuginfo'=2' -C embed-bitcode'=no' -C extra-filename'=-3333333333333333' -C metadata'=3333333333333333' -L dependency'=/target/debug/deps' src/main.rs \
        1>          /target/debug/out-3333333333333333-stdout \
        2>          /target/debug/out-3333333333333333-stderr \
        || echo $? >/target/debug/out-3333333333333333-errcode\
  ; find /target/debug/deps/ /target/debug/out-3333333333333333-* -name '*-3333333333333333*' -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{}' + \
 || echo $? >/target/debug/out-3333333333333333-errcode
FROM scratch AS out-3333333333333333
COPY --link --from=dep-n-mycrate-0.1.0-3333333333333333 /target/debug/deps /deps
COPY --link --from=dep-n-mycrate-0.1.0-3333333333333333 /target/debug/out-3333333333333333-* /

# Pipe this file to (not portable due to usage of local build contexts):
# DOCKER_BUILDKIT="1" \
#   docker buildx build --target=out-3333333333333333 <THIS_FILE

FROM scratch
COPY --link --from=out-3333333333333333 /deps/mycrate-3333333333333333 /mycrate

"##]]
        );
    }
}
