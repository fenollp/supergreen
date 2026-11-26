/// What holds of every stage across the `recipes/` corpus, checked on each Containerfile
/// made on the host at `home`, built given the build `contexts`. Each violation starts with
/// its kind.
pub(crate) fn violations(containerfile: &str, home: &str, contexts: &[String]) -> Vec<String> {
    let mut found = vec![];
    let mut defined: Vec<&str> = vec![];
    for block in containerfile.split("\nFROM ").skip(1) {
        let (head, body) = block.split_once('\n').unwrap_or((block, ""));
        // Comments aside (eg. `finalpathcomments`' Md dumps)
        let body: String =
            body.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect();
        let body = body.as_str();
        let name = head.rsplit_once(" AS ").map_or("(final)", |(_, name)| name);
        let mut violation = |kind: &str, what: &str| found.push(format!("{kind}: {what} ({name})"));

        if defined.contains(&name) {
            violation("duplicate stage", name);
        }
        let mut dsts: Vec<&str> = vec![];
        for mount in body.split_whitespace().filter_map(|w| w.strip_prefix("--mount=")) {
            let field = |key: &str| mount.split(',').find_map(|kv| kv.strip_prefix(key));
            let from = field("from=").unwrap_or_default();
            if !defined.contains(&from) && !contexts.iter().any(|c| c == from) {
                violation("undefined mount source", from);
            }
            if let Some(dst) = field("dst=") {
                if dsts.contains(&dst) {
                    violation("dst mounted twice", dst);
                }
                dsts.push(dst);
            }
        }
        let mounted =
            |path: &str| dsts.iter().any(|d| path == *d || path.starts_with(&format!("{d}/")));
        let own = body.lines().find_map(|l| l.strip_prefix("WORKDIR ")).unwrap_or("-");

        // RUNs are heredocs, each ended by a line `EOR` (`safeify` keeps values from making one)
        let (opened, ended) =
            (body.matches("<<'EOR'\n").count(), body.lines().filter(|l| *l == "EOR").count());
        if opened != ended {
            violation("heredocs opened and ended", &format!("{opened} and {ended}"));
        }
        if let Some((_, run)) = body.split_once("RUN \\\n") {
            if let Some((_, script)) = run.split_once("<<'EOR'\n") {
                let script: Vec<&str> = script.lines().take_while(|l| *l != "EOR").collect();
                if let Err(e) = shell_words(&script.join("\n").replace("\\\n", "")) {
                    violation("unbalanced quotes", &e);
                }
            } else {
                violation("RUN without heredoc", name);
            }
        }
        // Where a path starts: not inside another one (with home `/cc`: `crates/cc/cc.crate`)
        let at_path_start = |w: &str| {
            w.match_indices(&format!("{home}/"))
                .any(|(at, _)| w[..at].chars().last().is_none_or(|c| "='\",:".contains(c)))
        };
        for word in body.split_whitespace().filter(|w| at_path_start(w)) {
            violation("host path", word);
        }
        for line in body.lines().map(str::trim) {
            if let Some(call) = line.strip_prefix("rustc ") {
                let words: Vec<_> = call.split_whitespace().collect();
                for pair in words.windows(2) {
                    let path = pair[1].split_once("'=").map(|(_, p)| p.trim_end_matches('\''));
                    match (pair[0], path) {
                        ("--extern", Some(p)) if !mounted(p) => violation("unmounted extern", p),
                        ("-L", Some(p))
                            if pair[1].starts_with("native")
                                && p.starts_with("/target/")
                                && !mounted(p) =>
                        {
                            violation("unmounted native", p)
                        }
                        _ => {}
                    }
                }
            } else if line.starts_with("/target/") && line.ends_with(" \\") {
                let exe = line.trim_end_matches(" \\");
                if !mounted(exe) {
                    violation("unmounted exe", exe);
                }
            } else if let Some((var, val)) = line.trim_start_matches("env ").split_once('=') {
                let val = val.trim_end_matches(" \\");
                let is_var =
                    var.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
                if is_var && val.starts_with("/target/") && !mounted(val) && !val.starts_with(own) {
                    violation("unmounted env path", &format!("{var}={val}"));
                }
            }
        }
        defined.push(name);
    }
    found
}

/// Words as `sh` splits them, quotes removed: just what `safeify` produces, with lines
/// continued by a `\` outside quotes.
pub(crate) fn shell_words(text: &str) -> Result<Vec<String>, String> {
    let (mut words, mut word, mut chars) = (vec![], None::<String>, text.chars());
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                let w = word.get_or_insert_default();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => w.push(c),
                        None => return Err("unterminated '".to_owned()),
                    }
                }
            }
            '"' => return Err("unexpected \"".to_owned()), // Never produced
            '\\' => match chars.next() {
                Some('\n') => {} // A line continuation
                c => word.get_or_insert_default().extend(c),
            },
            ' ' | '\t' | '\n' => words.extend(word.take()), // `sh`'s, not Unicode's
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    Ok(words)
}

