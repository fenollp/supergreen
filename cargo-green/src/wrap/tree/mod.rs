//! A dependency tree grown one cargo call at a time, as in `recipes/vixargs@0.1.0.Dockerfile`:
//!
//! ```text
//! autocfg                     N  lib, from crates.io
//!  └─ num-traits/build.rs     X  its build script, compiled against autocfg
//!      └─ (running it)        Z  writes $OUT_DIR
//!          └─ num-traits      N  lib, compiled with that $OUT_DIR
//!              └─ mycrate     N  bin, local code
//! ```
//!
//! Each step is a real [`wrap_rustc`] or [`exec_build_script`] call. The only thing
//! faked is what the runner reports back; each step's Md is read by the next.
//!
//! Further down, other shapes the `recipes/` corpus holds, each grown from as few
//! crates as it takes.

use std::sync::Arc;

use super::{exec_build_script, wrap_rustc};
use crate::{
    base_image::BaseImage,
    dirs::Paths,
    r#final::Final,
    green::Green,
    redact::Vars,
    runner::Runner,
    sys::{
        Sys,
        fake::{
            FakeBuilds, FakeCargo, FakeFs, FakeGit,
            Stream::{Stderr, Stdout},
        },
    },
};

pub(crate) const PWD: &str = "/home/u/mycrate";
pub(crate) const DEBUG: &str = "/home/u/mycrate/target/debug";
pub(crate) const RECIPE: &str = "/home/u/recipe.Dockerfile";

pub(crate) const AUTOCFG: &str = "1111111111111111";
pub(crate) const BUILT: &str = "2222222222222222";
pub(crate) const RUN: &str = "3333333333333333";
pub(crate) const NUM_TRAITS: &str = "4444444444444444";
pub(crate) const MYCRATE: &str = "5555555555555555";

/// Where a crate's source comes from.
#[derive(Clone, Copy, Default)]
pub(crate) enum Src {
    #[default]
    Registry,
    /// A member of the [`Host::checkout`]ed workspace, by its path in there.
    Git(&'static str),
    /// [`Host::pwd`]
    Local,
}

/// One `rustc` call, as cargo makes it. Unset fields get cargo's defaults.
#[derive(Default)]
pub(crate) struct Krate<'a> {
    pub(crate) pkg: &'a str,
    pub(crate) version: &'a str,
    pub(crate) mdid: &'a str,
    pub(crate) src: Src,
    /// `lib` when empty.
    pub(crate) crate_types: &'a [&'a str],
    /// When not `pkg`'s (eg. `build_script_build`).
    pub(crate) name: Option<&'a str>,
    /// Relative to the manifest dir, when not `src/{lib,main}.rs`.
    pub(crate) input: Option<&'a str>,
    /// When not the profile's `deps`.
    pub(crate) out_dir: Option<String>,
    pub(crate) edition: Option<&'a str>,
    /// Cross-compiling for this triple.
    pub(crate) target: Option<&'a str>,
    /// Each `name=path`.
    pub(crate) externs: Vec<String>,
    pub(crate) args: String,
    pub(crate) vars: &'a [(&'a str, &'a str)],
    /// What rustc reports, eg. diagnostics.
    pub(crate) stderr: &'a [&'a str],
}

/// Running a build script, as cargo does.
#[derive(Default)]
pub(crate) struct Run<'a> {
    pub(crate) pkg: &'a str,
    pub(crate) version: &'a str,
    /// The build script's compilation, and this run.
    pub(crate) built: &'a str,
    pub(crate) mdid: &'a str,
    pub(crate) src: Src,
    pub(crate) target: Option<&'a str>,
    pub(crate) vars: &'a [(&'a str, &'a str)],
    /// `cargo:rustc-env=`s printed after `stdout`, and the files written.
    pub(crate) sets: &'a [(&'a str, &'a str)],
    pub(crate) writes: &'a [&'a str],
    /// The script's own output, `cargo::` instructions included.
    pub(crate) stdout: &'a [&'a str],
}

/// Where a project builds: all that is specific to the host. Defaults to [`PWD`]'s.
#[derive(Clone, Debug)]
pub(crate) struct Host {
    pub(crate) home: String,
    pub(crate) project: String,
    /// Of `index.crates.io-<hash>`, which cargo derives from its own version.
    pub(crate) index: String,
    /// `debug`, `release`…
    pub(crate) profile: String,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            home: "/home/u".into(),
            project: "mycrate".into(),
            index: "1949cf8c6b5b557f".into(),
            profile: "debug".into(),
        }
    }
}

