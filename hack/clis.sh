#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")
source "$repo_root"/hack/ck.sh

# Usage:           $0                              #=> generate CI
#
# Usage:           $0 ( <name@version> | <name> )  #=> cargo install name@version
# Usage:           $0   ok                         #=> cargo install all working bins
#
# Usage:           $0 ( build | test )             #=> cargo build ./cargo-green
#
# Usage:    jobs=1 $0 ..                           #=> cargo --jobs=$jobs
# Usage: offline=1 $0 ..                           #=> cargo --frozen (defaults to just: --locked)
# Usage:    rmrf=1 $0 ..                           #=> rm -rf $CARGO_TARGET_DIR/*; cargo ...
# Usage:   reset=1 $0 ..                           #=> docker buildx rm $BUILDX_BUILDER; cargo ...
# Usage:   clean=1 $0 ..                           #=> Both reset=1 + rmrf=1
# Usage:   final=0 $0 ..                           #=> Don't generate final Containerfile
#
# Usage:    DOCKER_HOST=.. $0 ..                   #=> Overrides machine
# Usage: BUILDX_BUILDER=.. $0 ..                   #=> Overrides builder (set to "empty" to set BUILDX_BUILDER='')

# TODO: test other runtimes: runc crun containerd buildkit-rootless lima colima
# * CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:buildx-stable-1-rootless"
#   * https://github.com/docker/setup-docker-action testing rootless and containerd
# * a matrix of earlier and earlier versions of: buildkit x buildx/docker x cargo/rustc
# * a local + cached DockerHub proxy

# TODO: set -x in ci

# TODO: set about green's overhead with --timings

# ok: builds | ko: doesn't build | [ok]D: ok|ko but old: shows too many cfg warnings | Ok: takes >=10min in CI
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.22.0;          oks[i]=kO; nvs_args[i]='--features=fix' # Flaky and slow
((i+=1)); nvs[i]=cargo-deny@0.18.5;           oks[i]=Ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-fuzz@0.13.1;           oks[i]=ko; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-green@main;            oks[i]=ko; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main cargo-green' # BUG: couldn't read `cargo-green/src/main.rs`: No such file or directory (os error 2)
((i+=1)); nvs[i]=cargo-llvm-cov@0.6.21;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.114;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]='--git https://github.com/cross-rs/cross.git --rev=49cd054de9b832dfc11a4895c72b0aef533b5c6a cross' # Pinned on 2025/12/03
((i+=1)); nvs[i]=dbcc@2.2.1;                  oks[i]=oD; nvs_args[i]=''
((i+=1)); nvs[i]=diesel_cli@2.3.4;            oks[i]=ok; nvs_args[i]='--no-default-features --features=postgres'
((i+=1)); nvs[i]=hickory-dns@0.26.0-alpha.1;  oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=mussh@3.1.3;                 oks[i]=oD; nvs_args[i]=''
((i+=1)); nvs[i]=ntpd@1.7.0-alpha.20251003;   oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=qcow2-rs@0.1.6;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ripgrep@15.1.0;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=rublk@0.2.13;                oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=shpool@0.9.3;                oks[i]=ok; nvs_args[i]=''

#cdylib
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=kD; nvs_args[i]='' # Flaky builds + non-hermetic CARGOGREEN_SET_ENVS='VERGEN_CARGO_TARGET_TRIPLE,VERGEN_BUILD_SEMVER'
((i+=1)); nvs[i]=code_reload@main             oks[i]=ko; nvs_args[i]='--git https://github.com/alordash/code_reload.git --rev=fc16bd2102ea1b59f55563923d6c161684230950 simple' # Pinned on 2025/12/03 # BUG: couldn't read `$CARGO_HOME/git/checkouts/code_reload-a4960c8e3a9a144c/fc16bd2/src/code_reload_core/src/lib.rs`: No such file or directory (os error 2)
((i+=1)); nvs[i]=stu@0.7.5;                   oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=torrust-index@3.0.0-develop; oks[i]=ko; nvs_args[i]='--git https://github.com/torrust/torrust-index.git --rev=f9c17f3d6f37b949101df3a5d4b4384c641ff929' # Pinned on 2025/12/03 # use of unresolved module or unlinked crate `reqwest`
((i+=1)); nvs[i]=cargo-authors@0.5.5;         oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-config2@0.1.39;        oks[i]=ok; nvs_args[i]='--example=get'
((i+=1)); nvs[i]=privaxy@main;                oks[i]=ko; nvs_args[i]='--git https://github.com/Barre/privaxy.git --rev=5dad688538bc7397d71d1c9cfd9d9d53bcf68032 privaxy' # Pinned on 2025/12/03 # BUG: $CARGO_HOME/registry/src/index.crates.io-0000000000000000/openssl-src-111.18.0+1.1.1n/src/lib.rs:496:32: No such file or directory

