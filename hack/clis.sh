#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")
source "$repo_root"/hack/ck.sh

# Usage:           $0                              #=> generate CI
#
# Usage:           $0 ( <name@version> | <name> )  #=> cargo install name@version
# Usage:           $0   ok                         #=> cargo install all working bins
#
# Usage:           $0 ( build | package | test )   #=> cargo $cmd ./cargo-green
#
# Usage:    jobs=1 $0 ..                           #=> cargo --jobs=$jobs
# Usage: offline=1 $0 ..                           #=> cargo --frozen (defaults to just: --locked)
# Usage:    rmrf=1 $0 ..                           #=> rm -rf $CARGO_TARGET_DIR/*; cargo ...
# Usage:   reset=1 $0 ..                           #=> docker buildx rm $BUILDX_BUILDER; cargo ...
# Usage:   clean=1 $0 ..                           #=> Both reset=1 + rmrf=1
# Usage:   final=0 $0 ..                           #=> Don't generate final Containerfile
# Usage:   build=0 $0 ..                           #=> Use already installed cargo-green
#
# Usage:          CARGO=.. $0 ..                   #   CARGO='nightly' $0 ..
# Usage:    DOCKER_HOST=.. $0 ..                   #=> Overrides machine
# Usage: BUILDX_BUILDER=.. $0 ..                   #=> Overrides builder (set to "empty" to set BUILDX_BUILDER='')

# TODO: test other runtimes: runc crun containerd buildkit-rootless lima colima
# * CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:buildx-stable-1-rootless"
#   * https://github.com/docker/setup-docker-action testing rootless and containerd
# * a matrix of earlier and earlier versions of: buildkit x buildx/docker x cargo/rustc
# * a local + cached DockerHub proxy

# TODO: set -x in ci

# TODO: set about green's overhead with --timings

# ok: builds | ko: doesn't build | [ok]D: ok|ko but old: shows too many cfg warnings | Ok: takes >=8min in CI
declare -a nvs nvs_args toolchain
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.22.0;          oks[i]=ok; nvs_args[i]='--features=fix' # Flaky and slow
((i+=1)); nvs[i]=cargo-deny@0.18.5;           oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-fuzz@0.13.1;           oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-green@main;            oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main cargo-green'
((i+=1)); nvs[i]=cargo-llvm-cov@0.6.21;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.114;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]='--git https://github.com/cross-rs/cross.git --rev=49cd054de9b832dfc11a4895c72b0aef533b5c6a --bin=cross' # Pinned on 2025/12/03
((i+=1)); nvs[i]=dbcc@2.2.1;                  oks[i]=oD; nvs_args[i]=''
((i+=1)); nvs[i]=diesel_cli@2.3.4;            oks[i]=ok; nvs_args[i]='--no-default-features --features=postgres'
((i+=1)); nvs[i]=hickory-dns@0.26.0-alpha.1;  oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=mussh@3.1.3;                 oks[i]=oD; nvs_args[i]=''
((i+=1)); nvs[i]=ntpd@1.7.1;                  oks[i]=ok; nvs_args[i]='--bin=ntp-daemon'
((i+=1)); nvs[i]=qcow2-rs@0.1.6;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ripgrep@15.1.0;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=rublk@0.2.13;                oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=shpool@0.9.3;                oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=topiary-cli@0.7.3;           oks[i]=Ok; nvs_args[i]=''

#cdylib
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=kD; nvs_args[i]='' # Flaky builds + non-hermetic CARGOGREEN_SET_ENVS='VERGEN_CARGO_TARGET_TRIPLE,VERGEN_BUILD_SEMVER'
((i+=1)); nvs[i]=code_reload@main             oks[i]=ko; nvs_args[i]='--git https://github.com/alordash/code_reload.git --rev=fc16bd2102ea1b59f55563923d6c161684230950 simple' # BUG? doesnt set extrafn
((i+=1)); nvs[i]=stu@0.7.5;                   oks[i]=Ok; nvs_args[i]=''

