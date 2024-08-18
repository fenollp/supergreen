#!/bin/bash -eu
set -o pipefail

# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.21.0-pre.0;    oks[i]=ko; nvs_args[i]='--features=fix' # ResourceExhausted: grpc: received message larger than max (5136915 vs. 4194304)
((i+=1)); nvs[i]=cargo-bpf@2.3.0;             oks[i]=ko; nvs_args[i]='' # Package libelf was not found in the pkg-config search path.
((i+=1)); nvs[i]=cargo-config2@0.1.26;        oks[i]=ko; nvs_args[i]='--example=get' # unexpected output from `rustc --version`: ""
((i+=1)); nvs[i]=cargo-deny@0.16.1;           oks[i]=ko; nvs_args[i]='' # [rustix 0.38.34] thread 'main' panicked at /home/pete/.cargo/registry/src/index.crates.io-6f17d22bba15001f/rustix-0.38.34/build.rs:247:64: [rustix 0.38.34] called `Result::unwrap()` on an `Err` value: Os { code: 32, kind: BrokenPipe, message: "Broken pipe" }
((i+=1)); nvs[i]=cargo-fuzz@0.12.0;           oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET_PLATFORM` not defined at compile time .. current_platform-0.2.0 + HOST_PLATFORM
((i+=1)); nvs[i]=cargo-green@main;            oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main cargo-green'
((i+=1)); nvs[i]=cargo-llvm-cov@0.5.36;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.72;        oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET` not defined at compile time .. self_update-0.39.0
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]='--git https://github.com/cross-rs/cross.git --tag=v0.2.5 cross'
((i+=1)); nvs[i]=diesel_cli@2.1.1;            oks[i]=ko; nvs_args[i]='--no-default-features --features=postgres' # /usr/bin/ld: cannot find -lpq: No such file or directory
((i+=1)); nvs[i]=hickory-dns@0.25.0-alpha.1;  oks[i]=ok; nvs_args[i]='--features=dns-over-rustls'
((i+=1)); nvs[i]=krnlc@0.1.1;                 oks[i]=ko; nvs_args[i]='' # type annotations needed for `Box<_>` .. time-0.3.31
((i+=1)); nvs[i]=mussh@3.1.3;                 oks[i]=ko; nvs_args[i]='' # = note: /usr/bin/ld: cannot find -lsqlite3: No such file or directory (and -lssl -lcrypto -lz)
((i+=1)); nvs[i]=ntpd@1.2.3;                  oks[i]=ko; nvs_args[i]='' # BUG: bad URL creation https://static.crates.io/crates/md/md-5-0.10.6.crate
((i+=1)); nvs[i]=qcow2-rs@0.1.2;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ripgrep@14.1.0;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=rublk@0.2.0;                 oks[i]=ko; nvs_args[i]='' # could not find native static library `rustix_outline_x86_64`, perhaps an -L flag is missing?
((i+=1)); nvs[i]=shpool@0.6.2;                oks[i]=ko; nvs_args[i]='' # sudo apt-get install libpam0g-dev
((i+=1)); nvs[i]=solana-gossip@2.0.5;         oks[i]=ko; nvs_args[i]='' # error: environment variable `TYPENUM_BUILD_OP` not defined at compile time                                                                                                                    
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=ko; nvs_args[i]='' # BUG: unexpected crate-type: 'cdylib'
((i+=1)); nvs[i]=torrust-index@3.0.0-alpha.12; oks[i]=ko; nvs_args[i]='' # unexpected output from `rustc --version`: ""
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''

#TODO: not a cli but try users of https://github.com/dtolnay/watt
  # curl -s 'https://crates.io/api/v1/crates/rustversion/reverse_dependencies?page=4&per_page=100' --compressed |jq '.versions[]|select(.bin_names != [])|.crate'
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none
#TODO: test cargo miri usage
#TODO: test cargo lambda build --release --arm64 usage
#TODO: test https://github.com/facebookexperimental/MIRAI

# TODO https://github.com/aizcutei/nanometers?tab=readme-ov-file#testing-locally

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

    - name: Cache \`cargo fetch\`
      uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

    - name: Cache \`cargo install\`
      uses: actions/cache@v4
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD cargo-green
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./cargo-green
    - run: ls -lha ~/instmp/release/
    - run: ls -lha /home/runner/.cargo/bin/

    - uses: actions/upload-artifact@v4
      with:
        name: bin-artifacts
        path: /home/runner/.cargo/bin/cargo-green

EOF
}

as_install() {
  local name_at_version=$1; shift
  case "$name_at_version" in
    buildxargs@*) echo 'buildxargs';;
    cargo-green@*) echo 'cargo-green';;
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
        chmod +x ./cargo-green
        ./cargo-green --version | grep cargo-green
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
      run: /home/runner/.cargo/bin/cargo-green green supergreen env

    - name: Envs again
      run: /home/runner/.cargo/bin/cargo-green green supergreen env

    - name: Disk usage
      run: |
        docker system df
        docker buildx du
        sudo du -sh /var/lib/docker

    - name: cargo install net=ON cache=OFF remote=OFF jobs=$jobs
      run: |
        CARGOGREEN_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that cargo issue https://github.com/rust-lang/cargo/pull/14322
      run: |
        ! grep -C20 -F "thread 'main' panicked at src/cargo/util/dependency_queue.rs:" _

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that docker issue https://github.com/moby/buildkit/issues/5217
      run: |
        ! grep -C20 -F 'ResourceExhausted: grpc: received message larger than max' logs.txt

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some panic!s
      run: |
        ! grep -C20 -F ' panicked at ' logs.txt

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some BUGs
      run: |
        ! grep -C20 -F 'BUG: ' logs.txt

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> here's relevant logs
      run: |
        ! grep -C20 -F ' >>> ' logs.txt

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
        CARGOGREEN_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _

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
      cargo-green@*) continue ;;
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
    CARGO_TARGET_DIR=/tmp/crggreen cargo install --locked --frozen --offline --force --root=/tmp/crggreen --path="$PWD"/cargo-green
    ls -lha /tmp/crggreen/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs
    echo "$name_at_version"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    xdg-terminal-exec tail -f $tmplogs
    CARGOGREEN_LOG=debug \
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
  CARGO_TARGET_DIR=/tmp/rstcbldx cargo install --locked --frozen --offline --force --path="$gitdir"/cargo-green \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

send \
  'until' '[[' -f "$tmpgooo".installed ']];' \
  'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* '&&' \
  CARGOGREEN_LOG=debug \
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
