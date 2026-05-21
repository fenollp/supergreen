use std::{collections::HashSet, env};

use anyhow::{anyhow, Result};
use camino::Utf8Path;
use log::{debug, info, warn};

use crate::{
    build::{Effects, ERRCODE, STDERR, STDOUT},
    green::Green,
    md::Md,
    stage::Stage,
    target_dir::virtual_target_dir,
    wrap::{
        build_script::{exe_dance, is_buildrs_executable},
        envs::{fmap_env, rewrite_env},
    },
    ENV,
};

impl Md {
    pub(crate) fn call_block(
        &mut self,
        (stage, mut block): (&Stage, String),
        crate_name: Option<&str>,
        cargo_home: &Utf8Path,
        green_set_envs: &[String],
        call: &str,
        (out_stage, out_dir): (&Stage, Option<&Utf8Path>),
    ) -> Result<()> {
        let mut first = true;
        let mut push = |block: &mut String, var: &str, val: &String| -> Result<_> {
            let val = rewrite_env(val, cargo_home)?;
            block.push_str(&format!("    {} {var}={val} \\\n", if first { "env" } else { "   " }));
            first = false;
            Ok(())
        };

        let mut set: HashSet<_> =
            ["CARGO".to_owned(), "RUSTC".to_owned(), "RUSTUP_TOOLCHAIN".to_owned()].into();

        let mut vars = env::vars().collect::<Vec<_>>();
        vars.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (var, val) in vars.into_iter().filter_map(|kv| fmap_env(kv, self.buildrs)) {
            if set.contains(&var) {
                continue;
            }
            push(&mut block, &var, &val)?;
            set.insert(var.clone());
        }
        block.push_str(&format!("        {}=1 \\\n", ENV!()));

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
            if let Ok(val) = env::var(var) {
                warn!("passing ${var}={val:?} env through");
                push(&mut block, var, &val)?;
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
                push(&mut block, var, &val)?;
            }
        }

        let out_dir = out_dir.map(virtual_target_dir).unwrap_or(".".into());
        // TODO: let out_dir = out_dir.map(|_| "$OLDPWD").unwrap_or("$PWD"); whence  https://github.com/moby/buildkit/issues/6698  [frontend] $OLDPWD is unset (after >1 WORKDIR layers)

        block.push_str(&format!("      {call} \\\n"));
        block.push_str(&format!("        1>          {out_dir}/{out_stage}-{STDOUT} \\\n"));
        block.push_str(&format!("        2>          {out_dir}/{out_stage}-{STDERR} \\\n"));
        block.push_str(&format!("        || echo $? >{out_dir}/{out_stage}-{ERRCODE}\\\n"));

        if let Some(crate_name) = crate_name {
            if is_buildrs_executable(crate_name) {
                block.push_str(&exe_dance(self.this(), crate_name, &out_dir));
                block.push_str(&format!(" || echo $? >{out_dir}/{out_stage}-{ERRCODE} \\\n"));
            }
        }

        // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
        let pattern = if self.buildrs { "*".to_owned() } else { format!("*-{}*", self.this()) };
        block.push_str(&format!("  ; find {out_dir}/{pattern} -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{{}}' + \\\n"));
        block.push_str(&format!(" || echo $? >{out_dir}/{out_stage}-{ERRCODE}\n"));

        self.push_block(stage, &block);
        Ok(())
    }

    /// TODO? in Dockerfile, when using outputs:
    /// => skip the COPY (--mount=from=out-08c4d63ed4366a99) use the stage directly
    pub(crate) fn out_block(
        &mut self,
        stage: &Stage,
        prev: &Stage,
        out_dir: &Utf8Path,
        buildrs: bool,
    ) {
        let mut block = format!("FROM scratch AS {stage}\n");
        let out_dir = virtual_target_dir(out_dir);
        if buildrs {
            block.push_str(&format!("COPY --link --from={prev} {out_dir} /\n"));
        } else {
            let mdid = self.this();
            block.push_str(&format!("COPY --link --from={prev} {out_dir}/*-{mdid}* /\n"));
        }
        self.push_block(stage, &block);
    }

    pub(crate) async fn do_build(
        &mut self,
        green: &Green,
        containerfile_path: &Utf8Path,
        stage: &Stage,
        out_dir: &Utf8Path,
        target_path: &Utf8Path,
    ) -> Result<()> {
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
            self.set_envs = cargo_rustc_env;
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

        built
    }
}