((i+=1)); nvs[i]=torrust-index@4.0.0-develop; oks[i]=Ok; nvs_args[i]='--git https://github.com/torrust/torrust-index.git --rev=a401c0c62867a7abbf2eee0ca4e7324ab89a1af0 --bin=torrust-index' # Pinned on 2026/04/23
((i+=1)); nvs[i]=cargo-authors@0.5.5;         oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-config2@0.1.39;        oks[i]=ok; nvs_args[i]='--example=get'

# GTK+3
((i+=1)); nvs[i]=rapidraw@main;               oks[i]=ko; nvs_args[i]='--git https://github.com/CyberTimon/RapidRAW.git --tag=v1.4.6 RapidRAW' # Pinned 2025/12/04
# Compiling rawler v0.7.1 (/home/pete/.cargo/git/checkouts/rapidraw-23a119d7e5f78018/b0c070f/src-tauri/rawler/rawler)
# Calling  git config --get remote.origin.url
# Error: Failed getting repository origin url:
# error: could not compile `rawler` (build script)
#==> no gwd + no error

#===> lets recurse up
# Calling  git config --get remote.origin.url in Some("/home/pete/.cargo/git/checkouts/rapidraw-23a119d7e5f78018/b0c070f/src-tauri/rawler/rawler")
# Calling  git config --get remote.origin.url in Some("/home/pete/.cargo/git/checkouts/rapidraw-23a119d7e5f78018/b0c070f/src-tauri/rawler")
# Calling  git config --get remote.origin.url in Some("/home/pete/.cargo/git/checkouts/rapidraw-23a119d7e5f78018/b0c070f/src-tauri")
# Using runner /usr/bin/docker
#====> we find main repo

#=====> it actually uses that first one as submodule
#     b0c070f main λ cat .gitmodules
# [submodule "src-tauri/rawler"]
# 	path = src-tauri/rawler
# 	url = https://github.com/CyberTimon/RapidRAW-DngLab
#      b0c070f main λ pwd
# /home/pete/.cargo/git/checkouts/rapidraw-23a119d7e5f78018/b0c070f
#========> patch all this up together

((i+=1)); nvs[i]=privaxy@main;                oks[i]=ko; nvs_args[i]='--git https://github.com/Barre/privaxy.git --rev=5dad688538bc7397d71d1c9cfd9d9d53bcf68032'
# I 26/02/07 18:43:08.958 Z openssl-sys 0.9.78-d183b817a1884996 appending (AW) to final path /home/runner/work/supergreen/supergreen/recipes/privaxy@main.Dockerfile
# E 26/02/07 18:43:08.958 Z openssl-sys 0.9.78-d183b817a1884996 Error: Runner failed.
# Check logs at /home/runner/work/supergreen/supergreen/logs.txt
# cargo:rustc-cfg=const_fn
# cargo:rustc-cfg=openssl
# cargo:rerun-if-env-changed=X86_64_UNKNOWN_LINUX_GNU_OPENSSL_NO_VENDOR
# X86_64_UNKNOWN_LINUX_GNU_OPENSSL_NO_VENDOR unset
# cargo:rerun-if-env-changed=OPENSSL_NO_VENDOR
# OPENSSL_NO_VENDOR unset
# thread 'main' panicked at /home/runner/.cargo/registry/src/index.crates.io-0000000000000000/openssl-src-111.18.0+1.1.1n/src/lib.rs:496:32:
# called `Result::unwrap()` on an `Err` value: Os { code: 2, kind: NotFound, message: "No such file or directory" }
# note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
# Please report an issue along with information from the following:
# * docker buildx version
# # Pinned on 2025/12/03 # BUG: $CARGO_HOME/registry/src/index.crates.io-0000000000000000/openssl-src-111.18.0+1.1.1n/src/lib.rs:496:32: No such file or directory