((i+=1)); nvs[i]=miri@master;                 oks[i]=ko; nvs_args[i]='--git https://github.com/rust-lang/miri.git --rev=092a83d273087c4f9dd7f1e34a0cd1916819c674' # Pinned on 2025/12/03 # can't find crate for `rustc_errors`
((i+=1)); nvs[i]=zed@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/zed-industries/zed.git --tag=v0.215.3-pre zed' # Pinned on 2025/12/03 # BUG: error: couldn't read `crates/collections/src/collections.rs`: No such file or directory (os error 2)
((i+=1)); nvs[i]=verso@main;                  oks[i]=kD; nvs_args[i]='--git https://github.com/versotile-org/verso.git --rev eb719bdd6c7b verso' # Pinned on 2025/12/03 # use of unresolved module or unlinked crate `arboard`
((i+=1)); nvs[i]=cargo-udeps@0.1.60;          oks[i]=ko; nvs_args[i]='' # extern location for cargo does not exist: /tmp/clis-cargo-udeps_0-1-60/release/deps/libcargo-71fcb7d73f0f1dfb.rmeta

((i+=1)); nvs[i]=mirai@main;                  oks[i]=ko; nvs_args[i]='--git https://github.com/facebookexperimental/MIRAI.git --rev=8c258d28652c2bf5fbf7b92b7a6d4298d4ae18bc checker' # Pinned on 2025/12/03
#     Updating git repository `https://github.com/facebookexperimental/MIRAI.git`
#     Updating git submodule `git@github.com:microsoft/vcpkg.git`
# error: failed to update submodule `vcpkg`
# Caused by:
#   failed to fetch submodule `vcpkg` from git@github.com:microsoft/vcpkg.git
# Caused by:
#   failed to authenticate when downloading repository
#   * attempted ssh-agent authentication, but no usernames succeeded: `git`
#   if the git CLI succeeds then `net.git-fetch-with-cli` may help here
#   https://doc.rust-lang.org/cargo/reference/config.html#netgit-fetch-with-cli
# Caused by:
#   no authentication methods succeeded

((i+=1)); nvs[i]=a-mir-formality@main;        oks[i]=kD; nvs_args[i]='--git https://github.com/rust-lang/a-mir-formality.git --rev=3fc2f38319bb729fbf2f59c38e15e23a9b774716 a-mir-formality' # Pinned 2025/12/03 # error: cannot export macro_rules! macros from a `proc-macro` crate type currently

#((i+=1)); nvs[i]=kani-verifier@0.66.0;       oks[i]=ok; nvs_args[i]='--bin=cargo-kani --bin=kani'
 ((i+=1)); nvs[i]=kani-verifier@0.66.0;       oks[i]=ok; nvs_args[i]='--bin=cargo-kani'

((i+=1)); nvs[i]=creusat@master;              oks[i]=ko; nvs_args[i]='--git https://github.com/sarsko/creusat.git --rev=0758fe729d52d8289f3db3508940662e2969ec97 CreuSAT' # Pinned on 2025/12/03 # error: couldn't read `CreuSAT/src/lib.rs`: No such file or directory (os error 2)
#80 [checkout-0758fe7-0758fe729d52d8289f3db3508940662e2969ec97 1/1] ADD --keep-git-dir=false   https://github.com/sarsko/creusat.git#0758fe729d52d8289f3db3508940662e2969ec97 /
#80 0.026 Initialized empty Git repository in /var/lib/buildkit/runc-overlayfs/snapshots/snapshots/28181/fs/
#80 0.033 fatal: Not a valid object name 0758fe729d52d8289f3db3508940662e2969ec97^{commit}
#80 7.016 From https://github.com/sarsko/creusat
#80 7.016  * branch              0758fe729d52d8289f3db3508940662e2969ec97 -> FETCH_HEAD
#80 7.019 0758fe729d52d8289f3db3508940662e2969ec97

((i+=1)); nvs[i]=cargo-make@0.37.24;          oks[i]=ko; nvs_args[i]='' # BUG confused by 2 versions of same crate: struct takes 3 generic arguments but 2 generic arguments were supplied