#[cfg(test)]
mod env_values_for_all {
    use proptest::prelude::*;

    use super::shell_words;
    use crate::wrap::safeify;

    proptest! {
        /// What the container sees of an env var is what cargo set: `safeify`'d into the RUN's
        /// heredoc, then split by `sh`. Newlines too. (corpus: 1243 multi-line
        /// `CARGO_PKG_DESCRIPTION`s)
        #[test]
        fn env_values_reach_the_container(val in "(\\PC|\n|\nEOR\n){0,40}") {
            let line = format!("X={}", safeify(&val).unwrap());
            prop_assert!(line.lines().all(|l| l != "EOR"), "{line}");
            prop_assert_eq!(shell_words(&line).unwrap(), vec![format!("X={val}")]);
        }
    }
}

#[cfg(test)]
mod spotting_violations {
    use crate::wrap::tree::violations;

    #[test]
    fn in_a_containerfile() {
        let containerfile = "\
# syntax=docker.io/docker/dockerfile:1
FROM rust AS rust-base
FROM rust-base AS dep-n-a-1
WORKDIR /target/debug/deps
RUN \\
  --mount=from=out-2,dst=/target/debug/deps/libb-2.rmeta,source=/deps/libb-2.rmeta \\
  --mount=from=out-0,dst=/target/debug/deps/libc-0.rmeta,source=/deps/libc-0.rmeta \\
  --mount=from=out-0,dst=/target/debug/deps/libc-0.rmeta,source=/deps/libc-0.rmeta \\
  <<'EOR'
export \\
  DEP_X_ROOT=/target/debug/build/x-9/out \\
  OUT_DIR=/target/debug/deps/../build/a-1/out \\
  CARGOGREEN=1
rustc --extern d'=/target/debug/deps/libd-3.rmeta' -L native'=/target/debug/build/n-4/out' -L native'=/usr/lib' /home/u/a/src/lib.rs \\
  1>../1 2>../2 || echo $? >../_
EOR
FROM rust-base AS run-z-a-5
RUN \\
  <<'EOR'
export \\
  CARGOGREEN=1
/target/debug/build/a-6/build-script-build \\
  1>../1 2>../2 || echo $? >../_
EOR
FROM rust-base AS run-z-a-5
FROM rust-base AS dep-n-b-7
RUN \\
  <<'EOR'
export \\
  X=it' s \\
  CARGOGREEN=1
EOR
FROM rust-base AS dep-n-c-8
RUN \\
  <<'EOR'
export \\
  X='a
EOR
b' \\
  CARGOGREEN=1
EOR
FROM rust-base AS dep-n-d-9
RUN \\
  <<'EOR'
export \\
  CARGOGREEN=1
";
        assert_eq!(
            violations(containerfile, "/home/u", &[]),
            [
                "undefined mount source: out-2 (dep-n-a-1)",
                "undefined mount source: out-0 (dep-n-a-1)",
                "undefined mount source: out-0 (dep-n-a-1)",
                "dst mounted twice: /target/debug/deps/libc-0.rmeta (dep-n-a-1)",
                "host path: /home/u/a/src/lib.rs (dep-n-a-1)",
                "unmounted env path: DEP_X_ROOT=/target/debug/build/x-9/out (dep-n-a-1)",
                "unmounted extern: /target/debug/deps/libd-3.rmeta (dep-n-a-1)",
                "unmounted native: /target/debug/build/n-4/out (dep-n-a-1)",
                "unmounted exe: /target/debug/build/a-6/build-script-build (run-z-a-5)",
                "duplicate stage: run-z-a-5 (run-z-a-5)",
                "unbalanced quotes: unterminated ' (dep-n-b-7)",
                "heredocs opened and ended: 1 and 2 (dep-n-c-8)",
                "unbalanced quotes: unterminated ' (dep-n-c-8)",
                "heredocs opened and ended: 1 and 0 (dep-n-d-9)",
            ]
        );
    }
}

#[cfg(test)]
mod splitting_shell_words {
    use super::shell_words;

    /// `sh` splits words on ASCII blanks only: an en quad is part of a value.
    #[test]
    fn keeps_unicode_whitespace_in_values() {
        let val = "\u{2000}";
        let line = format!("X={}", crate::wrap::safeify(val).unwrap());
        assert_eq!(shell_words(&line).unwrap(), vec![format!("X={val}")]);
        assert_eq!(shell_words("a\u{2000}b c").unwrap(), ["a\u{2000}b", "c"]);
    }
}