((i+=1)); nvs[i]=miri@master;                 oks[i]=ko; nvs_args[i]='--git https://github.com/rust-lang/miri.git --rev=1fe9d5ba386064c14eb517aacfa8e3d5a1acf97c'; toolchain[i]='nightly-2026-03-16' # Pinned on 2026/03/19
# 174 | fn make_miri_codegen_backend(sess: &Session) -> Box<dyn CodegenBackend> {
#     | ----------------------------------------------------------------------- takes 1 argument
# ...
# 285 |         config.make_codegen_backend = Some(Box::new(make_miri_codegen_backend));
#     |                                            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected function that takes 2 arguments
#     |
#     = note: required for the cast from `Box<fn(&Session) -> Box<...> {make_miri_codegen_backend}>` to `Box<dyn FnOnce(&Options, &Target) -> Box<dyn CodegenBackend> + Send>`
#     = note: the full name for the type has been written to '/target/release/deps/miri-e9f47534ee52cbf9.long-type-13241406945400517937.txt'
#     = note: consider using `--verbose` to print the full type name to the console
((i+=1)); nvs[i]=zed@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/zed-industries/zed.git --tag=v0.233.10';
# In file included from /home/pete/.cargo/registry/src/index.crates.io/tree-sitter-0.26.8/src/lib.c:13:
# /home/pete/.cargo/registry/src/index.crates.io/tree-sitter-0.26.8/src/./wasm_store.c:16:10: fatal error: wasm.h: No such file or directory
#    16 | #include <wasm.h>
#       |          ^~~~~~~~
# compilation terminated.

((i+=1)); nvs[i]=verso@main;                  oks[i]=ok; nvs_args[i]='--git https://github.com/versotile-org/verso.git --rev eb719bdd6c7b verso' # Pinned on 2025/12/03
((i+=1)); nvs[i]=cargo-udeps@0.1.60;          oks[i]=Ok; nvs_args[i]=''

((i+=1)); nvs[i]=a-mir-formality@main;        oks[i]=ok; nvs_args[i]='--git https://github.com/rust-lang/a-mir-formality.git --rev=3fc2f38319bb729fbf2f59c38e15e23a9b774716 a-mir-formality' # Pinned 2025/12/03

((i+=1)); nvs[i]=kani-verifier@0.66.0;        oks[i]=ok; nvs_args[i]='--bin=cargo-kani'

((i+=1)); nvs[i]=CreuSAT@master;              oks[i]=ok; nvs_args[i]='--git https://github.com/sarsko/creusat.git --rev=0758fe729d52d8289f3db3508940662e2969ec97' # Pinned on 2025/12/03

((i+=1)); nvs[i]=cargo-make@0.37.24;          oks[i]=ok; nvs_args[i]='--bin=cargo-make'

#rust-toolchain.toml
((i+=1)); nvs[i]=coccinelleforrust@main;      oks[i]=Ko; nvs_args[i]='--git https://gitlab.inria.fr/coccinelle/coccinelleforrust.git --rev=50612e285' # Pinned on 2025/12/03 # Dirty ra_ap_stdx v0.0.312: the environment variable CI changed
((i+=1)); nvs[i]=edit@main;                   oks[i]=ok; nvs_args[i]='--git https://github.com/microsoft/edit --tag=v1.2.1 edit'; toolchain[i]='nightly-2026-03-16' # Pinned 2025/12/04
((i+=1)); nvs[i]=pyrefly@main;                oks[i]=ko; nvs_args[i]='--git https://github.com/facebook/pyrefly --tag=0.44.0'; toolchain[i]='nightly-2025-09-14' # from its rust-toolchain.toml
# running: cd "/tmp/clis-pyrefly_main/release/build/tikv-jemalloc-sys-3de93e63469ff870/out/build" && "make" "-j" "1"
# thread 'main' (6) panicked at /home/pete/.cargo/registry/src/index.crates.io/tikv-jemalloc-sys-0.6.0+5.3.0-1-ge13ca993e8ccb9ba9847cc330696e02839f328f7/build.rs:384:19:
# failed to execute command: No such file or directory (os error 2)

((i+=1)); nvs[i]=ipa@main;                    oks[i]=Ok; nvs_args[i]='--git https://github.com/seekbytes/IPA.git --rev=3094f92' # Pinned on 2025/12/04

