//! Recipes, as `hack/clis.sh` makes them, replayed: each call cargo made to us goes through
//! the wrapper again, which must generate the stages it generated then. A dev tool:
//!
//! ```shell
//! REPLAY=recipes/vixargs@0.1.0.Dockerfile cargo test replay::any -- --ignored
//! REPLAY=recipes/ cargo test replay::any -- --ignored  # Each recipe in there
//! REPLAY=recipes/marauder@master.Dockerfile REPLAY_STAGES=dep-n-anyhow-1.0.100-b84ff47cda414cbb \
//!   cargo test replay::any -- --ignored  # Only those stages and what they depend on
//! ```
//!
//! Each `dep-*` / `run-*` stage is turned back into the call cargo made: its env and rustc
//! arguments as the shell inside the container sees them, mapped back to host paths.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;

use super::{Host, Tree, checker::shell_words, exec_build_script, known, stage, wrap_rustc};

const RUSTUP_HOME: &str = "/home/u/.rustup";

macro_rules! re {
    ($pat:literal) => {{
        static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
        &*RE
    }};
}

enum Call {
    Rustc {
        stage: String,
        vars: Vec<(String, String)>,
        pwd: String,
        args: Vec<String>,
        writes: Vec<String>,
    },
    Run {
        stage: String,
        vars: Vec<(String, String)>,
        exe: String,
        writes: Vec<String>,
        sets: Vec<(String, String)>,
    },
}

impl Call {
    fn stage(&self) -> &str {
        match self {
            Self::Rustc { stage, .. } | Self::Run { stage, .. } => stage,
        }
    }

    fn vars(&self) -> &[(String, String)] {
        match self {
            Self::Rustc { vars, .. } | Self::Run { vars, .. } => vars,
        }
    }
}

/// What a recipe holds of the calls that made it.
#[derive(Default)]
struct Recipe {
    /// `name-version` of each crates.io crate
    crates: Vec<String>,
    set_envs: Vec<String>,
    /// Checkout dirs, their `FETCH_HEAD` and where that is.
    git: Vec<(String, String, String)>,
    calls: Vec<Call>,
    /// Stages as generated then
    expected: BTreeMap<String, String>,
    /// The whole recipe's stages, in order, when not only some were picked
    order: Vec<String>,
}

/// Container paths back to the host's.
fn host(s: &str) -> String {
    let h = Host::default();
    let s = s.replace("$CARGO_HOME/registry/src/index.crates.io/", &format!("{}/", h.src()));
    let s = s.replace("$CARGO_HOME", &h.cargo_home()).replace("$RUSTUP_HOME", RUSTUP_HOME);
    let target = format!("{}/target/", h.pwd());
    let replaced = re!(r"(^|[^\w.])/target/")
        .replace_all(&s, |c: &regex::Captures<'_>| format!("{}{target}", &c[1]));
    replaced.into_owned()
}

/// `(name, block)`s, each block from its `FROM` line to the next.
fn stages(text: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for block in text.split("\nFROM ").skip(1) {
        let (head, _) = block.split_once('\n').unwrap_or((block, ""));
        if let Some((_, name)) = head.rsplit_once(" AS ") {
            out.push((name.to_owned(), format!("FROM {}\n", block.trim_end())));
        }
    }
    out
}

/// What rustc writes, given the crate types and emitted kinds in `args`.
fn writes(args: &[String], mdid: &str) -> Vec<String> {
    let after = |flag: &'static str| {
        args.iter()
            .zip(args.iter().skip(1))
            .filter(move |(a, _)| *a == flag)
            .map(|(_, v)| v.as_str())
    };
    let name = after("--crate-name").next().unwrap();
    let metadata = after("--emit").any(|e| e.contains("metadata"));
    let mut writes = BTreeSet::new();
    for kind in after("--crate-type") {
        match kind {
            "lib" | "rlib" => {
                writes.insert(format!("lib{name}-{mdid}.rlib"));
                if metadata {
                    writes.insert(format!("lib{name}-{mdid}.rmeta"));
                }
            }
            "proc-macro" | "cdylib" => drop(writes.insert(format!("lib{name}-{mdid}.so"))),
            "staticlib" => drop(writes.insert(format!("lib{name}-{mdid}.a"))),
            _ => drop(writes.insert(format!("{name}-{mdid}"))),
        }
    }
    writes.into_iter().chain([format!("{name}-{mdid}.d")]).collect() // As the result lists them
}

