#!/bin/bash -eu
set -o pipefail

# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.21.0-pre.0;    oks[i]=ok; nvs_args[i]='--features=fix' # ResourceExhausted: grpc: received message larger than max (5136915 vs. 4194304)
((i+=1)); nvs[i]=cargo-bpf@2.3.0;             oks[i]=ko; nvs_args[i]='' # Package libelf was not found in the pkg-config search path.
((i+=1)); nvs[i]=cargo-config2@0.1.26;        oks[i]=ko; nvs_args[i]='--example=get' # unexpected output from `rustc --version`: ""
((i+=1)); nvs[i]=cargo-deny@0.14.3;           oks[i]=ko; nvs_args[i]='' # https://github.com/docker/buildx/issues/2453  ResourceExhausted: (x5) grpc: received message larger than max (4202037 vs. 4194304) [also: 4949313 vs. 4194304] 2023-11-21T13:21:18.5168012Z    1 | >>> # syntax=docker.io/docker/dockerfile:1@sha256:ac85f380a63b13dfcefa89046420e1781752bab202122f8f50032edf31be0021
((i+=1)); nvs[i]=cargo-llvm-cov@0.5.36;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.61;        oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET` not defined at compile time .. self_update-0.38.0
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=diesel_cli@2.1.1;            oks[i]=ko; nvs_args[i]='--no-default-features --features=postgres' # /usr/bin/ld: cannot find -lpq: No such file or directory
((i+=1)); nvs[i]=hickory-dns@0.25.0-alpha.1;  oks[i]=ok; nvs_args[i]='--features=dns-over-rustls'
((i+=1)); nvs[i]=solana-gossip@2.0.5;         oks[i]=ko; nvs_args[i]='' # error: environment variable `TYPENUM_BUILD_OP` not defined at compile time                                                                                                                    
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=ko; nvs_args[i]='' # BUG: unexpected crate-type: 'cdylib'
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''

# https://crates.io/crates/ntpd


# https://crates.io/crates/cargo-fuzz/0.12.0
#    Compiling thiserror-impl v1.0.50
# error: environment variable `TARGET_PLATFORM` not defined at compile time
#   --> /home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/current_platform-0.2.0/src/lib.rs:29:36
#    |
# 29 | pub const CURRENT_PLATFORM: &str = env!("TARGET_PLATFORM");
#    |                                    ^^^^^^^^^^^^^^^^^^^^^^^
#    |
#    = help: use `std::env::var("TARGET_PLATFORM")` to read the variable at run time
#    = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)

# error: environment variable `HOST_PLATFORM` not defined at compile time
#   --> /home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/current_platform-0.2.0/src/lib.rs:38:31
#    |
# 38 | pub const COMPILED_ON: &str = env!("HOST_PLATFORM");
#    |                               ^^^^^^^^^^^^^^^^^^^^^
#    |
#    = help: use `std::env::var("HOST_PLATFORM")` to read the variable at run time
#    = note: this error originates in the macro `env` (in Nightly builds, run with -Z macro-backtrace for more info)

# error: could not compile `current_platform` (lib) due to 2 previous errors
# warning: build failed, waiting for other jobs to finish...
# error: failed to compile `cargo-fuzz v0.12.0`, intermediate artifacts can be found at `/tmp/cfzz`.
# To reuse those artifacts with a future compilation, set the environment variable `CARGO_TARGET_DIR` to that path.
# 101 36s supergreen.git green λ CARGO_TARGET_DIR=/tmp/cfzz \cargo green install --force --locked cargo-fuzz


# TODO https://github.com/aizcutei/nanometers?tab=readme-ov-file#testing-locally


#    Compiling clap_lex v0.7.0
#    Compiling winnow v0.5.40
#    Compiling anyhow v1.0.81
#    Compiling bitflags v1.3.2
#    Compiling heck v0.4.1
#    Compiling same-file v1.0.6
#    Compiling strsim v0.11.0
#    Compiling arrayvec v0.7.4
#    Compiling vte v0.12.1
#    Compiling clap_builder v4.5.2
#    Compiling walkdir v2.5.0
#    Compiling clap_derive v4.5.0
#    Compiling inotify v0.9.6
#    Compiling toml_edit v0.19.15
#    Compiling sharded-slab v0.1.7
# error: failed to run custom build command for `motd v0.2.1`

# Caused by:
#   process didn't exit successfully: `/tmp/cargo-installSYJwTA/release/build/motd-0e127c12db4c87b5/build-script-build` (exit status: 101)
#   --- stdout
#   cargo:rerun-if-changed=src/pam_motd_overlay.c
#   cargo:rerun-if-changed=src/pam_motd_overlay_versions.ldscript

#   --- stderr
#   thread 'main' panicked at /home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/motd-0.2.1/build.rs:23:13:
#   build failed: error building overlay, code = exit status: 1
#   STDOUT: 
#   STDERR: ./src/pam_motd_overlay.c:11:10: fatal error: security/_pam_types.h: No such file or directory
#      11 | #include <security/_pam_types.h>
#         |          ^~~~~~~~~~~~~~~~~~~~~~~
#   compilation terminated.

#   note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
# error: failed to compile `shpool v0.6.2`, intermediate artifacts can be found at `/tmp/cargo-installSYJwTA`.
# To reuse those artifacts with a future compilation, set the environment variable `CARGO_TARGET_DIR` to that path.
# 101 70s supergreen.git main λ RUSTCBUILDX_LOG=debug \cargo green install --locked shpool --jobs=1



# https://crates.io/crates/motd/0.2.1

# Dependencies

# You must install libpam headers to build this crate. On debian based systems you can do so with

# sudo apt-get install libpam0g-dev


# RUSTCBUILDX_LOG=debug \cargo green install --offline --locked shpool --jobs=1




((i+=1)); nvs[i]=rustcbuildx@main;      oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main rustcbuildx'

#TODO: not a cli but try users of https://github.com/dtolnay/watt
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none
#TODO: test cargo miri usage
#TODO: test cargo lambda build --release --arm64 usage
#TODO: test https://github.com/facebookexperimental/MIRAI

#FIXME: test with Environment: CARGO_BUILD_RUSTC_WRAPPER or RUSTC_WRAPPER  or Environment: CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER or RUSTC_WORKSPACE_WRAPPER
# => the final invocation is $RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC.

#TODO: look into "writing rust tests inside tmux sessions"

header() {
	cat <<EOF
on: [push]
name: CLIs
jobs:


  meta-check:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v4
    - run: ./hack/clis.sh | tee .github/workflows/clis.yml
    - run: git --no-pager diff --exit-code
    - name: Run shellcheck
      uses: ludeeus/action-shellcheck@2.0.0
      with:
        check_together: 'yes'
        severity: error

  bin:
    runs-on: ubuntu-latest
    steps:
    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: stable

    - uses: actions/checkout@v4

    # Actually, the whole archives
    - name: Cache \`cargo fetch\`
      uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: cargo-deps-

    - name: Cache \`cargo install\`
      uses: actions/cache@v4
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}-and-\${{ hashFiles('src/**') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}-and-
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD cargo-green
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./cargo-green
    - name: Compile HEAD rustcbuildx
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./rustcbuildx
    - run: ls -lha ~/instmp/release/
    - run: ls -lha /home/runner/.cargo/bin/

    - uses: actions/upload-artifact@v4
      with:
        name: bin-artifacts
        path: |
          /home/runner/.cargo/bin/cargo-green
          /home/runner/.cargo/bin/rustcbuildx

EOF
}

as_install() {
  local name_at_version=$1; shift
  case "$name_at_version" in
    buildxargs@*) echo 'buildxargs';;
    rustcbuildx@*) echo 'rustcbuildx';;
    *) echo "$name_at_version" ;;
  esac
}

cli() {
	local name_at_version=$1; shift
  local jobs=$1; shift

  # TODO: drop
  #   thread 'main' panicked at src/cargo/util/dependency_queue.rs:191:13:
  #   assertion failed: edges.remove(&key)
  # https://github.com/fenollp/supergreen/actions/runs/9050434991/job/24865786185?pr=35#logs
  # https://github.com/rust-lang/cargo/issues/13889
  if [[ "$name_at_version" = cargo-llvm-cov@0.5.36 ]] && [[ "$jobs" != 1 ]]; then return; fi

	cat <<EOF
  $(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")$(if [[ "$jobs" != 1 ]]; then echo '-J'; fi):
    runs-on: ubuntu-latest
    needs: bin
    steps:
    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: stable
$(
	case "$name_at_version" in
		cargo-llvm-cov@*) printf '    - run: rustup component add llvm-tools-preview\n' ;;
		*) ;;
	esac
)

    - name: Retrieve saved bin
      uses: actions/download-artifact@v4
      with:
        name: bin-artifacts
    - run: | # TODO: whence https://github.com/actions/download-artifact/issues/236
        chmod +x ./cargo-green ./rustcbuildx
        ./rustcbuildx --version | grep rustcbuildx
        mv ./rustcbuildx /home/runner/.cargo/bin/
        mv ./cargo-green /home/runner/.cargo/bin/
        cargo green --version

    - uses: actions/checkout@v4

    - name: Docker info
      run: docker info

    - name: Buildx version
      run: docker buildx version

    - name: Podman version
      run: podman version || true

    - name: Rust version
      run: rustc -Vv

    - name: Envs
      run: /home/runner/.cargo/bin/rustcbuildx env

    - name: Envs again
      run: /home/runner/.cargo/bin/rustcbuildx env

    - name: Disk usage
      run: |
        docker system df
        docker buildx du
        sudo du -sh /var/lib/docker

    - name: cargo install net=ON cache=OFF remote=OFF jobs=$jobs
      run: |
        RUSTCBUILDX_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@

    - if: \${{ failure() || success() }}
      run: tail -n9999999 logs.txt ; echo >logs.txt

    - name: Disk usage
      if: \${{ failure() || success() }}
      run: |
        docker system df
        docker buildx du
        sudo du -sh /var/lib/docker

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

    - name: cargo install net=ON cache=ON remote=OFF jobs=$jobs
      run: |
        RUSTCBUILDX_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ 2>&1 | tee _

    - if: \${{ failure() || success() }}
      run: tail -n9999999 logs.txt

    - name: Disk usage
      if: \${{ failure() || success() }}
      run: |
        docker system df
        docker buildx du
        sudo du -sh /var/lib/docker

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

    - if: \${{ failure() || success() }}
      name: Finishes fast
      run: |
        grep Finished _
        grep Finished _ | grep -E [012]...s

    - if: \${{ failure() || success() }}
      run: |
        grep Fresh _

    - if: \${{ failure() || success() }}
      name: Did not recompile things (yay!)
      run: |
        ! grep Compiling _

    - if: \${{ failure() || success() }}
      run: |
        ! grep 'DEBUG|INFO|WARN|ERROR' _
        ! grep 'Falling back' _
        ! grep 'BUG[: ]' _

    - if: \${{ failure() || success() }}
      run: cat _ || true

EOF
}

# No args: try many combinations, sequentially
if [[ $# = 0 ]]; then
  header

  for i in "${!nvs[@]}"; do
    [[ "${oks[$i]}" = 'ok' ]] || continue
    name_at_version=${nvs["$i"]}
    case "$name_at_version" in
      rustcbuildx@*) continue ;;
    esac
    cli "$name_at_version" 1 "${nvs_args["$i"]}"
    # 3: https://docs.github.com/en/actions/using-github-hosted-runners/about-github-hosted-runners/about-github-hosted-runners#standard-github-hosted-runners-for-public-repositories
    cli "$name_at_version" 3 "${nvs_args["$i"]}"
  done

  exit
fi


name_at_version=$1; shift
modifier=${1:-0}

clean=0; if [[ "$modifier" = 'clean' ]]; then clean=1; fi

# Special first arg handling..
case "$name_at_version" in
  ok)
    for i in "${!nvs[@]}"; do
      [[ "${oks[$i]}" = 'ok' ]] || continue
      nv=${nvs[$i]}
      "$0" "${nv#*@}" "$modifier"
    done
    exit $? ;;

  build)
set -x
    tmptrgt=/tmp/clis-$name_at_version
    tmplogs=/tmp/clis-$name_at_version.logs.txt
    if [[ "$clean" = '1' ]]; then
      rm -rf "$tmptrgt"
    fi
    CARGO_TARGET_DIR=/tmp/rstcbldx cargo install --locked --frozen --offline --force --root=/tmp/rstcbldx --path="$PWD"/rustcbuildx
    CARGO_TARGET_DIR=/tmp/crggreen cargo install --locked --frozen --offline --force --root=/tmp/crggreen --path="$PWD"/cargo-green
    ls -lha /tmp/rstcbldx/bin/rustcbuildx
    ls -lha /tmp/crggreen/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs
    echo "$name_at_version"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    xdg-terminal-exec tail -f $tmplogs
    RUSTCBUILDX_LOG=debug \
    RUSTCBUILDX_LOG_PATH="$tmplogs" \
    RUSTCBUILDX_CACHE_IMAGE="${RUSTCBUILDX_CACHE_IMAGE:-}" \
    PATH=/tmp/crggreen/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \
      \cargo green -v build --jobs=${jobs:-1} --all-targets --all-features --locked --frozen --offline
      # FIXME: this doesn't depend on $name_at_version
    if [[ "$clean" = 1 ]]; then docker buildx du --builder=supergreen --verbose | tee --append "$tmplogs" || exit 1; fi
    case "$(wc "$tmplogs")" in '0 0 0 '*) ;;
                                       *) $PAGER "$tmplogs" ;; esac
    exit ;;
esac

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

session_name=$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")
tmptrgt=/tmp/clis-$session_name
tmplogs=/tmp/clis-$session_name.logs.txt
tmpgooo=/tmp/clis-$session_name.state
tmpbins=/tmp

if [[ "$clean" = '1' ]]; then
  rm -rf "$tmptrgt"
fi

rm -f "$tmpgooo".*
tmux new-session -d -s "$session_name"
tmux select-window -t "$session_name:0"

send() {
  tmux send-keys "$* && tmux select-layout even-vertical && exit" C-m
}


gitdir=$(realpath "$(dirname "$(dirname "$0")")")
send \
  CARGO_TARGET_DIR=/tmp/rstcbldx cargo install --locked --frozen --offline --force --path="$gitdir"/rustcbuildx \
    '&&' touch "$tmpgooo".installed
tmux split-window

send \
  CARGO_TARGET_DIR=/tmp/crggreen cargo install --locked --frozen --offline --force --path="$gitdir"/cargo-green \
    '&&' touch "$tmpgooo".installed_bis \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

send \
  'until' '[[' -f "$tmpgooo".installed ']] && [[' -f "$tmpgooo".installed_bis ']];' \
  'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* '&&' \
  RUSTCBUILDX_LOG=debug \
  RUSTCBUILDX_LOG_PATH="$tmplogs" \
  RUSTCBUILDX_CACHE_IMAGE="${RUSTCBUILDX_CACHE_IMAGE:-}" \
  PATH=/tmp/crggreen/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \cargo green -vv install --timings --jobs=${jobs:-1} --root=$tmpbins --locked --force "$(as_install "$name_at_version")" "$args" \
  '&&' 'if' '[[' "$clean" '=' '1' ']];' 'then' docker buildx du --builder=supergreen --verbose '|' tee --append "$tmplogs" '||' 'exit' '1;' 'fi' \
  '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"

echo "$name_at_version"
echo "Target dir: $tmptrgt"
echo "Logs: $tmplogs"
case "$(wc "$tmplogs")" in '0 0 0 '*) ;;
                                   *) $PAGER "$tmplogs" ;; esac