impl Host {
    pub(crate) fn cargo_home(&self) -> String {
        format!("{}/.cargo", self.home)
    }

    pub(crate) fn src(&self) -> String {
        format!("{}/registry/src/index.crates.io-{}", self.cargo_home(), self.index)
    }

    pub(crate) fn cache(&self) -> String {
        format!("{}/registry/cache/index.crates.io-{}", self.cargo_home(), self.index)
    }

    pub(crate) fn checkout(&self) -> String {
        format!("{}/git/checkouts/remarkable-tools-9f4e9942cc4e93a3/268a3ad", self.cargo_home())
    }

    pub(crate) fn fetch_head(&self) -> String {
        format!("{}/git/db/remarkable-tools-9f4e9942cc4e93a3/FETCH_HEAD", self.cargo_home())
    }

    pub(crate) fn pwd(&self) -> String {
        format!("{}/{}", self.home, self.project)
    }

    /// The profile's dir, of the host's or `target`'s.
    pub(crate) fn profile(&self, target: Option<&str>) -> String {
        let triple = target.map(|t| format!("{t}/")).unwrap_or_default();
        format!("{}/target/{triple}{}", self.pwd(), self.profile)
    }

    pub(crate) fn recipe(&self) -> String {
        format!("{}/recipe.Dockerfile", self.home)
    }

    /// Manifest dir, then where cargo calls rustc from, and how it names inputs from there.
    pub(crate) fn dirs(&self, pkg: &str, version: &str, src: Src) -> (String, String, String) {
        let checkout = self.checkout();
        match src {
            Src::Registry => {
                let dir = format!("{}/{pkg}-{version}", self.src());
                (dir.clone(), dir.clone(), format!("{dir}/"))
            }
            Src::Git("") => (checkout.clone(), checkout, String::new()),
            Src::Git(member) => (format!("{checkout}/{member}"), checkout, format!("{member}/")),
            Src::Local => (self.pwd(), self.pwd(), String::new()),
        }
    }
}

pub(crate) struct Tree {
    pub(crate) host: Host,
    pub(crate) fs: Arc<FakeFs>,
    /// What cargo got to read, across all calls.
    pub(crate) cargo: Arc<FakeCargo>,
    /// `build_script_build`, or its legacy name.
    pub(crate) buildrs: std::cell::Cell<&'static str>,
    pub(crate) experiments: Vec<&'static str>,
    /// `$CARGOGREEN_SET_ENVS`
    pub(crate) set_envs: Vec<String>,
    /// More checkouts, by dir, and the `FETCH_HEAD` of each.
    pub(crate) git: std::cell::RefCell<Vec<(String, String)>>,
    /// Kinds of [`violations`] a FIXME test expects.
    pub(crate) tolerate: Vec<String>,
}

impl Tree {
    pub(crate) fn new() -> Self {
        Self::on(Host::default())
    }

    pub(crate) fn on(host: Host) -> Self {
        let fs = FakeFs::new();
        fs.file(
            host.fetch_head(),
            "268a3ad78f33f306d308d1b1372d655215e160b2\t\thttps://github.com/fenollp/reMarkable-tools.git",
        );
        fs.pushd(host.pwd());
        fs.file("/Cargo.toml", "");
        fs.file("/src/main.rs", "");
        fs.file("/target/CACHEDIR.TAG", "");
        fs.popd();
        Self {
            host,
            fs,
            cargo: FakeCargo::new(),
            buildrs: std::cell::Cell::new("build_script_build"),
            experiments: vec!["finalpathnonprimary"],
            set_envs: vec![],
            git: Default::default(),
            tolerate: vec![],
        }
    }

