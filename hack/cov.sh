#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

# Code coverage of cargo-green, merged across runs: unit tests and real builds alike.
# Needs: cargo install --locked cargo-llvm-cov && rustup component add llvm-tools-preview
#
# Usage: $0 clean                #=> Drop all profiles (and instrumented builds)
# Usage: $0 test                 #=> Run unit tests, keeping their profiles
# Usage: $0 install <dir>        #=> Put an instrumented cargo-green in <dir> (e.g. target/bin)
# Usage: $0 env                  #=> Print LLVM_PROFILE_FILE, to set when running that binary
# Usage: $0 report [<args>..]    #=> Merge all profiles: a summary, or e.g. --html, --lcov --output-path lcov.info
#
# E.g. ./hack/clis.sh buildxargs && ./hack/clis.sh build && ./hack/cov.sh report --html
# (./hack/clis.sh uses an instrumented cargo-green unless cov=0)
#
# Profiles land in target/llvm-cov-target/, where `cargo llvm-cov` looks for them. Profiles from
# elsewhere (e.g. CI artifacts) merge just as well once copied in there.

profiles=$repo_root/target/llvm-cov-target

case "${1:-}" in
  clean)
    cargo llvm-cov clean --workspace ;;

  test)
    cargo llvm-cov nextest --all-features --locked --no-report ;;

  install)
    [[ $# -eq 2 ]]
    if ! cargo llvm-cov --version >/dev/null 2>&1; then
      echo 'Missing cargo-llvm-cov: cargo install --locked cargo-llvm-cov && rustup component add llvm-tools-preview (or set cov=0)' >&2
      exit 1
    fi
    # Builds then runs it once: it refuses llvm-cov's $RUSTC_WRAPPER, which is fine.
    cargo llvm-cov run --no-report --all-features --locked --bin cargo-green -- --version >/dev/null 2>&1 || true
    mkdir -p "$2"
    if ! cmp -s "$profiles"/debug/cargo-green "$2"/cargo-green; then
      cp "$profiles"/debug/cargo-green "$2"/.cargo-green.$$
      mv -f "$2"/.cargo-green.$$ "$2"/cargo-green
    fi ;;

  env)
    # Absolute, as cargo-green runs from many dirs. %8m: merged online into 8 files at most,
    # however many rustc calls there are (one process each).
    echo "LLVM_PROFILE_FILE=$profiles/run-%8m.profraw" ;;

  report)
    shift
    [[ $# -eq 0 ]] && set -- --summary-only
    # Leave out the test harness itself
    cargo llvm-cov report --ignore-filename-regex 'wrap/tree/' "$@" ;;

  *)
    sed -n '/^# Usage/p' "$0"
    exit 1 ;;
esac
