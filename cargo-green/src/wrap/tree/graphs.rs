//! Random crate graphs shaped after `recipes/`: as wide, deep and varied as cargo makes
//! them, then grown through the wrapper call by call. What the corpus shows (of its 5228
//! crates, in 42 recipes) sets what is drawn, and how often:
//! * crates: 3 to 333 per recipe (median 90), dependency chains up to 25 deep;
//! * `--extern`s per crate: none for 39%, one for 21%, then a long tail past 15;
//! * kinds: libs; 5.5% proc-macros; a few `cdylib`+`rlib`, `lib`+`cdylib`(+`staticlib`);
//! * 14% have a build script (with 0–3 dependencies), 29% of which write to `$OUT_DIR`,
//!   some to link a native library (`-L native` reaches 4% of crates), some setting
//!   `rustc-env`s or `links` metadata (`DEP_*`), a few named `build_script_main`;
//! * crates.io names and versions: the corpus' oddest, or any crates.io accepts, a few from
//!   git checkouts (at their root or 1–4 dirs deep), the same crate in two versions (137) or
//!   built twice (244), all built for another target (marauder), mostly in `release`.

use std::{collections::BTreeMap, sync::LazyLock};

use proptest::{prelude::*, sample::Index, test_runner::TestRng};

use super::{Host, Krate, Run, Src, Tree};

/// The oddest of the corpus' 3584 crates.io crates: uppercase, `-` and `_` mixed, short or
/// long names; pre-releases, `+build` metadata (with dashes, dots, a commit), big numbers,
/// `0.0.x`; and twins, in two versions.
const POOL: &[(&str, &str)] = &[
    ("Inflector", "0.11.4"),
    ("addr2line", "0.24.2"),
    ("adler2", "2.0.1"),
    ("aws-sdk-s3", "1.127.0"),
    ("cargo-c", "0.10.18+cargo-0.92.0"),
    ("cc", "1.0.77"),
    ("cc", "1.2.60"),
    ("derive_more-impl", "2.1.1"),
    ("gix-error", "0.0.0"),
    ("h2", "0.3.27"),
    ("h2", "0.4.13"),
    ("hickory-proto", "0.26.0-alpha.1"),
    ("lazy-regex-proc_macros", "3.4.1"),
    ("lenient_semver_version_builder", "0.4.2"),
    ("libgit2-sys", "0.17.0+1.8.1"),
    ("libgit2-sys", "0.18.3+1.9.2"),
    ("ra-ap-rustc_index_macros", "0.143.0"),
    ("ra_ap_base_db", "0.0.312"),
    ("ratatui", "0.30.0-beta.0"),
    ("rustc_apfloat", "0.2.3+llvm-462a31f5a5ab"),
    ("serde_yaml", "0.9.34+deprecated"),
    ("tikv-jemalloc-sys", "0.6.0+5.3.0-1-ge13ca993e8ccb9ba9847cc330696e02839f328f7"),
    ("timeless", "0.0.14-alpha"),
    ("timely-bytes-master", "0.13.0-dev.1"),
    ("toml", "0.9.12+spec-1.1.0"),
    ("toml", "1.1.2+spec-1.1.0"),
    ("zstd-sys", "2.0.15+zstd.1.5.7"),
    ("zstd-sys", "2.0.16+zstd.1.5.7"),
];

/// A crate from [`POOL`], or any name crates.io accepts with any semver version (as
/// `stage::props::stages_are_valid_names` draws them).
fn pkg() -> impl Strategy<Value = (String, String)> {
    let anything = (
        "[A-Za-z][A-Za-z0-9_-]{0,63}",
        "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}(-[0-9A-Za-z.-]{1,20})?(\\+[0-9A-Za-z.-]{1,20})?",
    );
    prop_oneof![
        3 => any::<Index>().prop_map(|at| {
            let (pkg, version) = at.get(POOL);
            ((*pkg).to_owned(), (*version).to_owned())
        }),
        1 => anything,
    ]
}