#rust-toolchain.toml
((i+=1)); nvs[i]=coccinelleforrust@main;      oks[i]=ko; nvs_args[i]='--git https://gitlab.inria.fr/coccinelle/coccinelleforrust.git --rev=04050b76b coccinelleforrust' # Pinned on 2025/12/03 # TODO: Unable to locate package python3.12-dev => try installing python3.12-dev via "also-run"
((i+=1)); nvs[i]=edit@main;                   oks[i]=ko; nvs_args[i]='--git https://github.com/microsoft/edit --tag=v1.2.1 edit' # Pinned 2025/12/04 # error[E0554]: `#![feature]` may not be used on the stable release channel
# => does toolchain file impact whole project or just primary crate?
((i+=1)); nvs[i]=pyrefly@main;                oks[i]=ko; nvs_args[i]='--git https://github.com/facebook/pyrefly --tag=0.44.0' # Pinned 2025/12/05 # BUG: couldn't read `$CARGO_HOME/git/checkouts/displaydoc-6f27dab09e41f0bc/7dc6e32/src/lib.rs`: No such file or directory

((i+=1)); nvs[i]=ipa@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/seekbytes/IPA.git --rev=3094f92 ipa' # Pinned on 2025/12/04 # BUG couldn't read `$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/khronos_api-3.1.0/api_webgl/extensions/WEBGL_multiview/extension.xml`: No such file or directory (os error 2)

((i+=1)); nvs[i]=cargo-tally@1.0.71;          oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-mutants@25.3.1;        oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=binsider@0.3.0;              oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=gifski@1.34.0;               oks[i]=ok; nvs_args[i]=''

#TODO: not a cli but try users of https://github.com/dtolnay/watt `./hack/find.sh rev watt` (no results)
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none

((i+=1)); nvs[i]=nanometers@master;           oks[i]=ko; nvs_args[i]='--git https://github.com/aizcutei/nanometers.git --rev=ca11bbbead' # Pinned 2025/12/04 # WEIRD: system library `pango` required by crate `pango-sys` was not found.

# TODO: https://belmoussaoui.com/blog/8-how-to-flatpak-a-rust-application/

((i+=1)); nvs[i]=uv@main;                     oks[i]=ko; nvs_args[i]='--git https://github.com/astral-sh/uv.git --rev=2748dce uv' # Pinned 2025/12/04 BUG: couldn't read `crates/uv-macros/src/lib.rs`: No such file or directory

((i+=1)); nvs[i]=flamegraph@0.6.10;           oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=qair@main;                   oks[i]=kD; nvs_args[i]='--git https://codeberg.org/willempx/qair.git --rev=0751f410da' # Pinned 2025/12/04 # conflicting implementations of trait `Trait` for type `(dyn Send + Sync + 'static)` # rustc 1.91.1 too new

((i+=1)); nvs[i]=rusty-man@master;            oks[i]=ko; nvs_args[i]='--git https://git.sr.ht/~ireas/rusty-man --tag=v0.5.0' # Pinned 2025/12/04 # BUG: error: couldn't read `src/main.rs`: No such file or directory (os error 2)

((i+=1)); nvs[i]=asterinas@main;              oks[i]=ko; nvs_args[i]='--git=https://github.com/asterinas/asterinas --tag=v0.16.1 cargo-osdk' # Pinned 2025/12/04 # BUG: couldn't read `$CARGO_HOME/git/checkouts/asterinas-afa2d1b9c5178441/48c7c37/ostd/libs/align_ext/src/lib.rs`: No such file or directory

((i+=1)); nvs[i]=fargo@main;                  oks[i]=kD; nvs_args[i]='--git https://fuchsia.googlesource.com/fargo --rev=a7d967b' # Pinned 2025/12/04 # BUG: couldn't read `src/lib.rs`: No such file or directory

((i+=1)); nvs[i]=rapidraw@main;               oks[i]=ko; nvs_args[i]='--git https://github.com/CyberTimon/RapidRAW.git --tag=v1.4.6 RapidRAW' # Pinned 2025/12/04 # system library `gdk-3.0` required by crate `gdk-sys`

((i+=1)); nvs[i]=harper@master;               oks[i]=ko; nvs_args[i]='--git https://github.com/Automattic/harper.git --tag=v1.1.0 harper-ls' # Pinned 2025/12/04 # BUG: couldn't read `harper-pos-utils/src/lib.rs`: No such file or directory

