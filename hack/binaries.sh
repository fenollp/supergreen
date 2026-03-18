#!/usr/bin/env -S bash -eu
set -o pipefail

# ok: builds | ko: doesn't build | [ok]D: ok|ko but old: shows too many cfg warnings | Ok: takes >=10min in CI
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.22.1;          oks[i]=ok; nvs_args[i]='--features=fix'
((i+=1)); nvs[i]=cargo-deny@0.18.5;           oks[i]=Ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-fuzz@0.13.1;           oks[i]=ok; nvs_args[i]=''
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

unset i
