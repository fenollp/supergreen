use std::collections::HashSet;

use anyhow::{Result, anyhow};
use camino::Utf8Path;
use log::{debug, info, warn};

use crate::{
    build::{ERRCODE, Effects, STDERR, STDOUT},
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
        green_set_envs: &[String],
        vars: &Vars,
        call: &str,
        (out_stage, out_dir): (&Stage, Option<&Utf8Path>),
    ) -> Result<()> {
        let mut first = true;
        let mut push = |block: &mut String, var: &str, val: &String| -> Result<_> {
            let val = paths.rewrite_env(val)?;
            block.push_str(&format!("    {} {var}={val} \\\n", if first { "env" } else { "   " }));
            first = false;
            Ok(())
        };

        let mut set: HashSet<_> =
            [CARGO!().to_owned(), "RUSTC".to_owned(), RUSTUP_TOOLCHAIN!().to_owned()].into();

        let primary = vars.contains_key(CARGO_PRIMARY_PACKAGE!());

        // Sorted, being a BTreeMap: the block has to be byte-identical across runs.
        let kvs = vars.iter().map(|(k, v)| (k.clone(), v.clone()));
        for (var, val) in kvs.filter_map(|kv| fmap_env(kv, self.buildrs, primary)) {
            if set.contains(&var) {
                continue;
            }
            push(&mut block, &var, &val)?;
            set.insert(var.clone());
        }
        block.push_str(&format!("        {}=1 \\\n", CARGOGREEN!()));

        for (var, val) in &self.set_envs {
            if set.contains(var) {
                continue;
            }
            warn!("setting rustc-env: ${var}={val:?}");
            push(&mut block, var, val)?;
            set.insert(var.to_owned());
        }

        for var in green_set_envs {
            if set.contains(var) {
                continue;
            }
            if let Some(val) = vars.get(var) {
                warn!("passing ${var}={val:?} env through");
                push(&mut block, var, val)?;
                set.insert(var.to_owned());
            }
        }

        // TODO: keep only paths that we explicitly mount or copy
        if false {
            // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
            for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
                let Some(val) = vars.get(var) else { continue };
                if set.contains(var) {
                    continue;
                }
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
        let (call, envs, Effects { written, stdout, stderr, rustc_envs }, result, built) =
            green.build_out(containerfile_path, stage, &self.contexts, out_dir).await;

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
                .filter(|(_, f)| f != &format!("{stage}-{STDOUT}"))
                .filter(|(_, f)| f != &format!("{stage}-{STDERR}"))
                .filter(|(_, f)| f != &format!("{stage}-{ERRCODE}"))
                .map(|(w, f)| (w, f.replace(&format!("-{}", self.this()), "")))
                .map(|(w, f)| (w, f.replace("_", "-"))) // cargo-install rewrites underscores
                .map(|(w, f)| (w, f.replace(".dwp", ""))) // cargo-install drops that extension
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
    use std::sync::Arc;

    use snapbox::str;

    use super::{Effects, Green, Md, Stage};
    use crate::{
        containerfile::assert_containerfile_eq,
        dirs::Paths,
        r#final::Final,
        md::MdId,
        sys::{
            Sys,
            fake::{FakeBuilds, FakeFs},
            install,
        },
    };

    const CONTAINERFILE: &str = "/work/target/debug/mycrate-3333333333333333.Dockerfile";
    const MD: &str = "/work/target/debug/3333333333333333.toml";
    const FINAL: &str = "/work/recipe.Dockerfile";

    #[test]
    fn the_recipe_ends_with_the_crate_s_artifacts() {
        let mdid: MdId = 0x3333333333333333_u64.into();
        let stage = Stage::output(mdid).unwrap();

        let fs = Arc::new(FakeFs::default());
        fs.file(CONTAINERFILE, "FROM rust AS rust-base\nFROM rust-base AS dep-n-mycrate-0.1.0\n");
        let builds = Arc::new(FakeBuilds {
            effects: Effects {
                // As `untar_into` reports them: bare names, relative to the out dir.
                written: vec![
                    // Kept, with the disambiguating hash stripped back off.
                    "libmycrate-3333333333333333.rlib".into(),
                    "libmycrate-3333333333333333.rmeta".into(),
                    // cargo-install rewrites underscores.
                    "my_bin-3333333333333333".into(),
                    // Dropped: of no use inside an image. The .dwp especially, since
                    // it would otherwise be copied over my_bin under the same name.
                    "mycrate-3333333333333333.d".into(),
                    "my_bin-3333333333333333.dwp".into(),
                ],
                ..Effects::default()
            },
            ..FakeBuilds::default()
        });
        let _guard = install(Sys {
            fs: Arc::clone(&fs) as _,
            builds: Arc::clone(&builds) as _,
            ..Sys::fake()
        });

        let green = Green {
            r#final: Final { path: Some(FINAL.into()) },
            experiment: vec!["finalpathnonprimary".to_owned()],
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
            str![[r#"
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