#zstd
((i+=1)); nvs[i]=sccache@0.12.0;              oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=gst-plugin-webrtc-signalling@main; oks[i]=kD; nvs_args[i]='--git https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs --rev=0a592e9c5649b4099b0ef7c25b6389d4bccea94a' # Pinned on 2025/12/05 # BUG: couldn't read `net/webrtc/protocol/src/lib.rs`: No such file or directory
#((i+=1)); nvs[i]=cargo-c@0.10.18+cargo-0.92.0; oks[i]=ko; nvs_args[i]='' # extern location for cargo does not exist: /tmp/clis-cargo-c_0-10-18+cargo-0-92-0/release/deps/libcargo-398e775d8efe7ba7.rmeta
 ((i+=1)); nvs[i]=cargo-c@0.10.15+cargo-0.90.0; oks[i]=ko; nvs_args[i]='' # extern location for cargo does not exist: /tmp/clis-cargo-c_0-10-15+cargo-0-90-0/release/deps/libcargo-6a92f81c48ba907f.rmeta

# Depends on https://lib.rs/crates/nvml-wrapper and on https://github.com/nagisa/rust_libloading
((i+=1)); nvs[i]=bottom@0.11.4;               oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=cargo-rail@0.1.0;            oks[i]=ko; nvs_args[i]='' # requires rustc 1.91.0 or newer

#FIXME: test with Environment: CARGO_BUILD_RUSTC_WRAPPER or RUSTC_WRAPPER  or Environment: CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER or RUSTC_WORKSPACE_WRAPPER
# => the final invocation is $RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC.

#TODO: look into "writing rust tests inside tmux sessions"

header() {
  [[ $# -eq 0 ]]
  cat <<EOF
on: [push]
name: CLIs
jobs:


$(bin_job)

EOF
}

as_install() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  case "$name_at_version" in
    *@main | *@master) echo "${name_at_version%@*}" ;;
    *) echo "$name_at_version" ;;
  esac
}