/// Another version of `pkg`: its twin in [`POOL`], or the next major.
fn other_version(pkg: &str, version: &str) -> String {
    match POOL.iter().find(|(p, v)| *p == pkg && *v != version) {
        Some((_, v)) => (*v).to_owned(),
        None => {
            let major: u64 = version.split('.').next().unwrap().parse().unwrap();
            format!("{}.0.0", major + 1)
        }
    }
}

/// Where git members and workspace path dependencies sit: `'static` for [`Src`].
static DIRS: LazyLock<Vec<[String; 4]>> = LazyLock::new(|| {
    (0..128)
        .map(|i| {
            [
                format!("m{i}"),
                format!("crates/m{i}"),
                format!("ostd/libs/m{i}"),
                format!("a/b/c/m{i}"),
            ]
        })
        .collect()
});

const TRIPLE: &str = "armv7-unknown-linux-musleabihf";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Lib,
    ProcMacro,
    /// `lib` and `cdylib`
    Dylib,
    /// `cdylib` and `rlib`: no `.rmeta`
    RlibOnly,
    /// `cdylib`, `lib` and `staticlib`
    Static,
}

impl Kind {
    fn crate_types(self) -> &'static [&'static str] {
        match self {
            Self::Lib => &["lib"],
            Self::ProcMacro => &["proc-macro"],
            Self::Dylib => &["cdylib", "lib"],
            Self::RlibOnly => &["cdylib", "rlib"],
            Self::Static => &["cdylib", "lib", "staticlib"],
        }
    }

    /// Whether cargo pipelines it, giving it `.rmeta`s.
    fn pipelined(self) -> bool {
        matches!(self, Self::Lib | Self::Dylib | Self::Static)
    }

    /// What of a crate of this kind cargo passes on to a dependent.
    fn extern_ext(self, pipelined: bool) -> &'static str {
        match self {
            Self::ProcMacro => "so",
            Self::RlibOnly => "rlib",
            _ if pipelined => "rmeta",
            _ => "rlib",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Origin {
    Registry,
    /// A git checkout: at its root, or that many dirs deep.
    Git(usize),
    /// A path dependency, in the workspace.
    Path,
}

#[derive(Clone, Debug)]
enum Name {
    Pkg(String, String),
    /// Another build of an earlier crate: another version of it, or the same. (Or that crate,
    /// when first.)
    Twin(Index, bool, String, String),
}

#[derive(Clone, Debug)]
struct BuildRs {
    /// Of earlier crates, as all deps: so that dropping crates, shrinking keeps graphs valid.
    deps: Vec<Index>,
    writes: bool,
    /// Builds a native library its crate links, and dependents link against.
    native: bool,
    /// How many `cargo:rustc-env`s it prints.
    sets: usize,
    /// Has `links` metadata, so dependents' build scripts get `DEP_*`s.
    links: bool,
    legacy: bool,
}

#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    name: Name,
    origin: Origin,
    deps: Vec<Index>,
    buildrs: Option<BuildRs>,
}

#[derive(Clone, Debug)]
struct Graph {
    nodes: Vec<Node>,
    /// What the local bin depends on, and maybe its own package's lib.
    root: Vec<Index>,
    root_lib: Option<Vec<Index>>,
    cross: bool,
    release: bool,
    primary: bool,
    set_env: bool,
    buildscriptsources: bool,
    incremental: bool,
}

/// Dependencies, as many as the corpus' `--extern`s.
fn fan_out(weights: [u32; 5]) -> impl Strategy<Value = Vec<Index>> {
    let [none, one, few, some, many] = weights;
    let deps = |n| proptest::collection::vec(any::<Index>(), n);
    prop_oneof![none => deps(0..=0), one => deps(1..=1), few => deps(2..=3), some => deps(4..=7), many => deps(8..=15)]
}