((i+=1)); nvs[i]=cargo-tally@1.0.71;          oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-mutants@25.3.1;        oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=binsider@0.3.0;              oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=gifski@1.34.0;               oks[i]=ok; nvs_args[i]=''

#TODO: not a cli but try users of https://github.com/dtolnay/watt `./hack/find.sh rev watt` (no results)
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none

((i+=1)); nvs[i]=nanometers@master;           oks[i]=ko; nvs_args[i]='--git https://github.com/aizcutei/nanometers.git --rev=ca11bbbead' # Pinned 2025/12/04
# error: The platform you're compiling for is not supported by winit
# Maybe on targets? wasm32-unknown-unknown [target.aarch64-apple-darwin] [target.x86_64-apple-darwin]

# TODO: https://belmoussaoui.com/blog/8-how-to-flatpak-a-rust-application/

((i+=1)); nvs[i]=uv@main;                     oks[i]=ko; nvs_args[i]='--git https://github.com/astral-sh/uv.git --rev=2748dce'; toolchain[i]='1.91' # failed to solve: ResourceExhausted: trying to send message larger than max (17778013 vs. 16777216)

((i+=1)); nvs[i]=flamegraph@0.6.10;           oks[i]=ok; nvs_args[i]='--bin=flamegraph'

((i+=1)); nvs[i]=qair@main;                   oks[i]=ok; nvs_args[i]='--git https://codeberg.org/willempx/qair.git --tag=0.7.0'; toolchain[i]='1.78.0' # Pinned 2020/06/14

((i+=1)); nvs[i]=rusty-man@master;            oks[i]=ko; nvs_args[i]='--git https://git.sr.ht/~ireas/rusty-man --tag=v0.5.0'; toolchain[i]='1.78.0' # Pinned 2025/12/04 # BUG: error: couldn't read `src/main.rs`: No such file or directory (os error 2)

((i+=1)); nvs[i]=cargo-osdk@main;             oks[i]=ok; nvs_args[i]='--git=https://github.com/asterinas/asterinas --tag=v0.16.1'

((i+=1)); nvs[i]=fargo@main;                  oks[i]=ok; nvs_args[i]='--git https://fuchsia.googlesource.com/fargo --rev=a7d967b'; toolchain[i]='1.78.0' # Pinned 2025/12/04

((i+=1)); nvs[i]=harper-ls@master;            oks[i]=ok; nvs_args[i]='--git https://github.com/Automattic/harper.git --tag=v1.1.0' # Pinned 2025/12/04

#zstd
((i+=1)); nvs[i]=sccache@0.12.0;              oks[i]=Ok; nvs_args[i]=''

((i+=1)); nvs[i]=gst-plugin-webrtc-signalling@main; oks[i]=ok; nvs_args[i]='--git https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs --tag=gstreamer-1.29.1' # Pinned on 2026/03/21
((i+=1)); nvs[i]=cargo-c@0.10.18+cargo-0.92.0;oks[i]=ok; nvs_args[i]='--bin=cargo-cbuild'

# Depends on https://lib.rs/crates/nvml-wrapper and on https://github.com/nagisa/rust_libloading
((i+=1)); nvs[i]=bottom@0.11.4;               oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=alacritty@0.17.0;            oks[i]=ok; nvs_args[i]=''

((i+=1)); nvs[i]=cargo-rail@0.1.0;            oks[i]=ok; nvs_args[i]=''

# Cross compilation
((i+=1)); nvs[i]=marauder@master;             oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/reMarkable-tools.git --bin=whiteboard --rev=063f8559c --target=armv7-unknown-linux-musleabihf' # Pinned on 2026/06/16

#FIXME: test with Environment: CARGO_BUILD_RUSTC_WRAPPER or RUSTC_WRAPPER  or Environment: CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER or RUSTC_WORKSPACE_WRAPPER
# => the final invocation is $RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC.

#TODO: look into "writing rust tests inside tmux sessions"

