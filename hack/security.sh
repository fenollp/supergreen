#!/usr/bin/env -S bash -eu
set -o pipefail

# Checks what README's Security section says of sandboxes, with a hostile build.rs and proc-macro.
# Runs real builds with the `cargo-green` found first on $PATH.

[[ $# -ne 0 ]] && echo "Usage: $0" && exit 1

green=$(command -v cargo-green)
CARGO=$(rustup which cargo)
RUSTUP_TOOLCHAIN=$(rustup show active-toolchain | cut -d' ' -f1)
export CARGO RUSTUP_TOOLCHAIN
# Settings that would loosen or skip the sandbox
unset CARGOGREEN_DEPS_NETWORK CARGOGREEN_SET_ENVS CARGOGREEN_RUNNER RUSTC_WRAPPER
# Builds run from the probed project, which logs must stay out of
if [[ -n "${CARGOGREEN_LOG_PATH:-}" ]]; then
	CARGOGREEN_LOG_PATH=$(realpath "$CARGOGREEN_LOG_PATH")
	export CARGOGREEN_LOG_PATH
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
proj=$tmp/proj
mkdir -p "$proj"/src "$proj"/peeper/src

# Secrets a build mustn't get to
canary=$tmp/canary
echo 'private' >"$canary"
export PROBE_SECRET=hunter2
export CARGO_REGISTRY_TOKEN=hunter3

cat >"$proj"/Cargo.toml <<'EOF'
[package]
name = "probe"
version = "0.1.0"
edition = "2021"

[dependencies]
peeper = { path = "peeper" }

[workspace]
EOF

cat >"$proj"/peeper/Cargo.toml <<'EOF'
[package]
name = "peeper"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
EOF

# What both the build.rs and the proc-macro try. The comment keeps caches from answering.
{
	echo "// $tmp"
	echo "const CANARY: &str = \"$canary\";"
	cat <<'EOF'
fn routed() -> bool {
    std::fs::read_to_string("/proc/net/route").is_ok_and(|routes| routes.lines().count() > 1)
}

fn probe() -> String {
    use std::{env, fs, path::Path};
    let report = |what: &str, held: bool| format!("{what}={}\n", if held { "held" } else { "BREACHED" });

    // Tampering. Whether it worked is checked from the outside.
    for dir in ["CARGO_MANIFEST_DIR", "CARGO_HOME"].iter().filter_map(|var| env::var(var).ok()) {
        let _ = fs::write(format!("{dir}/planted"), "planted");
        let _ = fs::write(format!("{dir}/Cargo.toml"), "overwritten");
    }

    let socket = ["/var/run/docker.sock", "/run/docker.sock"].iter().any(|s| Path::new(s).exists());
    [
        report("network", !routed()),
        report("host-file", fs::read(CANARY).is_err()),
        report("env-secret", env::var_os("PROBE_SECRET").is_none()),
        report("registry-token", env::var_os("CARGO_REGISTRY_TOKEN").is_none()),
        report("docker-socket", !socket),
    ]
    .concat()
}
EOF
} >"$proj"/peeper/src/probe.rs

cat >"$proj"/peeper/src/lib.rs <<'EOF'
use proc_macro::TokenStream;

include!("probe.rs");

#[proc_macro]
pub fn report(_: TokenStream) -> TokenStream {
    format!("{:?}", probe()).parse().unwrap()
}
EOF

cat >"$proj"/build.rs <<'EOF'
include!("peeper/src/probe.rs");

fn main() {
    // Not a panic: CI fails on those showing up in logs
    if std::env::var_os("CARGO_PROBE_NEEDS_NETWORK").is_some() && !routed() {
        eprintln!("probe: no network");
        std::process::exit(1);
    }
    for line in probe().lines() {
        println!("cargo:warning=build.rs {line}");
    }
}
EOF

cat >"$proj"/src/main.rs <<'EOF'
fn main() {
    for line in peeper::report!().lines() {
        println!("proc-macro {line}");
    }
}
EOF

(cd "$proj" && cargo generate-lockfile --offline --quiet)

failures=0
fail() {
	echo "FAILED: $*"
	failures=$((failures + 1))
}

# Usage: expect <log> <what=outcome>...  of both the build.rs and the proc-macro
expect() {
	local log=$1 who outcome; shift
	for outcome in "$@"; do
		for who in build.rs proc-macro; do
			grep -qE "(^|: )$who $outcome\$" "$log" || fail "$(basename "$log"): no '$who $outcome'"
		done
	done
}

# Usage: build <label> [VAR=value]...
build() {
	local label=$1; shift
	(cd "$proj" && env CARGO_TARGET_DIR="$tmp/target-$label" "$@" "$green" green build --offline) >"$tmp/$label.log" 2>&1
}

sources() {
	(cd "$proj" && find . -type f -exec sha256sum {} + | sort -k2)
}
cargo_home=${CARGO_HOME:-$HOME/.cargo}


echo '== control: unsandboxed, the probe gets through (else it proves nothing)'
cp -R "$proj" "$tmp"/control
(cd "$tmp"/control && CARGO_HOME="$tmp/control-home" CARGO_TARGET_DIR="$tmp/target-control" cargo build --offline) >"$tmp"/control.log 2>&1 || { cat "$tmp"/control.log; exit 1; }
"$tmp"/target-control/debug/probe >>"$tmp"/control.log
expect "$tmp"/control.log network=BREACHED host-file=BREACHED env-secret=BREACHED registry-token=BREACHED

echo '== sandboxed'
before=$(sources)
build default || { cat "$tmp"/default.log; exit 1; }
"$tmp"/target-default/debug/probe >>"$tmp"/default.log
expect "$tmp"/default.log network=held host-file=held env-secret=held registry-token=held docker-socket=held
[[ "$(sources)" = "$before" ]] || fail 'the build changed its sources'
[[ ! -e "$cargo_home"/planted ]] || fail "the build wrote to $cargo_home"

echo '== sandboxed, granted network: that and nothing else'
build granted CARGOGREEN_DEPS_NETWORK=probe || { cat "$tmp"/granted.log; exit 1; }
"$tmp"/target-granted/debug/probe >>"$tmp"/granted.log
expect "$tmp"/granted.log network=BREACHED host-file=held env-secret=held registry-token=held docker-socket=held

echo '== a step failing in its sandbox is not retried outside of it'
if build denied CARGO_PROBE_NEEDS_NETWORK=1; then
	fail 'a build.rs needing the network ran without being granted it'
elif ! grep -qF 'probe: no network' "$tmp"/denied.log; then
	cat "$tmp"/denied.log
	fail 'the build failed for another reason'
fi


if [[ "$failures" -ne 0 ]]; then
	tail -n +1 "$tmp"/*.log
	echo "$failures security properties do not hold"
	exit 1
fi
echo 'All security properties hold.'