fn buildrs() -> impl Strategy<Value = BuildRs> {
    (
        fan_out([75, 22, 3, 0, 0]),
        proptest::bool::weighted(0.29),
        proptest::bool::weighted(0.15),
        prop_oneof![95 => Just(0), 4 => Just(1), 1 => Just(3)],
        proptest::bool::weighted(0.05),
        proptest::bool::weighted(0.02),
    )
        .prop_map(|(deps, writes, native, sets, links, legacy)| BuildRs {
            writes: writes || native,
            deps,
            native,
            sets,
            links,
            legacy,
        })
}

fn node() -> impl Strategy<Value = Node> {
    let kind = prop_oneof![
        90 => Just(Kind::Lib),
        6 => Just(Kind::ProcMacro),
        2 => Just(Kind::Dylib),
        1 => Just(Kind::RlibOnly),
        1 => Just(Kind::Static),
    ];
    let name = prop_oneof![
        92 => pkg().prop_map(|(pkg, version)| Name::Pkg(pkg, version)),
        8 => (any::<Index>(), any::<bool>(), pkg()).prop_map(|(of, same, (pkg, version))| {
            Name::Twin(of, same, pkg, version)
        }),
    ];
    let origin = prop_oneof![90 => Just(Origin::Registry), 5 => (0..4_usize).prop_map(Origin::Git), 5 => Just(Origin::Path)];
    (kind, name, origin, fan_out([39, 21, 21, 13, 6]), proptest::option::weighted(0.14, buildrs()))
        .prop_map(|(kind, name, origin, deps, buildrs)| Node { kind, name, origin, deps, buildrs })
}

fn graph() -> impl Strategy<Value = Graph> {
    let nodes = |n| proptest::collection::vec(node(), n);
    (
        prop_oneof![6 => nodes(1..=10), 3 => nodes(11..=30), 1 => nodes(31..=80)],
        fan_out([0, 30, 40, 20, 10]),
        proptest::option::weighted(0.3, fan_out([20, 40, 40, 0, 0])),
        (
            proptest::bool::weighted(0.15),
            proptest::bool::weighted(0.7),
            any::<bool>(),
            proptest::bool::weighted(0.1),
            proptest::bool::weighted(0.1),
            proptest::bool::weighted(0.1),
        ),
    )
        .prop_map(
            |(
                nodes,
                root,
                root_lib,
                (cross, release, primary, set_env, buildscriptsources, incremental),
            )| Graph {
                nodes,
                root,
                root_lib,
                cross,
                release,
                primary,
                set_env,
                buildscriptsources,
                incremental,
            },
        )
}

/// Earlier crates, at `of`.
fn earlier(of: &[Index], i: usize) -> Vec<usize> {
    let mut deps: Vec<usize> =
        if i == 0 { vec![] } else { of.iter().map(|x| x.index(i)).collect() };
    deps.sort_unstable();
    deps.dedup();
    deps
}

impl Graph {
    /// Without what leaks host paths: incremental builds, path dependencies' build scripts
    /// and sources. (FIXMEs pinned elsewhere)
    fn without_host_leaks(mut self) -> Self {
        self.incremental = false;
        self.buildscriptsources = false;
        for node in &mut self.nodes {
            if matches!(node.origin, Origin::Path) {
                node.buildrs = None;
            }
        }
        self
    }

    fn deps(&self, i: usize) -> Vec<usize> {
        earlier(&self.nodes[i].deps, i)
    }

    fn build_deps(&self, i: usize) -> Vec<usize> {
        self.nodes[i].buildrs.as_ref().map(|b| earlier(&b.deps, i)).unwrap_or_default()
    }

    /// What a crate needs built first.
    fn needs(&self, i: usize) -> Vec<usize> {
        self.deps(i).into_iter().chain(self.build_deps(i)).collect()
    }

    fn root(&self) -> Vec<usize> {
        earlier(&self.root, self.nodes.len())
    }

    fn root_lib(&self) -> Option<Vec<usize>> {
        self.root_lib.as_ref().map(|of| earlier(of, self.nodes.len()))
    }

