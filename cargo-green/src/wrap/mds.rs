use std::collections::HashSet;

use anyhow::{Result, anyhow};
use camino::Utf8Path;
use log::{debug, info, warn};

use crate::{
    build::{Built, ERRCODE, Effects, STDERR, STDOUT},
    dirs::Paths,
    green::Green,
    md::Md,
    stage::Stage,
    wrap::{
        Vars,
        build_script::{exe_dance, is_buildrs_executable},
        envs::fmap_env,
    },
};

impl Md {
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn call_block(
        &mut self,
        (stage, mut block): (&Stage, String),
        crate_name: Option<&str>,
        paths: &Paths,
        locates_sources: bool,
        green_set_envs: &[String],
        env: &Vars,
        call: &str,
        (out_stage, out_dir): (&Stage, Option<&Utf8Path>),
    ) -> Result<()> {
        let mut first = true;
        let mut push = |block: &mut String, var: &str, val: &str| -> Result<_> {
            let val = paths.rewrite_env(val)?;
            block.push_str(&format!("    {} {var}={val} \\\n", if first { "env" } else { "   " }));
            first = false;
            Ok(())
        };

        let mut set: HashSet<_> = [CARGO!(), "RUSTC", RUSTUP_TOOLCHAIN!()].into();

        for (k, v) in env {
            let Some((k, v)) = fmap_env((k.as_str(), v.as_str()), self.buildrs) else { continue };
            let false = set.contains(k) else { continue };
            push(&mut block, k, v)?;
            set.insert(k);
        }
        block.push_str(&format!("        {}=1 \\\n", CARGOGREEN!()));

        // Crates for testing (eg. `snapbox`) rely on $CARGO_RUSTC_CURRENT_DIR
        // (cargo support discontinued since https://github.com/rust-lang/cargo/pull/14799)
        // to locate sources at runtime and we remapped local sources under `VIRTUAL_CWD`
        if locates_sources && !set.contains(CARGO_RUSTC_CURRENT_DIR!()) {
            push(&mut block, CARGO_RUSTC_CURRENT_DIR!(), paths.cwd.as_str())?;
            set.insert(CARGO_RUSTC_CURRENT_DIR!());
        }

        for (var, val) in &self.set_envs {
            let false = set.contains(var.as_str()) else { continue };
            warn!("setting rustc-env: ${var}={val:?}");
            push(&mut block, var, val)?;
            set.insert(var);
        }

        for var in green_set_envs {
            let false = set.contains(var.as_str()) else { continue };
            if let Some(val) = env.get(var.as_str()) {
                warn!("passing ${var}={val:?} env through");
                push(&mut block, var, val)?;
                set.insert(var);
            }
        }

        // TODO: keep only paths that we explicitly mount or copy
        if false {
            // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
            for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
                let Some(val) = env.get(var) else { continue };
                let false = set.contains(var) else { continue };
                debug!("system env set (skipped): ${var}={val:?}");
                push(&mut block, var, val)?;
            }
        }

        let out_dir = out_dir.map(|d| paths.rewrite_target_dir(d)).unwrap_or(".".into());
        // TODO: let out_dir = out_dir.map(|_| "$OLDPWD").unwrap_or("$PWD"); whence  https://github.com/moby/buildkit/issues/6698  [frontend] $OLDPWD is unset (after >1 WORKDIR layers)
        let outdir_stdio = format!("{out_dir}/..")
            .replace("./..", "..")
            .replace("/out/..", "")
            .replace("/deps/..", "");

        block.push_str(&format!("      {call} \\\n"));
        block.push_str(&format!("        1>          {outdir_stdio}/{out_stage}-{STDOUT} \\\n"));
        block.push_str(&format!("        2>          {outdir_stdio}/{out_stage}-{STDERR} \\\n"));
        block.push_str(&format!("        || echo $? >{outdir_stdio}/{out_stage}-{ERRCODE}\\\n"));

        if let Some(crate_name) = crate_name
            && is_buildrs_executable(crate_name)
        {
            block.push_str(&exe_dance(self.this(), crate_name, &out_dir));
            block.push_str(&format!(" || echo $? >{outdir_stdio}/{out_stage}-{ERRCODE} \\\n"));
        }

        // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
        let pattern = if self.buildrs { "" } else { &format!(" -name '*-{}*'", self.this()) };
        block.push_str(&format!("  ; find {out_dir}/ {outdir_stdio}/{out_stage}-*{pattern} -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{{}}' + \\\n"));
        block.push_str(&format!(" || echo $? >{outdir_stdio}/{out_stage}-{ERRCODE}\n"));

