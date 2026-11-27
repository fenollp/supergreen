//! What a build's progress (BuildKit's `--progress=plain`, on STDERR) says (and Podman's).
//!
//! A wrapper's Containerfile holds, before its own stages, those of every crate it depends on:
//! other wrappers just built them, so these steps should all come out `CACHED`.

use std::{collections::BTreeMap, fmt};

use anyhow::{Result, bail};
use log::error;

/// Build contexts (eg. local code tarballs) that are large are suspicious
const MAX_CONTEXT_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Steps {
    /// How many steps were CACHED
    cached: u32,
    /// How many steps actually ran (not CACHED)
    ran: u32,
    /// All this took how long
    ran_ms: u64,
}

/// What one build's progress said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Progress {
    /// Steps of the stages being built: theirs is the work.
    own: Steps,
    /// Steps of the base image's stages: cached, unless a first build (concurrent ones each say
    /// they ran them, as BuildKit runs them once for them all).
    base: Steps,
    /// Steps of the stages of the crates depended on: should all be cached.
    previous: Steps,
    /// Previous steps that ran nonetheless, slowest first: `(ms, "[stage i/n] INSTR")`.
    reran: Vec<(u64, String)>,
    /// Crate sources and such (`ADD`s): those that ran fetched them.
    sources: Steps,
    /// Bytes of each build context sent over to the runner.
    contexts: BTreeMap<String, u64>,
    /// Exporting (results tarball, build cache), in ms.
    exporting_ms: u64,
}

impl Progress {
    /// Fails on a build context larger than [MAX_CONTEXT_BYTES]
    pub(crate) fn check_contexts(&self) -> Result<()> {
        let Some((ctx, bytes)) =
            self.contexts.iter().find(|(_, bytes)| **bytes > MAX_CONTEXT_BYTES)
        else {
            return Ok(());
        };
        error!("build context {ctx:?} weighs {bytes}B, over {MAX_CONTEXT_BYTES}B");
        bail!("Suspicious: build context is quite large: {bytes}B (max: {MAX_CONTEXT_BYTES}B)")
    }