as_env() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  case "$name_at_version" in
    bottom@*) envvars+=(CARGOGREEN_SET_ENVS='GITHUB_SHA'); envvars+=(GITHUB_SHA=) ;; # "Dirty bottom v0.11.4: the environment variable GITHUB_SHA changed"
    cargo-authors@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,pkg-config') ;;
    cargo-udeps@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,pkg-config,zlib1g-dev') ;;
    coccinelleforrust@*) envvars+=(CARGOGREEN_ADD_APT='python3-dev') ;;
    diesel_cli@*) envvars+=(CARGOGREEN_ADD_APT='libpq-dev') ;;
    mussh@*) envvars+=(CARGOGREEN_ADD_APT='libsqlite3-dev,libssl-dev,pkg-config,zlib1g-dev') ;;
    nanometers@*) envvars+=(CARGOGREEN_ADD_APT='libcairo2-dev,libpango-1.0-0,libpango1.0-dev,libssl-dev,libxcb-render0-dev,libxcb-shape0-dev,libxcb-xfixes0-dev,libxkbcommon-dev') ;;
    privaxy@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev') ;;
    rublk@*) envvars+=(CARGOGREEN_ADD_APT='libclang-dev') ;;
    sccache@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,pkg-config,zlib1g-dev') ;;
    torrust-index@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,zlib1g-dev') ;;
    *) ;;
  esac

  if [[ -n "${DOCKER_HOST:-}" ]]; then
    echo Using DOCKER_HOST="$DOCKER_HOST"
    envvars+=(DOCKER_HOST="$DOCKER_HOST")
  fi

  if [[ -n "${CARGOGREEN_LOG_PATH:-}" ]]; then
    echo Using CARGOGREEN_LOG_PATH="$CARGOGREEN_LOG_PATH"
    envvars+=(CARGOGREEN_LOG_PATH="$CARGOGREEN_LOG_PATH")
  fi
  if [[ -n "${CARGOGREEN_LOG:-}" ]]; then
    echo Using CARGOGREEN_LOG="$CARGOGREEN_LOG"
    envvars+=(CARGOGREEN_LOG="$CARGOGREEN_LOG")
  fi
  if [[ -n "${CARGOGREEN_LOG_STYLE:-}" ]]; then
    echo Using CARGOGREEN_LOG_STYLE="$CARGOGREEN_LOG_STYLE"
    envvars+=(CARGOGREEN_LOG_STYLE="$CARGOGREEN_LOG_STYLE")
  fi
  if [[ -n "${CARGOGREEN_RUNNER:-}" ]]; then
    echo Using CARGOGREEN_RUNNER="$CARGOGREEN_RUNNER"
    envvars+=(CARGOGREEN_RUNNER="$CARGOGREEN_RUNNER")
  fi
  if [[ -n "${BUILDX_BUILDER:-}" ]]; then
    echo Using BUILDX_BUILDER="$BUILDX_BUILDER"
    envvars+=(BUILDX_BUILDER="$BUILDX_BUILDER")
  fi
  if [[ -n "${CARGOGREEN_BUILDER_IMAGE:-}" ]]; then
    echo Using CARGOGREEN_BUILDER_IMAGE="$CARGOGREEN_BUILDER_IMAGE"
    envvars+=(CARGOGREEN_BUILDER_IMAGE="$CARGOGREEN_BUILDER_IMAGE")
  fi
  if [[ -n "${CARGOGREEN_SYNTAX_IMAGE:-}" ]]; then
    echo Using CARGOGREEN_SYNTAX_IMAGE="$CARGOGREEN_SYNTAX_IMAGE"
    envvars+=(CARGOGREEN_SYNTAX_IMAGE="$CARGOGREEN_SYNTAX_IMAGE")
  fi
  if [[ -n "${CARGOGREEN_REGISTRY_MIRRORS:-}" ]]; then
    echo Using CARGOGREEN_REGISTRY_MIRRORS="$CARGOGREEN_REGISTRY_MIRRORS"
    envvars+=(CARGOGREEN_REGISTRY_MIRRORS="$CARGOGREEN_REGISTRY_MIRRORS")
  fi
  if [[ -n "${CARGOGREEN_CACHE_IMAGES:-}" ]]; then
    echo Using CARGOGREEN_CACHE_IMAGES="$CARGOGREEN_CACHE_IMAGES"
    envvars+=(CARGOGREEN_CACHE_IMAGES="$CARGOGREEN_CACHE_IMAGES")
  fi
  if [[ -n "${CARGOGREEN_CACHE_FROM_IMAGES:-}" ]]; then
    echo Using CARGOGREEN_CACHE_FROM_IMAGES="$CARGOGREEN_CACHE_FROM_IMAGES"
    envvars+=(CARGOGREEN_CACHE_FROM_IMAGES="$CARGOGREEN_CACHE_FROM_IMAGES")
  fi
  if [[ -n "${CARGOGREEN_CACHE_TO_IMAGES:-}" ]]; then
    echo Using CARGOGREEN_CACHE_TO_IMAGES="$CARGOGREEN_CACHE_TO_IMAGES"
    envvars+=(CARGOGREEN_CACHE_TO_IMAGES="$CARGOGREEN_CACHE_TO_IMAGES")
  fi
  if [[ -n "${CARGOGREEN_FINAL_PATH:-}" ]]; then
    echo Using CARGOGREEN_FINAL_PATH="$CARGOGREEN_FINAL_PATH"
    envvars+=(CARGOGREEN_FINAL_PATH="$CARGOGREEN_FINAL_PATH")
  fi
  if [[ -n "${CARGOGREEN_BASE_IMAGE:-}" ]]; then
    echo Using CARGOGREEN_BASE_IMAGE="$CARGOGREEN_BASE_IMAGE"
    envvars+=(CARGOGREEN_BASE_IMAGE="$CARGOGREEN_BASE_IMAGE")
  fi
  if [[ -n "${CARGOGREEN_SET_ENVS:-}" ]]; then
    echo Using CARGOGREEN_SET_ENVS="$CARGOGREEN_SET_ENVS"
    envvars+=(CARGOGREEN_SET_ENVS="$CARGOGREEN_SET_ENVS")
  fi
  if [[ -n "${CARGOGREEN_BASE_IMAGE_INLINE:-}" ]]; then
    echo Using CARGOGREEN_BASE_IMAGE_INLINE="$CARGOGREEN_BASE_IMAGE_INLINE"
    envvars+=(CARGOGREEN_BASE_IMAGE_INLINE="$CARGOGREEN_BASE_IMAGE_INLINE")
  fi
  if [[ -n "${CARGOGREEN_WITH_NETWORK:-}" ]]; then
    echo Using CARGOGREEN_WITH_NETWORK="$CARGOGREEN_WITH_NETWORK"
    envvars+=(CARGOGREEN_WITH_NETWORK="$CARGOGREEN_WITH_NETWORK")
  fi
  if [[ -n "${CARGOGREEN_ADD_APT:-}" ]]; then
    echo Using CARGOGREEN_ADD_APT="$CARGOGREEN_ADD_APT"
    envvars+=(CARGOGREEN_ADD_APT="$CARGOGREEN_ADD_APT")
  fi
  if [[ -n "${CARGOGREEN_ADD_APT_GET:-}" ]]; then
    echo Using CARGOGREEN_ADD_APT_GET="$CARGOGREEN_ADD_APT_GET"
    envvars+=(CARGOGREEN_ADD_APT_GET="$CARGOGREEN_ADD_APT_GET")
  fi
  if [[ -n "${CARGOGREEN_ADD_APK:-}" ]]; then
    echo Using CARGOGREEN_ADD_APK="$CARGOGREEN_ADD_APK"
    envvars+=(CARGOGREEN_ADD_APK="$CARGOGREEN_ADD_APK")
  fi
  if [[ -n "${CARGOGREEN_EXPERIMENT:-}" ]]; then
    echo Using CARGOGREEN_EXPERIMENT="$CARGOGREEN_EXPERIMENT"
    envvars+=(CARGOGREEN_EXPERIMENT="$CARGOGREEN_EXPERIMENT")
  fi
}