    /// When cross-compiling: crates built for the host, being proc-macros or what proc-macros
    /// and build scripts depend on.
    fn on_host(&self) -> Vec<bool> {
        let mut host: Vec<_> = self.nodes.iter().map(|n| n.kind == Kind::ProcMacro).collect();
        for i in (0..self.nodes.len()).rev() {
            let mut deps = self.build_deps(i);
            if host[i] {
                deps.extend(self.deps(i));
            }
            deps.into_iter().for_each(|d| host[d] = true);
        }
        host
    }
}

/// The graph and an order cargo may build it in.
fn graph_and_order() -> impl Strategy<Value = (Graph, Vec<usize>)> {
    graph().prop_perturb(|g, mut rng: TestRng| {
        let (mut order, mut left): (Vec<usize>, Vec<usize>) =
            (vec![], (0..g.nodes.len()).collect());
        while !left.is_empty() {
            let ready: Vec<_> = left
                .iter()
                .copied()
                .filter(|&i| g.needs(i).iter().all(|d| order.contains(d)))
                .collect();
            let next = ready[rng.random_range(0..ready.len())];
            order.push(next);
            left.retain(|&i| i != next);
        }
        (g, order)
    })
}

#[derive(Clone, Copy, Default)]
struct Grow<'a> {
    /// Reverses each crate's `--extern`s.
    externs_reversed: bool,
    /// Set in every call.
    vars: &'a [(&'a str, &'a str)],
}

fn mdid(role: u64, i: usize) -> String {
    format!("{:016x}", (role << 32) + i as u64)
}

/// A tree to grow `g` on, from `host`.
fn tree(g: &Graph, mut host: Host) -> Tree {
    host.profile = if g.release { "release" } else { "debug" }.into();
    let mut t = Tree::on(host);
    if g.set_env {
        t.set_envs = vec!["GITHUB_SHA".into()];
    }
    if !g.primary {
        t.experiments.push("finalpathnonprimary");
    }
    t.experiments.extend(g.buildscriptsources.then_some("buildscriptsources"));
    t.experiments.extend(g.incremental.then_some("incremental"));
    t
}

