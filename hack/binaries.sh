#!/usr/bin/env -S bash -eu
set -o pipefail

# The list of binaries (`cargo install`ed with cargo-green) and their parameters.
#
# Usage: source $0   #=> defines: nvs oks nvs_args toolchain, bin_envs() as_install() slugify() populate_binaries()
# Usage:        $0 json   #=> JSON array of the binaries to run in CI (the matrix of .github/workflows/clis.yml)
# Usage:        $0 list   #=> One name@version per line, for the binaries to run in CI
#
# Per binary:
#   nvs[i]        name@version (@main / @master for git installs)
#   oks[i]        ok: builds | ko: doesn't build | [ok]D: ok|ko but old: shows too many cfg warnings | Ok: takes >=8min in CI
#   nvs_args[i]   extra `cargo install` args
#   toolchain[i]  optional pinned toolchain
#   bin_envs()    env vars to set for that binary (KEY=VALUE lines)

binaries_repo_root=$(realpath "$(dirname "$(dirname "${BASH_SOURCE[0]}")")")

# ok: builds | ko: doesn't build | [ok]D: ok|ko but old: shows too many cfg warnings | Ok: takes >=8min in CI
declare -a nvs oks nvs_args toolchain
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

((i+=1)); nvs[i]=zed@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/zed-industries/zed.git --tag=v1.0.0';

((i+=1)); nvs[i]=verso@main;                  oks[i]=ok; nvs_args[i]='--git https://github.com/versotile-org/verso.git --rev eb719bdd6c7b verso' # Pinned on 2025/12/03
((i+=1)); nvs[i]=cargo-udeps@0.1.60;          oks[i]=Ok; nvs_args[i]=''

((i+=1)); nvs[i]=a-mir-formality@main;        oks[i]=ok; nvs_args[i]='--git https://github.com/rust-lang/a-mir-formality.git --rev=3fc2f38319bb729fbf2f59c38e15e23a9b774716 a-mir-formality' # Pinned 2025/12/03

((i+=1)); nvs[i]=kani-verifier@0.66.0;        oks[i]=ok; nvs_args[i]='--bin=cargo-kani'

((i+=1)); nvs[i]=CreuSAT@master;              oks[i]=ok; nvs_args[i]='--git https://github.com/sarsko/creusat.git --rev=0758fe729d52d8289f3db3508940662e2969ec97' # Pinned on 2025/12/03

((i+=1)); nvs[i]=cargo-make@0.37.24;          oks[i]=ok; nvs_args[i]='--bin=cargo-make'

#rust-toolchain.toml
((i+=1)); nvs[i]=coccinelleforrust@main;      oks[i]=Ko; nvs_args[i]='--git https://gitlab.inria.fr/coccinelle/coccinelleforrust.git --rev=50612e285' # Pinned on 2025/12/03 # Dirty ra_ap_stdx v0.0.312: the environment variable CI changed
((i+=1)); nvs[i]=edit@main;                   oks[i]=ok; nvs_args[i]='--git https://github.com/microsoft/edit --tag=v1.2.1 edit'; toolchain[i]='nightly-2026-03-16' # Pinned 2025/12/04
((i+=1)); nvs[i]=pyrefly@main;                oks[i]=Ok; nvs_args[i]='--git https://github.com/facebook/pyrefly --tag=0.44.0'; toolchain[i]='nightly-2025-09-14' # from its rust-toolchain.toml

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

((i+=1)); nvs[i]=uv@main;                     oks[i]=ko; nvs_args[i]='--git https://github.com/astral-sh/uv.git --rev=2748dce --bin=uv'; toolchain[i]='1.91' # failed to solve: ResourceExhausted: trying to send message larger than max (17778013 vs. 16777216)

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

((i+=1)); nvs[i]=crater@master;               oks[i]=Ok; nvs_args[i]='--git https://github.com/rust-lang/crater --rev=102a0c7b0e4298e7f21c8e177120b6e143409d63'

((i+=1)); nvs[i]=tract@0.22.1;                oks[i]=ok; nvs_args[i]=''

# Cross compilation
 ((i+=1)); nvs[i]=marauder@master;             oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/reMarkable-tools.git --bin=whiteboard --tag=v0.6.0 --target=armv7-unknown-linux-musleabihf' # Pinned on 2026/07/09