header() {
  local page=$1; shift
  [[ $# -eq 0 ]]
  cat <<EOF
on: [push]
name: CLIs p.$page
permissions: {}
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

declare -A binaries=()
populate_binaries() {
  [[ $# -eq 0 ]]
  while read -r dockerfile bin; do
    binaries["${dockerfile%.Dockerfile}"]="$bin"
  done < <(cat docker-bake.json | jq -r '.target | to_entries[] | "\( .value.dockerfile )"+" \( .key )"')
}

as_env() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  case "$name_at_version" in
    alacritty@*) envvars+=(CARGOGREEN_ADD_APT='cmake,g++,libfontconfig1-dev,libxcb-xfixes0-dev,libxkbcommon-dev,pkg-config,python3') ;; # From https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/INSTALL.md#debianubuntu
    bottom@*) envvars+=(CARGOGREEN_SET_ENVS='GITHUB_SHA'); envvars+=(GITHUB_SHA=) ;; # "Dirty bottom v0.11.4: the environment variable GITHUB_SHA changed"
    cargo-authors@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,"libssl-dev(>=3.5)",pkg-config') ;;
    cargo-c@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,"libssl-dev(>=3.5)",pkg-config') ;;
    cargo-llvm-cov@*) envvars+=(CARGOGREEN_COMPONENTS='llvm-tools-preview') ;;
    cargo-udeps@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,libssl-dev=3.5.5-1~deb13u2,pkg-config,zlib1g-dev') ;;
    coccinelleforrust@*) envvars+=(CARGOGREEN_ADD_APT='python3-dev') ;;
    diesel_cli@*) envvars+=(CARGOGREEN_ADD_APT='libpq-dev') ;;
    marauder@*) envvars+=(CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER='arm-linux-gnueabihf-gcc'); envvars+=(CARGOGREEN_ADD_APT='gcc-arm-linux-gnueabihf') ;;
    miri@*) envvars+=(CARGOGREEN_COMPONENTS='llvm-tools-preview,rust-src,rustc-dev'); envvars+=(CARGOGREEN_ADD_APT='build-essential') ;;
    mussh@*) envvars+=(CARGOGREEN_ADD_APT='libsqlite3-dev,"libssl-dev(>=3.5)",pkg-config,zlib1g-dev') ;;
    nanometers@*) envvars+=(CARGOGREEN_ADD_APT='libwayland-dev,libglib2.0-dev,libdbus-1-dev,libpangocairo-1.0-0,libasound2-dev,libcairo2-dev,libpango-1.0-0,libpango1.0-dev,libssl-dev=3.5.5-1~deb13u2,libxcb-render0-dev,libxcb-shape0-dev,libxcb-xfixes0-dev,libxkbcommon-dev,libx11-dev,libxcursor-dev,libxcb1-dev,libxi-dev,libxkbcommon-x11-dev,xvfb') ;;
    ntpd@*) envvars+=(NTPD_RS_GIT_REV=c7945250c378f65f65b2a75748132edf75063b3b); envvars+=(NTPD_RS_GIT_DATE=2025-05-09) ;; # Any commit, just fixed + Time of commit
    privaxy@*) envvars+=(CARGOGREEN_ADD_APT='build-essential,libayatana-appindicator3-dev,libgtk-3-dev,librsvg2-dev,libsoup2.4-dev,libssl-dev=3.5.5-1~deb13u2,pkg-config') ;;
    rapidraw@*) envvars+=(CARGOGREEN_ADD_APT='g++,libgtk-3-dev,libjavascriptcoregtk-4.1-dev,libsoup-3.0-dev,libssl-dev=3.5.5-1~deb13u2,libwebkit2gtk-4.1-dev') ;;
    rublk@*) envvars+=(CARGOGREEN_ADD_APT='libclang-dev') ;;
    sccache@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev=3.5.5-1~deb13u2,pkg-config,zlib1g-dev') ;;
    torrust-index@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev=3.5.5-1~deb13u2,pkg-config,zlib1g-dev') ;;
    zed@*) envvars+=(CARGOGREEN_ADD_APT='build-essential,clang,cmake,curl,elfutils,g++,gcc,gettext-base,git,jq,libasound2-dev,libfontconfig-dev,libgit2-dev,libglib2.0-dev,libsqlite3-dev,libssl-dev=3.5.5-1~deb13u2,libva-dev,libvulkan1,libwayland-dev,libx11-xcb-dev,libxkbcommon-x11-dev,libzstd-dev,lld,llvm,make,musl-dev,musl-tools,pipewire,xdg-desktop-portal') ;; # From https://github.com/zed-industries/zed/blob/v0.233.10/script/linux#L25-L52
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
  if [[ -n "${CARGOGREEN_WITH_NETWORK:-}" ]]; then
    echo Using CARGOGREEN_WITH_NETWORK="$CARGOGREEN_WITH_NETWORK"
    envvars+=(CARGOGREEN_WITH_NETWORK="$CARGOGREEN_WITH_NETWORK")
  fi
  if [[ -n "${CARGOGREEN_COMPONENTS:-}" ]]; then
    echo Using CARGOGREEN_COMPONENTS="$CARGOGREEN_COMPONENTS"
    envvars+=(CARGOGREEN_COMPONENTS="$CARGOGREEN_COMPONENTS")
  fi
  if [[ -n "${CARGOGREEN_ADD_APT:-}" ]]; then
    echo Using CARGOGREEN_ADD_APT="$CARGOGREEN_ADD_APT"
    envvars+=(CARGOGREEN_ADD_APT="$CARGOGREEN_ADD_APT")
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
  sed 's%@%_%g;s%+%_%g;s%\.%-%g;s%/%%g;s%:%%g' <<<"$name_at_version"
}