slugify() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  sed 's%@%_%g;s%\.%-%g;s%/%%g;s%:%%g' <<<"$name_at_version"
}

ntpd_locked_commit=c7945250c378f65f65b2a75748132edf75063b3b  # Any value, just fixed.
ntpd_locked_date=2025-05-09                                  # Time of commit

cli() {
  local name_at_version=$1; shift
  local registry=/tmp/.local-registry
  local registry_new=$registry-new
  local envvars=()
  as_env "$name_at_version"

	cat <<EOF
$(jobdef "$(slugify "$name_at_version")")
    continue-on-error: \${{ matrix.toolchain != '$stable' }}
    strategy:
      matrix:
        toolchain:
        - $stable
        - $fixed
        exclude:
        - toolchain: \${{ github.ref != 'refs/heads/main' && '$stable' }}
    env:
      CARGO_TARGET_DIR: /tmp/clis-$(slugify "$name_at_version")
    # CARGOGREEN_CACHE_FROM_IMAGES: docker-image://localhost:12345/\${{ github.repository }}
    # CARGOGREEN_CACHE_TO_IMAGES: docker-image://localhost:23456/\${{ github.repository }}
      CARGOGREEN_FINAL_PATH: recipes/$name_at_version.Dockerfile
      CARGOGREEN_EXPERIMENT: finalpathnonprimary # dumps on each build call
      CARGOGREEN_LOG: trace
      CARGOGREEN_LOG_PATH: logs.txt
$(
  case "$name_at_version" in
    ntpd@*)
      printf '      NTPD_RS_GIT_REV: %s\n' "$ntpd_locked_commit"
      printf '      NTPD_RS_GIT_DATE: %s\n' "$ntpd_locked_date"
      ;;
    *) ;;
  esac
)
    needs: bin
    steps:
$(login_to_readonly_hub)
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: \${{ matrix.toolchain }}
        rustflags: ''
        cache-on-failure: true
$(
	case "$name_at_version" in
		cargo-llvm-cov@*) printf '    - run: rustup component add llvm-tools-preview\n' ;;
		*) ;;
	esac
)

$(restore_bin)
$(restore_builder_data)
    - uses: actions/checkout@v6