    pub(crate) fn green(&self, vars: &[(&str, &str)]) -> Green {
        let env: Vars = vars.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect();
        Green {
            env,
            runner: Runner::Docker,
            base: BaseImage {
                image_inline: "FROM docker.io/library/rust:1.99.0-slim AS rust-base".to_owned(),
                ..Default::default()
            },
            r#final: Final { path: Some(self.host.recipe().into()) },
            experiment: self.experiments.iter().map(|&x| x.to_owned()).collect(),
            set_envs: self.set_envs.clone(),
            paths: Paths {
                cargo_home: self.host.cargo_home().into(),
                cwd: self.host.pwd().into(),
                host_target_dir: Some(format!("{}/target", self.host.pwd()).into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Runs `f` as if its build then left `stdout` and `stderr`, and wrote `written`.
    /// Returns the Containerfile it built, once checked against [`violations`].
    pub(crate) fn call(
        &self,
        (stdout, stderr): (&str, &str),
        written: &[String],
        f: impl AsyncFnOnce() -> anyhow::Result<()>,
    ) -> String {
        let builds = FakeBuilds::with_stdio(stdout, stderr, written);
        let members = ["", "/pb", "/ostd/libs/align_ext"];
        let heads =
            members.map(|m| (format!("{}{m}", self.host.checkout()), self.host.fetch_head()));
        let git = FakeGit::with_heads(heads.into_iter().chain(self.git.borrow().iter().cloned()));
        let _guard = Sys::install(Sys {
            fs: self.fs.clone(),
            builds: builds.clone(),
            cargo: self.cargo.clone(),
            git,
            ..Sys::fake()
        });
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f()).unwrap();
        // One build, two with `incremental`: both of the same Containerfile
        let built = builds.built();
        assert!(!built.is_empty() && built.iter().all(|b| *b == built[0]), "{built:?}");
        let containerfile = self.fs.read(&built[0]).unwrap();
        let contexts: Vec<_> = builds.contexts()[0].iter().map(|c| c.name.to_string()).collect();
        let mut found = violations(&containerfile, &self.host.home, &contexts);
        found.retain(|v| !self.tolerate.iter().any(|kind| v.starts_with(kind)));
        assert!(found.is_empty(), "{found:#?}\nin {containerfile}");
        containerfile
    }

    pub(crate) fn compile(&self, k: Krate) -> String {
        let Krate { pkg, version, mdid, src, .. } = k;
        let name = k.name.map_or_else(|| pkg.replace('-', "_"), ToOwned::to_owned);
        let types = if k.crate_types.is_empty() { &["lib"][..] } else { k.crate_types };
        let is = |t| types.contains(&t);
        let (dir, pwd, prefix) = self.host.dirs(pkg, version, src);
        if let Src::Registry = src {
            let tarball = format!("{}/{pkg}-{version}.crate", self.host.cache());
            self.fs.file(tarball, format!("<{pkg} tarball>"));
        }
        let profile = self.host.profile(k.target);
        let out_dir = k.out_dir.unwrap_or_else(|| format!("{profile}/deps"));
        let input = k.input.unwrap_or(if is("bin") { "src/main.rs" } else { "src/lib.rs" });

        let mut args = format!(
            "--crate-name {name} --edition={edition} {prefix}{input} --emit=dep-info,{emit}link \
             -C extra-filename=-{mdid} --out-dir {out_dir} -L dependency={profile}/deps",
            edition = k.edition.unwrap_or("2021"),
            emit = if is("lib") { "metadata," } else { "" },
        );
        types.iter().for_each(|t| args.push_str(&format!(" --crate-type {t}")));
        if let Some(triple) = k.target {
            let host = self.host.profile(None);
            args.push_str(&format!(" --target {triple} -L dependency={host}/deps"));
        }
        if is("proc-macro") {
            args.push_str(" -C prefer-dynamic --extern proc_macro");
        }
        k.externs.iter().for_each(|x| args.push_str(&format!(" --extern {x}")));
        args.push(' ');
        args.push_str(&k.args);

        let mut writes = vec![];
        for t in types {
            writes.extend(match *t {
                "lib" => vec![format!("lib{name}-{mdid}.rlib"), format!("lib{name}-{mdid}.rmeta")],
                "rlib" if is("lib") => vec![],
                "rlib" => vec![format!("lib{name}-{mdid}.rlib")],
                "proc-macro" | "cdylib" => vec![format!("lib{name}-{mdid}.so")],
                "staticlib" => vec![format!("lib{name}-{mdid}.a")],
                "bin" => vec![format!("{name}-{mdid}")],
                _ => unreachable!("{t}"),
            });
        }
        writes.push(format!("{name}-{mdid}.d"));

        let mut vars = vec![
            ("CARGO_CRATE_NAME", name.as_str()),
            ("CARGO_MANIFEST_DIR", &dir),
            ("CARGO_PKG_NAME", pkg),
            ("CARGO_PKG_VERSION", version),
        ];
        vars.extend(k.vars);
        let green = self.green(&vars);
        let args = args.split_whitespace().map(ToOwned::to_owned).collect();
        let stderr = k.stderr.join("\n");
        self.call(("", &stderr), &writes, async || {
            wrap_rustc(green, args, pwd.as_str().into(), async { unreachable!("no fallback") })
                .await
        })
    }

    /// Compiles `k`'s build script.
    pub(crate) fn buildrs(&self, k: Krate) -> String {
        let out_dir = Some(format!("{}/build/{}-{}", self.host.profile(None), k.pkg, k.mdid));
        let name = Some(self.buildrs.get());
        self.compile(Krate { name, crate_types: &["bin"], input: Some("build.rs"), out_dir, ..k })
    }

    pub(crate) fn run(&self, r: Run) -> String {
        let (dir, _, _) = self.host.dirs(r.pkg, r.version, r.src);
        let out_dir = format!("{}/build/{}-{}/out", self.host.profile(r.target), r.pkg, r.mdid);
        let mut vars = vec![
            ("CARGO_MANIFEST_DIR", dir.as_str()),
            ("CARGO_PKG_NAME", r.pkg),
            ("CARGO_PKG_VERSION", r.version),
            ("OUT_DIR", &out_dir),
        ];
        vars.extend(r.vars);
        let green = self.green(&vars);
        let host = self.host.profile(None);
        let exe =
            format!("{host}/build/{}-{}/{}", r.pkg, r.built, self.buildrs.get().replace('_', "-"));
        let sets = r.sets.iter().map(|(k, v)| format!("cargo:rustc-env={k}={v}"));
        let stdout: Vec<_> = r.stdout.iter().map(|&l| l.to_owned()).chain(sets).collect();
        let writes: Vec<_> = r.writes.iter().map(|&w| w.to_owned()).collect();
        self.call((&stdout.join("\n"), ""), &writes, async || {
            exec_build_script(green, exe.into()).await
        })
    }

    /// The Md of `mdid`, minus its stages (the Containerfile shows those).
    pub(crate) fn md(&self, mdid: &str) -> String {
        let md = self.fs.read(format!("{}/{mdid}.toml", self.host.profile(None))).unwrap();
        md.split_once("\n[[stages]]").unwrap().0.to_owned()
    }

    pub(crate) fn autocfg(&self) -> String {
        let (pkg, version, mdid, edition) = ("autocfg", "1.0.1", AUTOCFG, Some("2015"));
        self.compile(Krate { pkg, version, mdid, edition, ..Default::default() })
    }

    pub(crate) fn num_traits_buildrs(&self) -> String {
        self.buildrs(Krate {
            pkg: "num-traits",
            version: "0.2.14",
            mdid: BUILT,
            edition: Some("2015"),
            externs: vec![format!("autocfg={DEBUG}/deps/libautocfg-{AUTOCFG}.rlib")],
            ..Default::default()
        })
    }

    pub(crate) fn num_traits_run_setting(&self, sets: &[(&str, &str)], writes: &[&str]) -> String {
        self.run(Run {
            pkg: "num-traits",
            version: "0.2.14",
            built: BUILT,
            mdid: RUN,
            vars: &[("TARGET", "x86_64-unknown-linux-gnu"), ("NUM_JOBS", "32")],
            sets,
            writes,
            ..Default::default()
        })
    }

    pub(crate) fn num_traits_run(&self, writes: &[&str]) -> String {
        self.num_traits_run_setting(&[("NUM_TRAITS_PROBED", "1")], writes)
    }

    pub(crate) fn num_traits(&self) -> String {
        let out_dir = format!("{DEBUG}/build/num-traits-{RUN}/out");
        self.compile(Krate {
            pkg: "num-traits",
            version: "0.2.14",
            mdid: NUM_TRAITS,
            edition: Some("2015"),
            args: "--cfg has_i128".into(),
            vars: &[("OUT_DIR", &out_dir)],
            ..Default::default()
        })
    }

    pub(crate) fn mycrate(&self) -> String {
        let externs = vec![format!("num_traits={DEBUG}/deps/libnum_traits-{NUM_TRAITS}.rlib")];
        self.bin(MYCRATE, externs, &[])
    }

    /// A crates.io library depending on `externs`.
    pub(crate) fn lib(&self, pkg: &str, version: &str, mdid: &str, externs: Vec<String>) -> String {
        self.compile(Krate { pkg, version, mdid, externs, ..Default::default() })
    }

    /// A crates.io proc-macro depending on `externs`.
    pub(crate) fn proc_macro(
        &self,
        pkg: &str,
        version: &str,
        mdid: &str,
        externs: Vec<String>,
    ) -> String {
        let crate_types = &["proc-macro"];
        self.compile(Krate { pkg, version, mdid, crate_types, externs, ..Default::default() })
    }

    /// The local bin, `mycrate`.
    pub(crate) fn bin(&self, mdid: &str, externs: Vec<String>, vars: &[(&str, &str)]) -> String {
        let (pkg, version, src, crate_types) = ("mycrate", "0.1.0", Src::Local, &["bin"][..]);
        self.compile(Krate {
            pkg,
            version,
            mdid,
            src,
            crate_types,
            externs,
            vars,
            ..Default::default()
        })
    }

    /// Up to num-traits, returning its Containerfile.
    pub(crate) fn grow_num_traits(&self) -> String {
        self.autocfg();
        self.num_traits_buildrs();
        self.num_traits_run(&["out/probe.rs"]);
        self.num_traits()
    }

    /// The whole tree, returning mycrate's Containerfile.
    pub(crate) fn grow(&self) -> String {
        self.grow_num_traits();
        self.mycrate()
    }
}

/// What `child` adds to the Containerfile of its last dependency, `parent`: a crate's
/// Containerfile always extends those of its dependencies.
pub(crate) fn grown<'a>(parent: &str, child: &'a str) -> &'a str {
    child.strip_prefix(parent).unwrap_or_else(|| panic!("{child}\n\ndoes not extend\n\n{parent}"))
}

/// The block of the stage whose name starts with `name`, up to the next stage.
pub(crate) fn stage<'a>(containerfile: &'a str, name: &str) -> &'a str {
    let at = containerfile.find(&format!(" AS {name}")).unwrap_or_else(|| panic!("no {name}"));
    let at = containerfile[..at].rfind('\n').map_or(0, |nl| nl + 1);
    let block = &containerfile[at..];
    block[1..].find("\nFROM ").map_or(block, |end| &block[..=end + 1])
}