cli() {
  local name_at_version=$1; shift
  local binname=$1; shift
  local cargo=$1; shift
  local registry=/tmp/.local-registry
  local registry_new=$registry-new
  local root=/tmp
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
      CARGOGREEN_LOG: debug
      CARGOGREEN_LOG_PATH: logs.txt
    needs: bin
    steps:
$(login_to_readonly_hub)
    - uses: $action__setup_rust_toolchain
      with:
        toolchain: \${{ matrix.toolchain }}
        rustflags: ''
        cache-on-failure: true
    - name: Drop Rust annotations
      run: |
        echo '::remove-matcher owner=rust::'
        echo '::remove-matcher owner=rustfmt::'
        echo '::remove-matcher owner=clippy::'


$(restore_bin)
$(restore_builder_data)
    - uses: $action__checkout
      with:
        persist-credentials: false

    - name: Prepare local private registry cache
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' || env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      run: |
        # https://github.com/fenollp/supergreen/actions/caches
        mkdir -p $registry
        mkdir -p $registry_new
    - name: 🔵 Restore local private registry cache
      if: \${{ env.CARGOGREEN_CACHE_FROM_IMAGES != '' || env.CARGOGREEN_CACHE_TO_IMAGES != '' }}
      uses: $action__cache_restore
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

$(cargo_green_setup)
    - name: 🔵 Envs
      run: cargo green supergreen env
    - if: \${{ matrix.toolchain != '$stable' }}
      run: cargo green supergreen show-rust-base 2>/dev/null | grep '\${{ matrix.toolchain }}'
    - run: cargo green supergreen builder
    - name: 🔵 Envs again
      run: cargo green supergreen env

$(cache_usage)
    - name: 🔵 $cargo install
      id: do-try
      timeout-minutes: 11
      continue-on-error: true
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          $cargo green -vv install --locked --force --root=$root $(as_install "$name_at_version") $@ |& tee _
    - name: 🔵 $cargo install jobs=1
      id: do-try-jobs1
      timeout-minutes: 11
      if: \${{ job.steps.do-try.outcome != 'success' }}
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          $cargo green -vv install --jobs=1 --locked --force --root=$root $(as_install "$name_at_version") $@ |& tee _
    - if: \${{ always() && matrix.toolchain != '$stable' }}
      uses: $action__upload_artifact
      name: Upload recipe
      with:
        name: $name_at_version.Dockerfile
        path: \${{ env.CARGOGREEN_FINAL_PATH }}
        if-no-files-found: error
$(postconds _)
$(cache_usage)
$(disk_usage)
$(check_bin_help_and_set_hash "$root" "$binname")

    - name: 🔵 Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          $cargo green -vv install --locked --force --root=$root $(as_install "$name_at_version") $@ |& tee _
$(postcond_fresh _)
$(postconds _)
$(check_bin_hash "$root" "$binname")

    - name: 🔵 Try CARGOGREEN_RUNNER=none with the same command twice without modifications...
      run: |
$(unset_action_envs)
        env ${envvars[@]} CARGOGREEN_RUNNER=none \\
          $cargo green -vv install --locked --force --root=$root $(as_install "$name_at_version") $@ |& tee _
$(postcond_fresh _)
$(postconds _)
$(check_bin_hash "$root" "$binname")

    - run: rm -rf $root/bin/* \$CARGO_TARGET_DIR/* >/dev/null
    - name: 🔵 Reuse locally cached results...
      run: |
$(unset_action_envs)
        env ${envvars[@]} CARGOGREEN_RUNNER=none \\
          $cargo green -vv install --locked --force --root=$root $(as_install "$name_at_version") $@ |& tee _
    - name: ...is blazingly fearlessly lightspeed fast
      run: |
        grep Finished ./_ || exit 1
        grep Finished ./_ | grep -E ....s || exit 1
$(postconds _)
$(check_bin_hash "$root" "$binname")

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
  # - name: Save local private registry cache
  #   uses: actions/cache/save@v5
  #   if: \${{ false }} # TODO: drop when digests are stable
  #   with:
  #     path: $registry
  #     key: localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ github.job }}-\${{ github.run_id }}

$(cache_usage)
$(disk_usage)

EOF
}

# No args: generate CI files
#   debug webui at https://github.com/fenollp/supergreen/actions/workflows/clis-1.yml
#                  https://github.com/fenollp/supergreen/actions/workflows/clis-2.yml
if [[ $# = 0 ]]; then
  populate_binaries
  page=1 ; perpage=0 ; actual=0 ; declare -a slows
  for i in "${!nvs[@]}"; do
    name_at_version=${nvs["$i"]}
    o=${oks[$i]}
    case "${o:0:1}" in
        O) slows[i]=recipes/$name_at_version.Dockerfile ; continue ;;
        o) ((actual+=1)) ;; # Skip big Os: they take too long
        *) continue ;;
    esac
    case "$name_at_version" in
      cargo-green@*) continue ;;
    esac
    cargo=${toolchain["$i"]:-''}
    if [[ "$cargo" = '' ]]; then
      cargo=cargo
    else
      cargo="cargo +$cargo"
    fi
    ((perpage+=1))
    [[ $perpage = 10 ]] && { perpage=1 ; ((page+=1)) ; }
    [[ $perpage = 1 ]] && header $page | tee .github/workflows/clis-$page.yml
    cli "$name_at_version" "${binaries["$name_at_version"]}" "$cargo" "${nvs_args["$i"]}" | tee --append .github/workflows/clis-$page.yml
  done

  echo
  echo Too slow to use:
  for slow in "${!slows[@]}"; do
    echo "${slows[$slow]}"
  done | sort
  echo Produced: "$actual" jobs in "$page" workflows
  exit
fi


arg1=$1; shift

rmrf=${rmrf:-0}
reset=${reset:-0}
[[ "${clean:-0}" = 1 ]] && rmrf=1 && reset=1
jobs=${jobs:-''} ; [[ "$jobs" != '' ]] && jobs="--jobs=$jobs"
frozen=--locked ; [[ "${offline:-}" = '1' ]] && frozen=--frozen
final=${final:-1}
build=${build:-1}

case "${BUILDX_BUILDER:-}" in
  '') BUILDX_BUILDER=supergreen ;;
  'empty') BUILDX_BUILDER= ;;
  *) ;;
esac

install_dir=$repo_root/target

# Ad-hoc $PATH otherwise macOS has troubles with string length
shortPATH=$install_dir/bin
for cmd in cargo docker; do
  shortPATH="$shortPATH:"$(dirname "$(which $cmd)")""
done
while read -d: -r path; do
  if ! [[ "$shortPATH" =~ (^"$path":|:"$path":|:"$path"$) ]]; then
    shortPATH="$shortPATH:$path"
  fi
done <<<"$PATH"


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

  build | package | test)
set -x
    # Keep target dir within PWD so it emulates a local build somewhat
    tmptrgt=$PWD/target/tmp-$arg1
    tmplogs=$tmptrgt.logs.txt
    mkdir -p "$tmptrgt"
    if [[ "$rmrf" = '1' ]]; then rm -rf "$tmptrgt"/* || exit 1; fi
    if [[ "$reset" = 1 ]]; then docker buildx rm "$BUILDX_BUILDER" --force || exit 1; fi
    if [[ "$build" = 1 ]]; then CARGO_TARGET_DIR=$install_dir cargo install $frozen --force --root=$install_dir --path="$PWD"/cargo-green || exit 1; fi
    ls -lha $install_dir/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs

    # case "$OSTYPE" in
    #   darwin*) osascript -e "$(printf 'tell app "Terminal" \n do script "tail -f %s" \n end tell' $tmplogs)" ;;
    #   *)       xdg-terminal-exec tail -f $tmplogs ;;
    # esac

    cargo=cargo ; [[ "${CARGO:-}" != '' ]] && cargo="cargo +$CARGO"

    echo "$arg1"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    CARGOGREEN_LOG=debug \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green-fetched.Dockerfile" \
    CARGOGREEN_EXPERIMENT=finalpathnonprimary \
    PATH=$shortPATH \
      $cargo green fetch
    CARGOGREEN_LOG=debug \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green.Dockerfile" \
    CARGOGREEN_EXPERIMENT=finalpathnonprimary \
    PATH=$shortPATH \
    CARGO_TARGET_DIR="$tmptrgt" \
      $cargo green -vv $arg1 $jobs --all-features $frozen -p cargo-green
    exit ;;

  *)
    # Matching first arg:
    picked=-1
    for i in "${!nvs[@]}"; do
      case "${nvs[$i]}" in
        *"$arg1"*) picked=$i; break ;;
      esac
    done
    if [[ "$picked" = -1 ]]; then
      echo "Could not match '$arg1' among:"
      for i in "${!nvs[@]}"; do
        echo "${nvs[$i]}" "${nvs_args[$i]}"
      done
      exit 1
    fi
    name_at_version=${nvs[$i]}
    args=${nvs_args[$i]}
    cargo="cargo +${toolchain["$i"]:-${CARGO:-$fixed}}"
    timings=; [[ '1.61' = "$(printf "%s\n1.61\n" "${cargo/cargo +}" | sort -V | head -n1)" ]] && timings=--timings
    ;;
esac

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
    '&&' 'if' '[[ ' "$build" = 1 ']];' 'then' CARGO_TARGET_DIR=$install_dir cargo install $frozen --force --root=$install_dir --path="$gitdir"/cargo-green '||' 'exit' '1;' 'fi' \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

# RUSTFLAGS="--remap-path-prefix=$tmptrgt="

envvars=(CARGO_INCREMENTAL=0)
envvars+=(PATH=$shortPATH)
envvars+=(CARGOGREEN_LOG=debug)
envvars+=(CARGOGREEN_LOG_PATH="$tmplogs")
envvars+=(CARGO_TARGET_DIR="$tmptrgt")
if [[ "$final" = '1' ]]; then
  envvars+=(CARGOGREEN_FINAL_PATH=recipes/$name_at_version.Dockerfile)
  envvars+=(CARGOGREEN_EXPERIMENT=finalpathnonprimary) #,finalpathcomments)
fi
as_env "$name_at_version"
send \
  'until' '[[' -f "$tmpgooo".installed ']];' 'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* \
  '&&' 'if' '[[' "$reset" '=' '1' ']];' 'then' docker buildx rm "$BUILDX_BUILDER" --force '||' 'exit' '1;' 'fi' \
  '&&' "${envvars[@]}" $cargo green -vv install $timings $jobs --root=$tmpbins $frozen --force "$(as_install "$name_at_version")" "$args" \
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