$(rundeps_versions)

    - name: Prepare local private registry cache
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' || env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: |
        # https://github.com/fenollp/supergreen/actions/caches
        mkdir -p $registry
        mkdir -p $registry_new
    - name: 🔵 Restore local private registry cache
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' || env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      uses: actions/cache/restore@v5
      with:
        path: $registry
        # github.run_id: https://github.com/actions/toolkit/issues/658#issuecomment-2640690759
        key: localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ github.job }}-\${{ github.run_id }}
        restore-keys: |
          localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ github.job }}-
          localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-
          localprivatereg-\${{ runner.os }}-
          localprivatereg-

    - name: Pull regist3 image
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' || env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: |
        false \\
        || docker build --tag regist3 - <<<'FROM docker.io/registry:3' \\
        || docker build --tag regist3 - <<<'FROM mirror.gcr.io/registry:3' \\
        || docker build --tag regist3 - <<<'FROM public.ecr.aws/docker/registry:3' \\
        || exit 1
    - name: Start "cache from" image registry
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' }}
      run: docker run --name=reg-from --rm --detach -p 12345:5000 --user \$(id -u):\$(id -g) -v     $registry:/var/lib/registry regist3
    - name: Start "cache to" image registry
      if: \${{ env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: docker run --name=reg-to   --rm --detach -p 23456:5000 --user \$(id -u):\$(id -g) -v $registry_new:/var/lib/registry regist3

    - name: 🔵 Envs
      run: ~/.cargo/bin/cargo-green green supergreen env
    - if: \${{ matrix.toolchain != '$stable' }}
      run: ~/.cargo/bin/cargo-green green supergreen env CARGOGREEN_BASE_IMAGE | grep '\${{ matrix.toolchain }}'
    - run: ~/.cargo/bin/cargo-green green supergreen builder
    - name: 🔵 Envs again
      run: ~/.cargo/bin/cargo-green green supergreen env

$(cache_usage)
    - name: 🔵 cargo install
      id: cargo-install
      continue-on-error: true
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --locked --force $(as_install "$name_at_version") $@ |& tee _
    - name: 🔵 cargo install jobs=1
      #if: \${{ job.steps.cargo-install.outcome == 'failure' }} this actually hides failure of cargo-install step
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --jobs=1 --locked --force $(as_install "$name_at_version") $@ |& tee _
    - if: \${{ always() && matrix.toolchain != '$stable' }}
      uses: actions/upload-artifact@v6
      name: Upload recipe
      with:
        name: $name_at_version.Dockerfile
        path: \${{ env.CARGOGREEN_FINAL_PATH }}
        if-no-files-found: error
$(postconds _)
$(cache_usage)

    - name: Target dir disk usage
      if: \${{ always() }}
      run: du -sh \$CARGO_TARGET_DIR || true

    - name: 🔵 Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --locked --force $(as_install "$name_at_version") $@ |& tee _
$(postcond_fresh _)
$(postconds _)

    - name: 🔵 Compare old/new local private registry image digests
      if: \${{ always() && env.CARGOGREEN_CACHE_FROM_IMAGES != '' && env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: |
        diff --width=150 -y \\
          <(find $registry/docker/registry/v2/blobs/sha256/??/ -type d | awk -F/ '{print \$NF}' | sort -u) \\
          <(find $registry_new/docker/registry/v2/blobs/sha256/??/ -type d | awk -F/ '{print \$NF}' | sort -u) || true
        du -sh $registry $registry_new || true
    - name: Local private registry cache dance
      if: \${{ always() && env.CARGOGREEN_CACHE_FROM_IMAGES != '' && env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: |
        # [ci: caches keep growing](https://github.com/moby/buildkit/issues/1850)
        docker stop --timeout 10 reg-from reg-to
        rm -rf $registry
        mv $registry_new $registry
    - name: Save local private registry cache
      uses: actions/cache/save@v5
      if: \${{ false }} # TODO: drop when digests are stable
      with:
        path: $registry
        key: localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ github.job }}-\${{ github.run_id }}

$(cache_usage)

    - name: Target dir disk usage
      if: \${{ always() }}
      run: du -sh \$CARGO_TARGET_DIR || true

EOF
}

# No args: generate CI file
if [[ $# = 0 ]]; then
  header

  for i in "${!nvs[@]}"; do
    o=${oks[$i]}
    case "${o:0:1}" in
        o|O) ;;
        *) continue ;;
    esac
    name_at_version=${nvs["$i"]}
    case "$name_at_version" in
      cargo-green@*) continue ;;
    esac
    cli "$name_at_version" "${nvs_args["$i"]}"
  done

  exit
fi


arg1=$1; shift

rmrf=${rmrf:-0}
reset=${reset:-0}
[[ "${clean:-0}" = 1 ]] && rmrf=1 && reset=1
jobs=${jobs:-''} ; [[ "$jobs" != '' ]] && jobs="--jobs=$jobs"
frozen=--locked ; [[ "${offline:-}" = '1' ]] && frozen=--frozen
final=${final:-1}

case "${BUILDX_BUILDER:-}" in
  '') BUILDX_BUILDER=supergreen ;;
  'empty') BUILDX_BUILDER= ;;
  *) ;;
esac

install_dir=$repo_root/target
CARGO=${CARGO:-cargo} ; [[ "$CARGO" = 'cargo' ]] && CARGO="$CARGO +$fixed"

# Special first arg handling..
case "$arg1" in
  # Try all, sequentially
  ok)
    for i in "${!nvs[@]}"; do
      o=${oks[$i]}
      case "${o:0:1}" in
          o|O) ;;
          *) continue ;;
      esac
      nv=${nvs[$i]}
      "$0" "${nv#*@}"
    done
    exit $? ;;

  build | test)