fn var<'a>(vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The call that made the stage `name`, and the variables set after `CARGOGREEN=1`.
fn call(name: &str, block: &str, blocks: &[(String, String)]) -> (Call, Vec<String>) {
    let run = block.split_once("RUN \\\n").unwrap().1;
    let mounts: Vec<&str> = run.lines().filter(|l| l.starts_with("  --mount=")).collect();
    let command: Vec<&str> = run.lines().filter(|l| !l.starts_with("  --mount=")).collect();
    let command = command.join("\n");
    let command = command.split("\n        1>").next().unwrap().trim_end().trim_end_matches('\\');
    // What `env` gets, undoing `safeify`'s newlines
    let words = shell_words(command).unwrap().into_iter().map(|w| w.replace("\\\n", "\n"));
    let words: Vec<String> = words.collect();
    assert_eq!(words[0], "env", "{name}");
    let at = 1 + words[1..].iter().take_while(|w| re!(r"^[A-Z][A-Z0-9_]*=").is_match(w)).count();
    let vars: Vec<(String, String)> = words[1..at]
        .iter()
        .filter_map(|w| w.split_once('='))
        .filter(|(k, _)| *k != "CARGOGREEN")
        .map(|(k, v)| (k.to_owned(), host(v)))
        .collect();
    let after = block.split_once("CARGOGREEN=1").unwrap().1;
    let after = re!(r"(?m)^\s+([A-Z][A-Z0-9_]*)=").captures_iter(after);
    let after = after.map(|c| c[1].to_owned()).collect();
    let mdid = name.rsplit('-').next().unwrap();

    if name.starts_with("run-") {
        // It wrote something iff a crate besides its own mounts its OUT_DIR
        let mount = format!("--mount=from=out-{mdid},dst=");
        let own = Regex::new(&format!(r"OUT_DIR=\S+-{mdid}/out")).unwrap();
        let wrote = blocks.iter().any(|(n, b)| {
            let mounted = b.lines().any(|l| l.contains(&mount) && l.contains("/out,source=/out"));
            n.starts_with("dep-") && mounted && !own.is_match(b)
        });
        let writes = if wrote { vec!["out/x".to_owned()] } else { vec![] };
        let exe = host(&words[at]);
        return (Call::Run { stage: name.into(), vars, exe, writes, sets: vec![] }, after);
    }

    let mut args: Vec<String> = words[at + 1..].iter().map(|a| host(a)).collect();
    // Back to cargo's --extern order, which mounts follow (the call is sorted)
    let dsts: Vec<&str> =
        mounts.iter().filter_map(|m| m.split("dst=").nth(1)?.split(',').next()).collect();
    let target = format!("{}/target/", Host::default().pwd());
    let at_extern: Vec<usize> =
        (1..args.len()).filter(|&k| args[k - 1] == "--extern" && args[k].contains('=')).collect();
    let mut externs: Vec<String> = at_extern.iter().map(|&k| args[k].clone()).collect();
    externs.sort_by_key(|x| {
        let dst = x.split_once('=').unwrap().1.replace(&target, "/target/");
        dsts.iter().position(|d| *d == dst).unwrap_or(usize::MAX)
    });
    at_extern.into_iter().zip(externs).for_each(|(k, x)| args[k] = x);

    let writes = writes(&args, mdid);
    let workdirs: Vec<&str> = block.lines().filter_map(|l| l.strip_prefix("WORKDIR ")).collect();
    let pwd = match workdirs[..] {
        [_, cwd, ..] => host(cwd),
        _ => var(&vars, "CARGO_MANIFEST_DIR").unwrap().to_owned(),
    };
    (Call::Rustc { stage: name.into(), vars, pwd, args, writes }, after)
}