#((i+=1)); nvs[i]=marauder@master;             oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/reMarkable-tools.git --bin=whiteboard --rev=49f037f --target=armv7-unknown-linux-musleabihf' # Pinned on 2026/07/09 Uses buildscriptsources

#FIXME: test with Environment: CARGO_BUILD_RUSTC_WRAPPER or RUSTC_WRAPPER  or Environment: CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER or RUSTC_WORKSPACE_WRAPPER
# => the final invocation is $RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC.

#TODO: look into "writing rust tests inside tmux sessions"

as_install() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  case "$name_at_version" in
    *@main | *@master) echo "${name_at_version%@*}" ;;
    *) echo "$name_at_version" ;;
  esac
}

slugify() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  sed 's%@%_%g;s%+%_%g;s%\.%-%g;s%/%%g;s%:%%g' <<<"$name_at_version"
}

# Maps name@version to the name of the binary it produces (from docker-bake.json)
declare -A binaries=()
populate_binaries() {
  [[ $# -eq 0 ]]
  while read -r dockerfile bin; do
    binaries["${dockerfile%.Dockerfile}"]="$bin"
  done < <(jq -r '.target | to_entries[] | "\( .value.dockerfile )"+" \( .key )"' "$binaries_repo_root"/docker-bake.json)
}

# Whether CI runs this binary: only the ones that build (ok) and aren't too slow (Ok)
in_ci() {
  local i=$1; shift
  [[ $# -eq 0 ]]
  [[ "${oks[$i]:0:1}" = o ]] || return 1
  case "${nvs[$i]}" in
    cargo-green@*) return 1 ;; # Built from HEAD instead
  esac
}

# Prints KEY=VALUE lines
bin_envs() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  local envvars=()
  case "$name_at_version" in
    alacritty@*) envvars+=(CARGOGREEN_ADD_APT='cmake,g++,libfontconfig1-dev,libxcb-xfixes0-dev,libxkbcommon-dev,pkg-config,python3') ;; # From https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/INSTALL.md#debianubuntu
    bottom@*) envvars+=(CARGOGREEN_SET_ENVS='GITHUB_SHA'); envvars+=(GITHUB_SHA=) ;; # "Dirty bottom v0.11.4: the environment variable GITHUB_SHA changed"
    cargo-authors@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,libssl-dev(>=3.5),pkg-config') ;;
    cargo-c@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,libssl-dev(>=3.5),pkg-config') ;;
    cargo-llvm-cov@*) envvars+=(CARGOGREEN_COMPONENTS='llvm-tools-preview') ;;
    cargo-udeps@*) envvars+=(CARGOGREEN_ADD_APT='libcurl4-openssl-dev,libssl-dev(>=3.5),pkg-config,zlib1g-dev') ;;
    coccinelleforrust@*) envvars+=(CARGOGREEN_ADD_APT='python3-dev') ;;
    crater@*) envvars+=(CARGOGREEN_ADD_APT='build-essential,ca-certificates,cmake,curl,git,libsqlite3-dev,libssh2-1-dev,libssl-dev,pkg-config,zlib1g-dev') ;; # From https://github.com/rust-lang/crater/blob/102a0c7b0e4298e7f21c8e177120b6e143409d63/Dockerfile#L11-L18
    diesel_cli@*) envvars+=(CARGOGREEN_ADD_APT='libpq-dev') ;;
    marauder@*) envvars+=(CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER='arm-linux-gnueabihf-gcc'); envvars+=(CARGOGREEN_ADD_APT='gcc-arm-linux-gnueabihf,libc6-dev-armhf-cross,protobuf-compiler(>=3.6)') ;; # ; envvars+=(CARGOGREEN_EXPERIMENT='buildscriptsources') ;;
    miri@*) envvars+=(CARGOGREEN_COMPONENTS='llvm-tools-preview,rust-src,rustc-dev'); envvars+=(CARGOGREEN_ADD_APT='build-essential') ;;
    mussh@*) envvars+=(CARGOGREEN_ADD_APT='libsqlite3-dev,libssl-dev(>=3.5),pkg-config,zlib1g-dev') ;;
    nanometers@*) envvars+=(CARGOGREEN_ADD_APT='libwayland-dev,libglib2.0-dev,libdbus-1-dev,libpangocairo-1.0-0,libasound2-dev,libcairo2-dev,libpango-1.0-0,libpango1.0-dev,libssl-dev=3.5.5-1~deb13u2,libxcb-render0-dev,libxcb-shape0-dev,libxcb-xfixes0-dev,libxkbcommon-dev,libx11-dev,libxcursor-dev,libxcb1-dev,libxi-dev,libxkbcommon-x11-dev,xvfb') ;;
    ntpd@*) envvars+=(NTPD_RS_GIT_REV=c7945250c378f65f65b2a75748132edf75063b3b); envvars+=(NTPD_RS_GIT_DATE=2025-05-09) ;; # Any commit, just fixed + Time of commit
    privaxy@*) envvars+=(CARGOGREEN_ADD_APT='build-essential,libayatana-appindicator3-dev,libgtk-3-dev,librsvg2-dev,libsoup2.4-dev,libssl-dev=3.5.5-1~deb13u2,pkg-config') ;;
    pyrefly@*) envvars+=(CARGOGREEN_ADD_APT='make') ;;
    rapidraw@*) envvars+=(CARGOGREEN_ADD_APT='g++,libgtk-3-dev,libjavascriptcoregtk-4.1-dev,libsoup-3.0-dev,libssl-dev=3.5.5-1~deb13u2,libwebkit2gtk-4.1-dev') ;;
    rublk@*) envvars+=(CARGOGREEN_ADD_APT='libclang-dev') ;;
    sccache@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev(>=3.5),pkg-config,zlib1g-dev') ;;
    torrust-index@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev(>=3.5),pkg-config,zlib1g-dev') ;;
    uv@*) envvars+=(CARGOGREEN_ADD_APT='make') ;;
    zed@*) envvars+=(CARGOGREEN_ADD_APT='build-essential,clang,cmake,curl,elfutils,g++,gcc,gettext-base,git,jq,libasound2-dev,libfontconfig-dev,libgit2-dev,libglib2.0-dev,libsqlite3-dev,libssl-dev(>=3.5),libva-dev,libvulkan1,libwayland-dev,libx11-xcb-dev,libxkbcommon-x11-dev,libzstd-dev,lld,llvm,make,musl-dev,musl-tools,pipewire,protobuf-compiler,xdg-desktop-portal') ;; # From https://github.com/zed-industries/zed/blob/v0.233.10/script/linux#L25-L52
    *) ;;
  esac
  [[ ${#envvars[@]} -eq 0 ]] || printf '%s\n' "${envvars[@]}"
}

binaries_json() {
  populate_binaries
  for i in "${!nvs[@]}"; do
    in_ci "$i" || continue
    local nv=${nvs[$i]} cargo=cargo
    [[ "${toolchain[$i]:-}" = '' ]] || cargo="cargo +${toolchain[$i]}"
    local envs; envs=$(bin_envs "$nv" | jq -Rn '[inputs | select(length > 0) | capture("^(?<k>[^=]+)=(?<v>.*)$") | {(.k): .v}] | add // {}')
    # Only what is not a default (args '', cargo, envs {}) and what can't be computed from nv (see clis.sh)
    local obj; obj=$(jq -n \
      --arg nv "$nv" \
      --arg args "${nvs_args[$i]}" \
      --arg cargo "$cargo" \
      --arg bin "${binaries[$nv]:-NOBINYET}" \
      --argjson envs "$envs" \
      '{nv: $nv}
        + (if $args != "" then {args: $args} else {} end)
        + (if $cargo != "cargo" then {cargo: $cargo} else {} end)
        + (if $envs != {} then {envs: $envs} else {} end)
        + {bin: $bin}')
    # Identifies that object, eg. to key the cache of its crates sources
    jq -c --arg sha1 "$(jq -cS . <<<"$obj" | sha1sum | cut -d' ' -f1)" '. + {sha1: $sha1}' <<<"$obj"
  done | jq -s .
}

if [[ "${BASH_SOURCE[0]}" = "$0" ]]; then
  case "${1:-}" in
    json) binaries_json ;;
    list) binaries_json | jq -r '.[].nv' ;;
    *) echo "Usage: $0 ( json | list )" ; exit 1 ;;
  esac
fi