set -x
    # Keep target dir within PWD so it emulates a local build somewhat
    tmptrgt=$PWD/target/tmp-$arg1
    tmplogs=$tmptrgt.logs.txt
    mkdir -p "$tmptrgt"
    if [[ "$rmrf" = '1' ]]; then rm -rf "$tmptrgt"/* || exit 1; fi
    if [[ "$reset" = 1 ]]; then docker buildx rm "$BUILDX_BUILDER" --force || exit 1; fi
    CARGO_TARGET_DIR=$install_dir cargo install $frozen --force --root=$install_dir --path="$PWD"/cargo-green
    ls -lha $install_dir/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs

    case "$OSTYPE" in
      darwin*) osascript -e "$(printf 'tell app "Terminal" \n do script "tail -f %s" \n end tell' $tmplogs)" ;;
      *)     # xdg-terminal-exec tail -f $tmplogs ;;
    esac

    echo "$arg1"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    CARGOGREEN_LOG=trace \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green-fetched.Dockerfile" \
    CARGOGREEN_EXPERIMENT=finalpathnonprimary \
    PATH=$install_dir/bin:"$PATH" \
      $CARGO green -v fetch
    CARGOGREEN_LOG=trace \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green.Dockerfile" \
    CARGOGREEN_EXPERIMENT=finalpathnonprimary \
    PATH=$install_dir/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \
      $CARGO green -v $arg1 $jobs --all-targets --all-features $frozen -p cargo-green
    exit ;;
esac

name_at_version=$arg1

# Matching first arg:
picked=-1
for i in "${!nvs[@]}"; do
  case "${nvs[$i]}" in
    *"$name_at_version"*) picked=$i; break ;;
  esac
done
if [[ "$picked" = -1 ]]; then
  echo "Could not match '$name_at_version' among:"
  for i in "${!nvs[@]}"; do
    echo "${nvs[$i]}" "${nvs_args[$i]}"
  done
  exit 1
fi
name_at_version=${nvs[$i]}
args=${nvs_args[$i]}

session_name=$(slugify "$name_at_version") #$(slugify "${DOCKER_HOST:-}")
tmptrgt=/tmp/clis-$session_name
tmplogs=$tmptrgt.logs.txt
tmpgooo=$tmptrgt.state
tmpbins=/tmp

if [[ "$rmrf" = '1' ]]; then
  rm -rf "$tmptrgt"/*
fi

rm -f "$tmpgooo".*
tmux new-session -d -s "$session_name"
tmux select-window -t "$session_name:0"

send() {
  tmux send-keys " $* && tmux select-layout even-vertical && exit" C-m
}

gitdir=$(realpath "$(dirname "$(dirname "$0")")")
send \
  set -x \
    '&&' CARGO_TARGET_DIR=$install_dir cargo install $frozen --force --root=$install_dir --path="$gitdir"/cargo-green \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

# Ad-hoc $PATH otherwise macOS has troubles with string length
shortPATH=$install_dir/bin
for cmd in cargo docker; do
  shortPATH="$shortPATH:"$(dirname "$(which $cmd)")""
done

envvars=(CARGO_INCREMENTAL=0)
envvars+=(PATH=$shortPATH)
envvars+=(CARGOGREEN_LOG=trace)
envvars+=(CARGOGREEN_LOG_PATH="$tmplogs")
envvars+=(CARGO_TARGET_DIR="$tmptrgt")
if [[ "$final" = '1' ]]; then
  envvars+=(CARGOGREEN_FINAL_PATH=recipes/$name_at_version.Dockerfile)
  envvars+=(CARGOGREEN_EXPERIMENT=finalpathnonprimary)
fi
# envvars+=(CARGOGREEN_SYNTAX_IMAGE=docker-image://docker.io/docker/dockerfile:1@sha256:4c68376a702446fc3c79af22de146a148bc3367e73c25a5803d453b6b3f722fb)
# envvars+=(CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/rust:1.86.0-slim@sha256:3f391b0678a6e0c88fd26f13e399c9c515ac47354e3cadfee7daee3b21651a4f)
as_env "$name_at_version"
send \
  'until' '[[' -f "$tmpgooo".installed ']];' 'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* \
  '&&' 'if' '[[' "$reset" '=' '1' ']];' 'then' docker buildx rm "$BUILDX_BUILDER" --force '||' 'exit' '1;' 'fi' \
  '&&' 'case' "$name_at_version" 'in' ntpd'@*)' export NTPD_RS_GIT_REV=$ntpd_locked_commit '&&' export NTPD_RS_GIT_DATE=$ntpd_locked_date ';;' '*)' ';;' 'esac' \
  '&&' "${envvars[@]}" $CARGO green -vv install --timings $jobs --root=$tmpbins $frozen --force "$(as_install "$name_at_version")" "$args" \
  '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"

if [[ "$final" = '1' ]]; then
  cat recipes/$name_at_version.Dockerfile | sed "s%/home/$USER/%/home/runner/%g" >recipes/$name_at_version.Dockerfile-
  mv recipes/$name_at_version.Dockerfile- recipes/$name_at_version.Dockerfile
fi

echo "$name_at_version"
echo "Target dir: $tmptrgt"
echo "Logs: $tmplogs"