/// A stage without what every call ends with (redirecting its stdio, then touching what it
/// wrote): `scenarios::a_registry_crate` shows it all once.
pub(crate) fn brief(block: &str) -> String {
    let tail = ["        1>", "        2>", "        || echo $?", "  ; find ", " || echo $?"];
    let mut out = String::new();
    let mut elided = false;
    for line in block.lines() {
        if tail.iter().any(|t| line.starts_with(t)) {
            if !elided {
                out.push_str("        …\n");
            }
            elided = true;
            continue;
        }
        elided = false;
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The `export …` lines of the stage starting with `name`, up to the command they run:
/// `rustc` or a build script (an absolute path).
pub(crate) fn envs(containerfile: &str, name: &str) -> String {
    let command = |l: &str| l.starts_with("rustc ") || l.starts_with('/');
    let lines = stage(containerfile, name).lines().skip_while(|l| *l != "export \\");
    lines.take_while(|l| !command(l)).map(|l| format!("{l}\n")).collect()
}

/// The `rustc` call of the stage starting with `name`.
pub(crate) fn rustc<'a>(containerfile: &'a str, name: &str) -> &'a str {
    let call = stage(containerfile, name).lines().find(|l| l.starts_with("rustc "));
    call.unwrap().trim_end_matches(" \\")
}

/// An `--extern` to a crate's file of extension `ext`, in the host's `deps`.
pub(crate) fn ext(name: &str, mdid: &str, ext: &str) -> String {
    format!("{name}={DEBUG}/deps/lib{name}-{mdid}.{ext}")
}

/// The `--mount`s of the stage starting with `name`, in order.
pub(crate) fn mounts(containerfile: &str, name: &str) -> String {
    let lines = stage(containerfile, name).lines().filter(|l| l.starts_with("  --mount="));
    lines.map(|l| format!("{}\n", l.trim_end_matches(" \\"))).collect()
}

pub(crate) fn recipe_tail(t: &Tree) -> String {
    t.fs.read(t.host.recipe()).unwrap().split_once("\n# Pipe").unwrap().1.to_owned()
}

mod checker;
mod scenarios;

pub(crate) use checker::violations;