/// The stages `roots` and all they depend on.
fn closure(blocks: &[(String, String)], roots: &[&str]) -> BTreeSet<String> {
    let by_mdid: BTreeMap<&str, &str> = blocks
        .iter()
        .filter(|(n, _)| n.starts_with("dep-") || n.starts_with("run-"))
        .map(|(n, _)| (n.rsplit('-').next().unwrap(), n.as_str()))
        .collect();
    let text: BTreeMap<&str, &str> = blocks.iter().map(|(n, b)| (n.as_str(), b.as_str())).collect();
    let (mut keep, mut todo) = (BTreeSet::new(), roots.to_vec());
    while let Some(name) = todo.pop() {
        if !keep.insert(name.to_owned()) {
            continue;
        }
        let block = text[name];
        let deps = re!(r"--mount=from=out-([0-9a-f]{16}),").captures_iter(block);
        todo.extend(deps.map(|c| by_mdid[&c[1]]));
        if name.starts_with("dep-")
            && let Some(c) = re!(r"OUT_DIR=\S+-([0-9a-f]{16})/out").captures(block)
        {
            todo.push(by_mdid[&c[1]]);
        }
    }
    keep
}

fn recipe(text: &str, roots: &[&str]) -> Recipe {
    let blocks = stages(text);
    let calling = |n: &String| n.starts_with("dep-") || n.starts_with("run-");
    let keep = if roots.is_empty() {
        blocks.iter().map(|(n, _)| n.clone()).filter(calling).collect()
    } else {
        closure(&blocks, roots)
    };
    let (mut calls, mut afters) = (vec![], vec![]);
    for (name, block) in blocks.iter().filter(|(n, _)| keep.contains(n)) {
        let (call, after) = call(name, block, &blocks);
        calls.push(call);
        afters.push(after);
    }

    // After `CARGOGREEN=1` in a run, there can only be `$CARGOGREEN_SET_ENVS`. In a crate's
    // compilation, the others are what its build script set.
    let mut set_envs = BTreeSet::new();
    for (call, after) in calls.iter().zip(&afters) {
        if let Call::Run { .. } = call {
            set_envs.extend(after.iter().cloned());
        }
    }
    let mut sets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (call, after) in calls.iter().zip(&afters) {
        let out_dir = var(call.vars(), "OUT_DIR");
        let Some(c) = out_dir.and_then(|v| re!(r"-([0-9a-f]{16})/out$").captures(v)) else {
            continue;
        };
        let these = sets.entry(c[1].to_owned()).or_default();
        for name in after.iter().filter(|v| !set_envs.contains(*v)) {
            if !these.iter().any(|(k, _)| k == name) {
                these.push((name.clone(), var(call.vars(), name).unwrap().to_owned()));
            }
        }
    }
    for call in &mut calls {
        if let Call::Run { stage, sets: s, .. } = call {
            *s = sets.remove(stage.rsplit('-').next().unwrap()).unwrap_or_default();
        }
    }

    let heads: BTreeMap<&str, (&str, &str)> = blocks
        .iter()
        .filter_map(|(n, b)| {
            let c = re!(r"ADD --keep-git-dir=false \\\n  (\S+)#([0-9a-f]{40}) /").captures(b)?;
            Some((n.as_str(), (c.get(1)?.as_str(), c.get(2)?.as_str())))
        })
        .collect();
    let mut git = BTreeMap::new();
    for call in &calls {
        let block = &blocks.iter().find(|(n, _)| n == call.stage()).unwrap().1;
        let Some(c) = re!(r"--mount=from=(checkout-\S+?),dst=(\S+?) ").captures(block) else {
            continue;
        };
        let (url, commit) = heads[&c[1]];
        let checkout = host(&c[2]);
        let repo = checkout.split("/git/checkouts/").nth(1).unwrap().split('/').next().unwrap();
        let dir = var(call.vars(), "CARGO_MANIFEST_DIR").unwrap().to_owned();
        let db = format!("{}/git/db/{repo}/FETCH_HEAD", Host::default().cargo_home());
        git.insert(dir.clone(), (dir, format!("{commit}\t\t{url}"), db));
    }

    let mdids: BTreeSet<&str> = keep.iter().map(|n| n.rsplit('-').next().unwrap()).collect();
    let picked =
        |n: &str| keep.contains(n) || n.strip_prefix("out-").is_some_and(|m| mdids.contains(m));
    let expected: BTreeMap<_, _> = blocks.iter().filter(|(n, _)| picked(n)).cloned().collect();
    let order = blocks.iter().map(|(n, _)| n.clone()).filter(|n| roots.is_empty() && picked(n));
    let crates = re!(r"https://static.crates.io/crates/\S+?/(\S+)\.crate").captures_iter(text);
    Recipe {
        crates: crates.map(|c| c[1].to_owned()).collect::<BTreeSet<_>>().into_iter().collect(),
        set_envs: set_envs.into_iter().collect(),
        git: git.into_values().collect(),
        calls,
        expected,
        order: order.collect(),
    }
}