/// Grows `g` on `t` in `order`, returning each call's Containerfile by its MdId.
#[expect(clippy::too_many_lines)]
fn grow(t: &Tree, g: &Graph, order: &[usize], how: Grow) -> BTreeMap<String, String> {
    let host_too = if g.cross { g.on_host() } else { vec![false; g.nodes.len()] };
    // (pkg, version) of each crate
    let mut names: Vec<(String, String)> = vec![];
    for node in &g.nodes {
        names.push(match &node.name {
            Name::Twin(of, same, _, _) if !names.is_empty() => {
                let (pkg, version) = of.get(&names).clone();
                let version = if *same { version } else { other_version(&pkg, &version) };
                (pkg, version)
            }
            Name::Pkg(pkg, version) | Name::Twin(_, _, pkg, version) => {
                (pkg.clone(), version.clone())
            }
        });
    }

    // A crate's build, for the target (or the host when not cross-compiling) or the host
    let on_target = |i: usize| !g.cross || g.nodes[i].kind != Kind::ProcMacro;
    let lib_mdid = |i: usize, host: bool| mdid(if host && g.cross { 5 } else { 1 }, i);
    let run_mdid = |i: usize, host: bool| mdid(if host && g.cross { 6 } else { 3 }, i);
    let profile = |host: bool| t.host.profile((g.cross && !host).then_some(TRIPLE));
    let target = |host: bool| (g.cross && !host).then_some(TRIPLE);
    let crate_name = |i: usize| names[i].0.replace('-', "_");
    let out_dir = |i: usize, host: bool| {
        format!("{}/build/{}-{}/out", profile(host), names[i].0, run_mdid(i, host))
    };
    let externs = |of: &[usize], host: bool, pipelined: bool| {
        let mut externs: Vec<_> = of
            .iter()
            .map(|&d| {
                let (name, kind) = (crate_name(d), g.nodes[d].kind);
                let host = host || kind == Kind::ProcMacro;
                let ext = kind.extern_ext(pipelined);
                format!("{name}={}/deps/lib{name}-{}.{ext}", profile(host), lib_mdid(d, host))
            })
            .collect();
        if how.externs_reversed {
            externs.reverse();
        }
        externs
    };
    // Native libraries reach whatever depends on their crate, transitively
    // By crate build (crate, for host?)
    let mut natives: BTreeMap<(usize, bool), Vec<(usize, bool)>> = BTreeMap::new();
    // Which build of `d` a build `for_host` (or not) depends on
    let variant =
        |d: usize, for_host: bool| g.cross && (for_host || g.nodes[d].kind == Kind::ProcMacro);
    let src = |i: usize| match g.nodes[i].origin {
        Origin::Registry => Src::Registry,
        Origin::Git(depth) => Src::Git(if depth == 0 { "" } else { &DIRS[i][depth - 1] }),
        Origin::Path => Src::Member(&DIRS[i][1]),
    };
    let mut vars = how.vars.to_vec();
    if g.set_env {
        vars.push(("GITHUB_SHA", "ff4ee9ac9e7f"));
    }

    let mut out = BTreeMap::new();
    let mut built_buildrs = vec![false; g.nodes.len()];
    for &i in order {
        let node = &g.nodes[i];
        let (pkg, version) = (&names[i].0, &names[i].1);
        let src = src(i);
        if let Src::Git(member) = src {
            let dir = if member.is_empty() {
                t.host.checkout()
            } else {
                format!("{}/{member}", t.host.checkout())
            };
            t.git.borrow_mut().push((dir, t.host.fetch_head()));
        }
        if let Src::Member(member) = src {
            t.fs.file(format!("{}/{member}/Cargo.toml", t.host.pwd()), "");
        }
        let local = matches!(src, Src::Member(_));
        let mut variants = vec![];
        if on_target(i) {
            variants.push(false);
        }
        if g.cross && host_too[i] {
            variants.push(true);
        }
        for host in variants {
            let mut crate_vars = vars.clone();
            let own_out_dir = out_dir(i, host);
            let mut args = vec![];
            if let Some(b) = &node.buildrs {
                let built = mdid(2, i);
                t.buildrs.set(if b.legacy { "build_script_main" } else { "build_script_build" });
                if !built_buildrs[i] {
                    built_buildrs[i] = true;
                    let k = Krate {
                        pkg,
                        version,
                        mdid: &built,
                        src,
                        externs: externs(&g.build_deps(i), true, false),
                        vars: &vars,
                        ..Default::default()
                    };
                    out.insert(built.clone(), t.buildrs(k));
                }
                let run = run_mdid(i, host);
                let lib = format!("native_{i}");
                let container_out_dir =
                    own_out_dir.replace(&format!("{}/target/", t.host.pwd()), "/target/");
                let stdout: Vec<String> = if b.native {
                    vec![
                        format!("cargo:rustc-link-lib=static={lib}"),
                        format!("cargo:rustc-link-search=native={container_out_dir}"),
                    ]
                } else {
                    vec![]
                };
                let stdout: Vec<&str> = stdout.iter().map(String::as_str).collect();
                let sets: Vec<(String, String)> = (0..b.sets)
                    .map(|n| (format!("SET_{i}_{n}"), format!("it's \"{n}\" $HOME `x`")))
                    .collect();
                let sets: Vec<(&str, &str)> =
                    sets.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                // `links` metadata, from its direct dependencies
                let dep_vars: Vec<(String, String)> = g
                    .deps(i)
                    .iter()
                    .filter(|&&d| g.nodes[d].buildrs.as_ref().is_some_and(|b| b.links && b.writes))
                    .map(|&d| {
                        (
                            format!("DEP_NATIVE{d}_ROOT"),
                            out_dir(d, host || g.nodes[d].kind == Kind::ProcMacro),
                        )
                    })
                    .collect();
                let mut run_vars = vars.clone();
                run_vars.push(("TARGET", target(host).unwrap_or("x86_64-unknown-linux-gnu")));
                run_vars.extend(dep_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                let native_file = format!("out/lib{lib}.a");
                let writes: Vec<&str> = if b.native {
                    vec![&native_file]
                } else if b.writes {
                    vec!["out/x"]
                } else {
                    vec![]
                };
                let r = Run {
                    pkg,
                    version,
                    built: &built,
                    mdid: &run,
                    src,
                    target: target(host),
                    vars: &run_vars,
                    sets: &sets,
                    writes: &writes,
                    stdout: &stdout,
                };
                out.insert(run.clone(), t.run(r));
                crate_vars.push(("OUT_DIR", &own_out_dir));
                if b.native {
                    natives.entry((i, host)).or_default().push((i, host));
                    args.push(format!("-l static={lib}"));
                }
            }
            let lib = lib_mdid(i, host);
            let mut native_dirs: Vec<(usize, bool)> =
                natives.get(&(i, host)).cloned().unwrap_or_default();
            for d in g.deps(i) {
                native_dirs
                    .extend(natives.get(&(d, variant(d, host))).into_iter().flatten().copied());
            }
            native_dirs.sort_unstable();
            native_dirs.dedup();
            natives.insert((i, host), native_dirs.clone());
            args.extend(native_dirs.iter().map(|&(d, h)| format!("-L native={}", out_dir(d, h))));
            if g.incremental && local {
                args.push(format!(
                    "-C incremental={}/incremental/{}-{lib}",
                    profile(host),
                    crate_name(i)
                ));
            }
            let k = Krate {
                pkg,
                version,
                mdid: &lib,
                src,
                target: target(host),
                crate_types: node.kind.crate_types(),
                externs: externs(&g.deps(i), host, node.kind.pipelined()),
                args: args.join(" "),
                vars: &crate_vars,
                ..Default::default()
            };
            out.insert(lib.clone(), t.compile(k));
        }
    }

    let mut root_externs = externs(&g.root(), false, false);
    let mut root_args: Vec<String> = vec![];
    let natives_of = |of: &[usize]| {
        let mut dirs: Vec<(usize, bool)> = of
            .iter()
            .flat_map(|&d| natives.get(&(d, variant(d, false))).cloned().unwrap_or_default())
            .collect();
        dirs.sort_unstable();
        dirs.dedup();
        dirs
    };
    if let Some(deps) = &g.root_lib() {
        let lib = mdid(7, 0);
        let k = Krate {
            pkg: "mycrate",
            version: "0.1.0",
            mdid: &lib,
            src: Src::Local,
            target: target(false),
            externs: externs(deps, false, true),
            args: natives_of(deps)
                .iter()
                .map(|&(d, h)| format!("-L native={}", out_dir(d, h)))
                .collect::<Vec<_>>()
                .join(" "),
            vars: &vars,
            ..Default::default()
        };
        out.insert(lib.clone(), t.compile(k));
        root_externs.push(format!("mycrate={}/deps/libmycrate-{lib}.rlib", profile(false)));
    }
    let all: Vec<usize> = g.root().into_iter().chain(g.root_lib().into_iter().flatten()).collect();
    root_args.extend(natives_of(&all).iter().map(|&(d, h)| format!("-L native={}", out_dir(d, h))));
    let mut root_vars = vars.clone();
    if g.primary {
        root_vars.push(("CARGO_PRIMARY_PACKAGE", "1"));
    }
    let root = mdid(4, 0);
    let k = Krate {
        pkg: "mycrate",
        version: "0.1.0",
        mdid: &root,
        src: Src::Local,
        target: target(false),
        crate_types: &["bin"],
        externs: root_externs,
        args: root_args.join(" "),
        vars: &root_vars,
        ..Default::default()
    };
    let containerfile = t.compile(k);
    out.insert(root, containerfile);
    out
}

fn natural(g: &Graph) -> Vec<usize> {
    (0..g.nodes.len()).collect()
}

proptest! {
    // Graphs are costly: few cases, unless `$PROPTEST_CASES` asks for more
    #![proptest_config(ProptestConfig::with_cases(
        std::env::var("PROPTEST_CASES").ok().and_then(|n| n.parse().ok()).unwrap_or(24)
    ))]

    /// Any graph: every Containerfile holds to the corpus' invariants ([`super::violations`],
    /// checked by each call), and growing it again changes nothing.
    #[test]
    fn any_graph((g, order) in graph_and_order()) {
        let t = tree(&g, Host::default());
        let first = grow(&t, &g, &order, Grow::default());
        prop_assert_eq!(grow(&t, &g, &order, Grow::default()), first);
    }

    /// Cargo builds in parallel: whichever order, the same Containerfiles.
    #[test]
    fn build_order_does_not_matter((g, order) in graph_and_order()) {
        let a = grow(&tree(&g, Host::default()), &g, &natural(&g), Grow::default());
        prop_assert_eq!(grow(&tree(&g, Host::default()), &g, &order, Grow::default()), a);
    }

    /// FIXME: the order of `--extern`s, though the rustc call is the same, reorders stages and
    /// mounts, so results keyed by Containerfile miss. Only the order differs, though.
    #[test]
    fn extern_order_only_reorders(g in graph()) {
        let a = grow(&tree(&g, Host::default()), &g, &natural(&g), Grow::default());
        let how = Grow { externs_reversed: true, ..Default::default() };
        let b = grow(&tree(&g, Host::default()), &g, &natural(&g), how);
        let lines = |cf: &str| {
            let mut ls: Vec<String> = cf.lines().map(Into::into).collect();
            ls.sort();
            ls
        };
        for (mdid, cf) in &a {
            prop_assert_eq!(lines(cf), lines(&b[mdid]), "{}", mdid);
        }
    }

    /// Two projects building the same crates share every stage, whatever their host's
    /// paths, crates.io index or `$CARGOGREEN_SET_ENVS` of unset variables.
    /// FIXME: not when a listed variable is set but empty, nor when a target's linker is
    /// configured: both reach every crate. (corpus: bottom's `GITHUB_SHA=`, marauder's
    /// `CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER`)
    #[test]
    fn projects_share_stages(
        g in graph(),
        home in "/[a-z]{1,8}(/[a-z]{1,8})?",
        project in "[a-z][a-z_-]{0,8}",
        index in "[0-9a-f]{16}",
        empty_listed in any::<bool>(),
        linker in any::<bool>(),
    ) {
        prop_assume!(home != "/home/u");
        let g = g.without_host_leaks();
        let a = grow(&tree(&g, Host::default()), &g, &natural(&g), Grow::default());

        let mut there = tree(&g, Host { home, project, index, ..Default::default() });
        there.set_envs.extend(["UNSET".into(), "EMPTY".into()]);
        let mut vars = vec![];
        if empty_listed {
            vars.push(("EMPTY", ""));
        }
        if linker {
            vars.push(("CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER", "arm-linux-gnueabihf-gcc"));
        }
        let b = grow(&there, &g, &natural(&g), Grow { vars: &vars, ..Default::default() });

        let leaked = |cf: &str| cf.lines().filter(|l| !l.contains("EMPTY=") && !l.contains("_LINKER=")).collect::<Vec<_>>().join("\n");
        for (mdid, cf) in &a {
            let first_diff = || cf.lines().zip(b[mdid].lines()).find(|(x, y)| x != y);
            prop_assert_eq!(leaked(cf), leaked(&b[mdid]), "{} {:?}", mdid, first_diff());
            prop_assert_eq!(*cf == b[mdid], vars.is_empty(), "{} {:?}", mdid, first_diff());
        }
    }
}
