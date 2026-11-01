use std::collections::HashSet;

use anyhow::{Result, anyhow};
use camino::Utf8Path;
use log::{debug, info, warn};

use crate::{
    build::{Built, Effects, STDCODE, STDERR, STDOUT},
    dirs::Paths,
    dotd::is_dotd,
    green::Green,
    md::Md,
    redact::{Redacted, Vars},
    rustc_arguments::encode_rustflags,
    stage::Stage,
    wrap::{
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
        out_dir: Option<&Utf8Path>,
    ) -> Result<()> {
        block.push_str("  <<'EOR'\n");
        block.push_str("export \\\n");

        let mut set: HashSet<_> = [CARGO!(), "RUSTC", RUSTUP_TOOLCHAIN!()].into();
        macro_rules! export_env {
            ($var:expr, $val:expr) => {{
                let just = set.insert($var);
                if just {
                    block.push_str(&format!("  {}={} \\\n", $var, paths.rewrite_env($val)?));
                }
                just
            }};
        }

        let is_run = self.is_buildrs_run();
        for (k, v) in env {
            let Some((k, v)) = fmap_env((k.as_str(), v.as_str()), is_run) else { continue };
            if k == "CARGO_ENCODED_RUSTFLAGS" {
                // Spelled as rustc's arguments are, paths rewritten
                let v = encode_rustflags(v, |val| paths.rewrite_str(val))?;
                export_env!(k, &v);
            } else {
                export_env!(k, v);
            }
        }

        // Crates for testing (eg. `snapbox`) rely on $CARGO_RUSTC_CURRENT_DIR
        // (cargo support discontinued since https://github.com/rust-lang/cargo/pull/14799)
        // to locate sources at runtime and we remapped local sources under `VIRTUAL_CWD`
        if locates_sources {
            let (var, val) = (CARGO_RUSTC_CURRENT_DIR!(), paths.cwd.as_str());
            if export_env!(var, val) {
                warn!("remmapping ${var}={val:?} env");
            }
        }

        for (var, val) in &self.set_envs {
            if export_env!(var, val) {
                warn!("setting rustc-env: ${:?}", Redacted(var, val));
            }
        }

        for var in green_set_envs {
            let Some(val) = env.get(var.as_str()) else { continue };
            if export_env!(var, val) {
                warn!("passing ${:?} env through", Redacted(var, val));
            }
        }

        if false {
            // https://github.com/maelstrom-software/maelstrom/blob/ef90f8a990722352e55ef1a2f219ef0fc77e7c8c/crates/maelstrom-util/src/elf.rs#L4
            for var in ["PATH", "DYLD_FALLBACK_LIBRARY_PATH", "LD_LIBRARY_PATH", "LIBPATH"] {
                let Some(val) = env.get(var) else { continue };
                if export_env!(var, val) {
                    debug!("system env set (skipped): ${:?}", Redacted(var, val));
                }
            }
        }

        block.push_str(&format!("  {}=1\n", CARGOGREEN!())); // Exports terminator

        let out_dir = out_dir.map(|d| paths.rewrite_target_dir(d)).unwrap_or(".".into());
        // TODO: let out_dir = out_dir.map(|_| "$OLDPWD").unwrap_or("$PWD"); whence  https://github.com/moby/buildkit/issues/6698  [frontend] $OLDPWD is unset (after >1 WORKDIR layers)
        let outdir_stdio = format!("{out_dir}/..")
            .replace("./..", "..")
            .replace("/out/..", "")
            .replace("/deps/..", "");
        let outfile = |part: &str| format!("{outdir_stdio}/{part}");

        block.push_str(&format!("{call} \\\n"));
        let (stdout, stderr, stdcode) = (outfile(STDOUT), outfile(STDERR), outfile(STDCODE));
        block.push_str(&format!("  1>{stdout} 2>{stderr} || echo $? >{stdcode}\n"));

        let mdid = self.this();
        if let Some(crate_name) = crate_name
            && is_buildrs_executable(crate_name)
        {
            block.push_str(&exe_dance(mdid, crate_name, &out_dir));
        }

        // TODO: [`COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
        let pattern =
            if is_run { "" } else { &format!(" \\( -name '*-{mdid}*' -o -name 'std*' \\)") };
        block.push_str(&format!("find {out_dir}/ {outdir_stdio}/std*{pattern} -exec touch --no-dereference --date=@$SOURCE_DATE_EPOCH '{{}}' +\n"));
        block.push_str("EOR\n");

        self.push_block(stage, &block);
        Ok(())
    }

    /// TODO? in Dockerfile, when using outputs:
    /// => skip the COPY (--mount=from=out-08c4d63ed4366a99) use the stage directly
    /// See also: [exporter: add src option to local and tar exporters - #7117](https://github.com/moby/buildkit/pull/7117)
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
        let up = out_dir.parent().expect("PROOF: out_dir has parents");
        // --parents keeps paths after the ./ pivot: {out_dir} lands at /{base} and std* at /
        block.push_str(&format!(
            "COPY --link --parents --from={prev} {up}/./{base} {up}/./std* /\n"
        ));
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

        if let Some(result) = result
            && built.is_ok()
            && let Err(e) = async {
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

        let base = green.paths.rewrite_target_dir(out_dir);
        let base = base.file_name().expect("PROOF: out_dir has a file name");
        let final_stage = format!(
            "FROM scratch\n{}\n",
            self.writes
                .iter()
                .filter_map(|w| w.file_name().map(|f| (w, f)))
                .filter(|(_, f)| !is_dotd(f))
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

#[cfg(test)]
mod do_build {
    use std::sync::Arc;

    use camino::Utf8Path;

    use super::{Green, Md, Stage};
    use crate::{
        dirs::Paths,
        r#final::Final,
        md::MdId,
        sys::{
            Sys,
            fake::{FakeBuilds, FakeFs},
        },
    };

    macro_rules! CWD {
        ($path:literal) => {
            concat!("/home/u/wrk", $path)
        };
    }

    const CONTAINERFILE: &str = CWD!("/target/debug/mycrate-3333333333333333.Dockerfile");
    const MD: &str = CWD!("/target/debug/3333333333333333.toml");
    const FINAL: &str = CWD!("/recipe.Dockerfile");

    fn with<P: AsRef<Utf8Path>>(
        files: impl IntoIterator<Item = P>,
        f: impl AsyncFnOnce(Green, Arc<FakeFs>, Arc<FakeBuilds>) -> (),
    ) {
        const GENERATED: &str = "\
FROM rust AS rust-base
FROM rust-base AS dep-n-mycrate-0.1.0
";
        let fs = FakeFs::new();
        fs.file(CONTAINERFILE, GENERATED);

        let builds = FakeBuilds::with_written(files);
        let _guard = Sys::install(Sys { fs: fs.clone(), builds: builds.clone(), ..Sys::fake() });

        let green = Green {
            r#final: Final { path: Some(FINAL.into()) },
            experiment: ["finalpathnonprimary".into()].into(),
            paths: Paths {
                cwd: CWD!("").into(),
                host_target_dir: Some(CWD!("/target").into()),
                ..Default::default()
            },
            ..Default::default()
        };

        use tokio::runtime::Builder;
        Builder::new_current_thread().build().unwrap().block_on(f(green, fs, builds));
    }

    #[test]
    fn the_recipe_ends_with_the_crate_s_artifacts() {
        let mdid: MdId = 0x3333333333333333_u64.into();
        let mut md: Md = mdid.into();
        md.push_block(&crate::stage::RUST, "FROM rust AS rust-base");

        let written = [
            // Kept
            "libmycrate-3333333333333333.rlib",
            "libmycrate-3333333333333333.rmeta",
            // cargo-install rewrites underscores
            "my_bin-3333333333333333",
            // Dropped
            "mycrate-3333333333333333.d",
            "my_bin-3333333333333333.dwp",
        ];

        with(written, async |green, fs, builds| {
            md.do_build(
                &green,
                MD.into(),
                CONTAINERFILE.into(),
                &Stage::output(mdid),
                CWD!("/target/debug/deps").into(),
            )
            .await
            .unwrap();

            assert_eq!(builds.built(), [CONTAINERFILE]);

            let all_written = fs
                .written()
                .into_iter()
                .filter(|p| !written.iter().any(|f| p.as_str().ends_with(f)))
                .collect::<Vec<_>>();
            assert_eq!(all_written, [FINAL, MD, CONTAINERFILE]);
            snapbox::assert_data_eq!(
                fs.read(FINAL).unwrap(),
                snapbox::str![[r#"
...
FROM scratch
COPY --link --from=out-3333333333333333 /deps/libmycrate-3333333333333333.rlib /libmycrate.rlib
COPY --link --from=out-3333333333333333 /deps/libmycrate-3333333333333333.rmeta /libmycrate.rmeta
COPY --link --from=out-3333333333333333 /deps/my_bin-3333333333333333 /my-bin

"#]]
            );
        });
    }
}