        self.push_block(stage, &block);
        Ok(())
    }

    /// TODO? in Dockerfile, when using outputs:
    /// => skip the COPY (--mount=from=out-08c4d63ed4366a99) use the stage directly
    pub(crate) fn out_block(
        &mut self,
        stage: &Stage,
        prev: &Stage,
        paths: &Paths,
        out_dir: &Utf8Path,
    ) {
        let mut block = format!("FROM scratch AS {stage}\n");
        let out_dir = paths.rewrite_target_dir(out_dir);
        let base = out_dir.file_name().expect("PROOF: out_dir has a file name");
        block.push_str(&format!("COPY --link --from={prev} {out_dir} /{base}\n"));
        let up_out_dir = out_dir.parent().expect("PROOF: out_dir has parents");
        block.push_str(&format!("COPY --link --from={prev} {up_out_dir}/{stage}-* /\n"));
        self.push_block(stage, &block);
    }

    pub(crate) async fn do_build(
        &mut self,
        green: &Green,
        md_path: &Utf8Path,
        containerfile_path: &Utf8Path,
        stage: &Stage,
        out_dir: &Utf8Path,
    ) -> Result<()> {
        let Built {
            call,
            envs,
            effects: Effects { written, stdout, stderr, rustc_envs },
            result,
            built,
        } = green.build_out(containerfile_path, stage, &self.contexts, out_dir).await;

        green
            .maybe_write_final_path(containerfile_path, &self.contexts, &call, &envs)
            .map_err(|e| anyhow!("Failed producing final path: {e}"))?;

        let mut md_ser = None;
        if !written.is_empty() || !stdout.is_empty() || !stderr.is_empty() || !rustc_envs.is_empty()
        {
            self.writes = written;
            self.stdout = stdout;
            self.stderr = stderr;
            self.set_envs = rustc_envs;
            info!("re-opening (RW) crate's md {md_path}");
            md_ser = Some(self.write_to(md_path)?);
        }

        // Now that Md is ready for other processes to use, let's emit to cargo, finally.
        self.stdout.iter().for_each(|line| green.paths.fwd_stdout_to_cargo(line));
        self.stderr.iter().for_each(|line| green.paths.fwd_stderr_to_cargo(line));

        if let Some(result) = result {
            if built.is_ok() {
                if let Err(e) = async {
                    let md_ser = Ok(md_ser)
                        .transpose()
                        .unwrap_or_else(|| self.to_string_pretty())
                        .map_err(|e| anyhow!("Failed serializing Md {md_path}: {e}"))?;
                    result.finalize(&md_ser).await
                }
                .await
                {
                    warn!("unable to finish writing result: {e}");
                }
            } else {
                result.discard().await;
            }
        }

        let base = green.paths.rewrite_target_dir(out_dir);
        let base = base.file_name().expect("PROOF: out_dir has a file name");
        let final_stage = format!(
            "FROM scratch\n{}\n",
            self.writes
                .iter()
                .filter_map(|w| w.file_name().map(|f| (w, f)))
                .filter(|(_, f)| !f.ends_with(".d"))
                .filter(|(_, f)| !f.ends_with(".dwp")) // TODO? should we be dropping this
                .map(|(w, f)| (w, f.replace(&format!("-{}", self.this()), "")))
                .map(|(w, f)| (w, f.replace("_", "-"))) // cargo-install rewrites underscores
                .map(|(src, dst)| format!("COPY --link --from={stage} /{base}/{src} /{dst}"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        green
            .maybe_append_to_final_path(md_path, final_stage)
            .map_err(|e| anyhow!("Failed finishing final path: {e}"))?;

        built
    }
}

/// The tail of a build: what `$CARGOGREEN_FINAL_PATH` ends up holding once the runner
/// has reported which files the crate produced.
#[cfg(test)]
mod do_build {
    use super::{Green, Md, Stage};
    use crate::{
        containerfile::assert_containerfile_eq,
        dirs::Paths,
        r#final::Final,
        md::MdId,
        sys::{
            Sys,
            fake::{FakeBuilds, FakeFs},
        },
    };

    const CONTAINERFILE: &str = "/work/target/debug/mycrate-3333333333333333.Dockerfile";
    const MD: &str = "/work/target/debug/3333333333333333.toml";
    const FINAL: &str = "/work/recipe.Dockerfile";

    #[test]
    fn the_recipe_ends_with_the_crate_s_artifacts() {
        let mdid: MdId = 0x3333333333333333_u64.into();
        let stage = Stage::output(mdid).unwrap();

        let fs = FakeFs::new();
        fs.file(CONTAINERFILE, "FROM rust AS rust-base\nFROM rust-base AS dep-n-mycrate-0.1.0\n");
        let builds = FakeBuilds::wrote([
            // Kept
            "libmycrate-3333333333333333.rlib",
            "libmycrate-3333333333333333.rmeta",
            // cargo-install rewrites underscores
            "my_bin-3333333333333333",
            // Dropped
            "mycrate-3333333333333333.d",
            "my_bin-3333333333333333.dwp",
        ]);
        let _guard = Sys::install(Sys { fs: fs.clone(), builds: builds.clone(), ..Sys::fake() });

        let green = Green {
            r#final: Final { path: Some(FINAL.into()) },
            experiment: ["finalpathnonprimary".into()].into(),
            paths: Paths {
                cwd: "/work".into(),
                host_target_dir: Some("/work/target".into()),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut md: Md = mdid.into();
        md.push_block(&crate::stage::RUST, "FROM rust AS rust-base");

        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(md.do_build(
                &green,
                MD.into(),
                CONTAINERFILE.into(),
                &stage,
                "/work/target/debug/deps".into(),
            ))
            .unwrap();

        assert_eq!(builds.built(), [CONTAINERFILE]);

        assert_containerfile_eq!(
            fs.read(FINAL).unwrap(),
            snapbox::str![[r#"
FROM rust AS rust-base
FROM rust-base AS dep-n-mycrate-0.1.0

# Pipe this file to:
# DOCKER_BUILDKIT="1" \
#   docker buildx build --target=out-3333333333333333 <THIS_FILE

FROM scratch
COPY --link --from=out-3333333333333333 /deps/libmycrate-3333333333333333.rlib /libmycrate.rlib
COPY --link --from=out-3333333333333333 /deps/libmycrate-3333333333333333.rmeta /libmycrate.rmeta
COPY --link --from=out-3333333333333333 /deps/my_bin-3333333333333333 /my-bin

"#]]
        );
    }
}