/// A stage as it compares: without the recipe's trailing comments.
fn clean(block: &str) -> String {
    let lines = block.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty());
    lines.map(|l| format!("{}\n", l.trim_end())).collect()
}

fn replay(r: Recipe) {
    let mut t = Tree::new();
    t.tolerate = vec![known::LINKS_OUT_DIR_UNMOUNTED.into()];
    t.set_envs = r.set_envs;
    for c in &r.crates {
        t.fs.file(format!("{}/{c}.crate", t.host.cache()), "");
    }
    for (dir, head, db) in r.git {
        t.fs.file(&db, head);
        t.git.borrow_mut().push((dir, db));
    }

    let mut last = String::new();
    for call in r.calls {
        let vars: Vec<_> = call.vars().iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let green = t.green(&vars);
        let name = call.stage().to_owned();
        last = match call {
            Call::Rustc { pwd, args, writes, .. } => t.call(("", ""), &writes, async || {
                wrap_rustc(green, args, pwd.as_str().into(), async { unreachable!() }).await
            }),
            Call::Run { exe, writes, sets, .. } => {
                let stdout = sets.iter().map(|(k, v)| format!("cargo:rustc-env={k}={v}"));
                let stdout: Vec<_> = stdout.collect();
                t.call((&stdout.join("\n"), ""), &writes, async || {
                    exec_build_script(green, exe.as_str().into()).await
                })
            }
        };
        let out = format!("out-{}", name.rsplit('-').next().unwrap());
        for name in [name.as_str(), &out] {
            let Some(expected) = r.expected.get(name) else { continue };
            pretty_assertions::assert_eq!(clean(stage(&last, name)), clean(expected), "{name}");
        }
    }

    if !r.order.is_empty() {
        let names: Vec<_> = last
            .lines()
            .filter_map(|l| l.strip_prefix("FROM ")?.split_once(" AS ").map(|(_, n)| n))
            .filter(|n| r.order.iter().any(|o| o == n))
            .collect();
        assert_eq!(names, r.order, "the whole recipe, in order");
    }
}

/// `$REPLAY`, or each recipe in it.
fn recipes() -> Vec<PathBuf> {
    // Relative to the workspace's root, as tests run from the crate's
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
    let path = root.join(std::env::var("REPLAY").expect("$REPLAY"));
    if !path.is_dir() {
        return vec![path];
    }
    let mut recipes: Vec<_> =
        std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()).collect();
    recipes.retain(|f| f.extension().is_some_and(|ext| ext == "Dockerfile"));
    recipes.sort();
    recipes
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
#[ignore = "set $REPLAY to a recipe, or a dir of them"]
fn any() {
    let only = std::env::var("REPLAY_STAGES").unwrap_or_default();
    let only: Vec<&str> = only.split(',').filter(|s| !s.is_empty()).collect();
    for path in recipes() {
        eprintln!("replaying {}", path.display());
        replay(recipe(&read(&path), &only));
    }
}