    /// Fails on any step of a previous stage that ran: these were all built by the wrappers of
    /// the crates we depend on, just before.
    pub(crate) fn check_previous(&self) -> Result<()> {
        let Steps { ran, ran_ms, .. } = self.previous;
        if ran != 0 {
            let slowest: Vec<_> =
                self.reran.iter().take(3).map(|(ms, step)| format!("{ms}ms {step}")).collect();
            bail!(
                "{ran} step{s} of previous stages ran again ({ran_ms}ms) instead of being cached: {slowest}",
                s = if ran == 1 { "" } else { "s" },
                slowest = slowest.join(" ; "),
            )
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Own,
    Base,
    Previous,
    Source,
    Exporting,
    Other,
}

/// Parses progress, line by line, for the build of the stages whose names end with `-{suffix}`.
#[derive(Debug)]
pub(crate) struct Parser {
    suffix: String,
    /// Vertex number => its kind and name
    vertices: BTreeMap<u32, (Kind, String)>,
    /// A source's ADD may show as more than one vertex (resolving, then fetching): by name,
    /// whether it was cached and for how long it ran.
    sources: BTreeMap<String, (bool, u64)>,
    progress: Progress,
}

impl Parser {
    #[must_use]
    pub(crate) fn new(suffix: &str) -> Self {
        Self {
            suffix: format!("-{suffix}"),
            vertices: BTreeMap::new(),
            sources: BTreeMap::new(),
            progress: Progress::default(),
        }
    }

    fn kind(&self, name: &str) -> Kind {
        if name.starts_with("exporting ") {
            return Kind::Exporting;
        }
        // [stage-name i/n] INSTR
        let Some(stage) = name.strip_prefix('[').and_then(|rest| rest.split_once([' ', ']']))
        else {
            return Kind::Other;
        };
        let (stage, rest) = stage;
        let instr = rest.split_once("] ").map(|(_, instr)| instr).unwrap_or_default();
        match stage {
            "internal" | "context" => Kind::Other,
            // Resolving an image: always DONE, never CACHED
            _ if instr.starts_with("FROM ") => Kind::Other,
            _ if stage.ends_with(&self.suffix) => Kind::Own,
            _ if instr.starts_with("ADD ") => Kind::Source,
            _ if stage == "xx" || stage.starts_with("rust-") || stage.starts_with("rustup-") => {
                Kind::Base
            }
            // Same instruction atop the base image, whatever the stage: one vertex for all of them,
            // named after whichever came first
            _ if instr.starts_with("WORKDIR ") => Kind::Base,
            _ => Kind::Previous,
        }
    }

    /// Feeds one line (ANSI escapes already stripped).
    pub(crate) fn feed(&mut self, line: &str) {
        let Some((num, rest)) = line.strip_prefix('#').and_then(|line| line.split_once(' ')) else {
            return;
        };
        let Ok(num) = num.parse::<u32>() else { return };

        if let Some(&(kind, ref name)) = self.vertices.get(&num) {
            let name = name.clone();
            self.status(kind, &name, rest);
            return;
        }
        // First line of a vertex: its name
        let kind = self.kind(rest);
        let _ = self.vertices.insert(num, (kind, rest.to_owned()));
    }

    fn status(&mut self, kind: Kind, name: &str, rest: &str) {
        let steps = match kind {
            Kind::Own => &mut self.progress.own,
            Kind::Base => &mut self.progress.base,
            Kind::Previous => &mut self.progress.previous,
            Kind::Source => {
                let (cached, ran_ms) = self.sources.entry(name.to_owned()).or_default();
                if rest == "CACHED" {
                    *cached = true;
                } else if let Some(ms) = rest.strip_prefix("DONE ").and_then(parse_secs) {
                    *ran_ms = (*ran_ms).max(ms);
                }
                return;
            }
            Kind::Exporting | Kind::Other => {
                if let Some(ms) = rest.strip_prefix("DONE ").and_then(parse_secs)
                    && kind == Kind::Exporting
                {
                    self.progress.exporting_ms += ms;
                }
                // e.g. `transferring cargo-green-git: 1.23MB done`
                if let Some(detail) = rest.strip_prefix("transferring ")
                    && let Some((ctx, size)) = detail.split_once(": ")
                    && ctx != "dockerfile"
                    && ctx != "context" // Its .dockerignore: there's no main context
                    && let Some(bytes) = parse_size(size.trim_end_matches(" done"))
                {
                    let _ = self.progress.contexts.insert(ctx.to_owned(), bytes);
                }
                return;
            }
        };
        if rest == "CACHED" {
            steps.cached += 1;
        } else if let Some(ms) = rest.strip_prefix("DONE ").and_then(parse_secs) {
            steps.ran += 1;
            steps.ran_ms += ms;
            if kind == Kind::Previous {
                let short: String = name.chars().take(120).collect();
                self.progress.reran.push((ms, short));
            }
        }
    }

    #[must_use]
    pub(crate) fn finish(mut self) -> Progress {
        self.progress.reran.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        for (cached, ran_ms) in self.sources.into_values() {
            if cached {
                self.progress.sources.cached += 1;
            } else {
                self.progress.sources.ran += 1;
                self.progress.sources.ran_ms += ran_ms;
            }
        }
        self.progress
    }
}

/// `0.7s` (BuildKit's plain progress always shows seconds) => 700
fn parse_secs(txt: &str) -> Option<u64> {
    scaled(txt.strip_suffix('s')?, 1_000)
}

/// `1.234` => 1230 for a `scale` of 1000: two decimals of precision, the third one rounding.
///
/// BuildKit's decimals are all there are: no floats to round, nor to cast back.
fn scaled(decimal: &str, scale: u64) -> Option<u64> {
    let (whole, decimals) = decimal.split_once('.').unwrap_or((decimal, ""));
    let mut digits = decimals.chars().map(|c| c.to_digit(10).map(u64::from));
    let mut next = || digits.next().unwrap_or(Some(0));
    let hundredths = next()? * 10 + next()?;
    let round_up = next()? >= 5;
    for rest in digits {
        rest?; // Even those past the third are digits
    }
    let hundredths =
        whole.parse::<u64>().ok()?.checked_mul(100)? + hundredths + u64::from(round_up);
    hundredths.checked_mul(scale).map(|n| n / 100)
}

/// `2B`, `11.33kB`, `1.2MB`, `3GB` (decimal units)
fn parse_size(txt: &str) -> Option<u64> {
    let digits = txt.find(|c: char| !c.is_ascii_digit() && c != '.')?;
    let (n, unit) = txt.split_at(digits);
    let mult = match unit {
        "B" => 1,
        "kB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        _ => return None,
    };
    scaled(n, mult)
}

impl fmt::Display for Progress {
    /// One line, for logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { own, base, previous, reran, sources, contexts, exporting_ms } = self;
        let steps = |s: &Steps| format!("{} cached, {} ran ({}ms)", s.cached, s.ran, s.ran_ms);
        write!(f, "own steps: {}", steps(own))?;
        write!(f, " · base steps: {}", steps(base))?;
        write!(f, " · previous steps: {}", steps(previous))?;
        write!(f, " · sources: {}", steps(sources))?;
        for (ctx, bytes) in contexts {
            write!(f, " · context {ctx}: {bytes}B sent")?;
        }
        write!(f, " · exporting: {exporting_ms}ms")?;
        if !reran.is_empty() {
            let slowest: Vec<_> =
                reran.iter().take(3).map(|(ms, name)| format!("{ms}ms {name}")).collect();
            write!(f, " · slowest re-ran: {}", slowest.join(", "))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Parser, Steps, parse_secs, parse_size};

    /// As the build of `buildxargs`'s bin went, its lib's stages built by the wrapper just before.
    const LOG: &str = r#"#0 building with "supergreen" instance using docker-container driver
#1 [internal] load build definition from Dockerfile
#1 transferring dockerfile: 11.33kB done
#1 DONE 0.0s
#5 [internal] load .dockerignore
#5 transferring context: 2B done
#5 DONE 0.0s
#6 [context cargo-green-git] load .dockerignore
#6 transferring cargo-green-git: 1.21MB done
#6 DONE 0.1s
#9 [cratesio-buildxargs-1.4.0 1/1] ADD --unpack --checksum=sha256:56c3   https://static.crates.io/crates/buildxargs/buildxargs-1.4.0.crat
#9 DONE 0.0s
#10 [cratesio-buildxargs-1.4.0 1/1] ADD --unpack --checksum=sha256:56c3   https://static.crates.io/crates/buildxargs/buildxargs-1.4.0.crat
#10 CACHED
#18 [rust-base 2/3] RUN   --mount=from=rustup-1.85.1-x86_64-unknown-linux-gnu,source=/rustup-init,dst=/rustup-init   <<'EOR' (/rustup-init
#18 CACHED
#8 [cratesio-pico-args-0.5.0 1/1] ADD --unpack --checksum=sha256:5be1   https://static.crates.io/crates/pico-args/pico-args-0.5.0.crate /
#8 DONE 0.4s
#13 [rust-base 1/3] FROM docker.io/library/debian:trixie-slim@sha256:cedb
#13 resolve docker.io/library/debian:trixie-slim@sha256:cedb 0.0s done
#13 DONE 0.0s
#15 [dep-n-pico-args-0.5.0-93907c7cbcd0b4c5 2/2] RUN   --mount=from=cratesio-pico-args-0.5.0 <<'EOR' (expo
#15 CACHED
#22 [dep-n-buildxargs-1.4.0-9cdea080dba2f69c 2/2] RUN   --mount=from=cratesio-buildxargs-1.4.0
#21 [dep-n-shlex-1.3.0-5407cf27096061a9 1/2] WORKDIR /target/release/deps
#21 DONE 0.3s
#22 0.058 + export CARGO_CRATE_NAME=buildxargs
#22 DONE 2.1s
#23 [dep-n-buildxargs-1.4.0-7039549d07a4a309 2/2] RUN   --mount=from=cratesio-buildxargs-1.4.0
#23 0.058 + export CARGO_BIN_NAME=buildxargs CARGO_CRATE_NAME=buildxargs
#23 DONE 0.7s
#24 exporting to client tarball
#24 sending tarball done
#24 DONE 0.1s
#25 exporting cache to registry
#25 preparing build cache for export 0.3s done
#25 writing layer sha256:4dac88f836e4cdf196795072c058bbc7e245f5747e877c637836f3d05f09c2ed done
#25 DONE 0.4s
"#;

    #[test]
    fn previous_stages_that_ran_again_are_told_apart() {
        let mut parser = Parser::new("7039549d07a4a309");
        LOG.lines().for_each(|line| parser.feed(line));
        let progress = parser.finish();

        assert_eq!(progress.own, Steps { cached: 0, ran: 1, ran_ms: 700 });
        assert_eq!(progress.base, Steps { cached: 1, ran: 1, ran_ms: 300 });
        assert_eq!(progress.previous, Steps { cached: 1, ran: 1, ran_ms: 2_100 });
        assert_eq!(progress.sources, Steps { cached: 1, ran: 1, ran_ms: 400 });
        assert_eq!(progress.contexts, [("cargo-green-git".to_owned(), 1_210_000)].into());
        assert_eq!(progress.exporting_ms, 500);
        assert_eq!(progress.reran.len(), 1);
        assert_eq!(progress.reran[0].0, 2_100);
        assert!(
            progress.reran[0].1.starts_with("[dep-n-buildxargs-1.4.0-9cdea080dba2f69c 2/2] RUN")
        );
    }

    #[test]
    fn previous_stages_must_have_been_cached() {
        let mut parser = Parser::new("7039549d07a4a309");
        LOG.lines().for_each(|line| parser.feed(line));
        let progress = parser.finish();

        let err = progress.check_previous().unwrap_err().to_string();
        assert!(err.starts_with("1 step of previous stages ran again (2100ms)"), "{err}");
        progress.check_contexts().unwrap();
    }

    #[test]
    fn build_contexts_must_be_small() {
        let mut parser = Parser::new("7039549d07a4a309");
        parser.feed("#6 [context my-crate] load from client");
        parser.feed("#6 transferring my-crate: 67.11MB done");
        let err = parser.finish().check_contexts().unwrap_err().to_string();
        assert!(err.contains(" 67110000B (max: 67108864B)"), "{err}");
    }

    #[test]
    fn sizes_and_times() {
        assert_eq!(parse_size("2B"), Some(2));
        assert_eq!(parse_size("11.33kB"), Some(11_330));
        assert_eq!(parse_size("1.2MB"), Some(1_200_000));
        assert_eq!(parse_size("3GB"), Some(3_000_000_000));
        assert_eq!(parse_size("done"), None);
        assert_eq!(parse_secs("0.7s"), Some(700));
        assert_eq!(parse_secs("12.0s"), Some(12_000));
        assert_eq!(parse_secs("12"), None);
        assert_eq!(parse_secs("0.0049s"), Some(0), "two decimals of precision");
        assert_eq!(parse_secs("0.005s"), Some(10), "rounded up from the third");
        assert_eq!(parse_secs("0.014s"), Some(10), "rounded down from the third");
        assert_eq!(parse_secs("1.999s"), Some(2_000));
        assert_eq!(parse_secs("1.2.3s"), None);
        assert_eq!(parse_secs("-1s"), None);
        assert_eq!(parse_secs("18446744073709551615s"), None, "no overflow");
    }
}
