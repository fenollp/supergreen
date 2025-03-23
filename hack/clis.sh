#!/bin/bash -eu
set -o pipefail

source $(realpath "$(dirname "$0")")/ck.sh

with_j=0 # TODO: 1 => adds jobs with -J (see cargo issue https://github.com/rust-lang/cargo/issues/13889)

# Usage:  $0                              #=> generate CI
# Usage:  $0 ( <name@version> | <name> )  #=> cargo install name@version
# Usage:  $0 ( build | test )             #=> cargo build ./cargo-green


# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.21.1;          oks[i]=ko; nvs_args[i]='--features=fix' # TODO: re-ok when GitHub Actions runners update to patched BuildKit (>=v0.20)
((i+=1)); nvs[i]=cargo-bpf@2.3.0;             oks[i]=ko; nvs_args[i]='' # Package libelf was not found in the pkg-config search path.
((i+=1)); nvs[i]=cargo-deny@0.16.1;           oks[i]=ko; nvs_args[i]='' # TODO: re-ok when GitHub Actions runners update to patched BuildKit (>=v0.20)
((i+=1)); nvs[i]=cargo-fuzz@0.12.0;           oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET_PLATFORM` not defined at compile time .. current_platform-0.2.0 + HOST_PLATFORM
((i+=1)); nvs[i]=cargo-green@main;            oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main cargo-green'
((i+=1)); nvs[i]=cargo-llvm-cov@0.5.36;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.72;        oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET` not defined at compile time .. self_update-0.39.0
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]='--git https://github.com/cross-rs/cross.git --tag=v0.2.5 cross'
((i+=1)); nvs[i]=diesel_cli@2.1.1;            oks[i]=ko; nvs_args[i]='--no-default-features --features=postgres' # /usr/bin/ld: cannot find -lpq: No such file or directory
((i+=1)); nvs[i]=hickory-dns@0.25.0-alpha.1;  oks[i]=ok; nvs_args[i]='--features=dns-over-rustls'
((i+=1)); nvs[i]=krnlc@0.1.1;                 oks[i]=ko; nvs_args[i]='' # type annotations needed for `Box<_>` .. time-0.3.31
((i+=1)); nvs[i]=mussh@3.1.3;                 oks[i]=ko; nvs_args[i]='' # = note: /usr/bin/ld: cannot find -lsqlite3: No such file or directory (and -lssl -lcrypto -lz)
((i+=1)); nvs[i]=ntpd@1.2.3;                  oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=qcow2-rs@0.1.2;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ripgrep@14.1.0;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=rublk@0.2.0;                 oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=shpool@0.6.2;                oks[i]=ko; nvs_args[i]='' # sudo apt-get install libpam0g-dev
((i+=1)); nvs[i]=solana-gossip@2.0.5;         oks[i]=ko; nvs_args[i]='' # error: environment variable `TYPENUM_BUILD_OP` not defined at compile time                                                                                                                    
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=ko; nvs_args[i]='' # BUG: unexpected crate-type: 'cdylib'
((i+=1)); nvs[i]=torrust-index@3.0.0-alpha.12; oks[i]=ko; nvs_args[i]='' # /usr/bin/ld: cannot find -lssl: No such file or directory
((i+=1)); nvs[i]=cargo-authors@0.5.5;         oks[i]=ko; nvs_args[i]='' #               cannot find -lz: Missing libz
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-config2@0.1.26;        oks[i]=ko; nvs_args[i]='--example=get' # all_externs = {"libanyhow-57432fd275d03fdb.rlib", "libbuild_context-384aad20df3c3d57.rlib", "libcargo_config2-5033c667798c709b.rlib", "libclap-c887d4f0020b6f5a.rlib", "libduct-83b4b7f8ebae4d93.rlib", "libfs_err-12deea2db0355be5.rlib", "liblexopt-52c12aa750ecabf4.rlib", "librustversion-6e7d2848c032b0fa.so", "libserde-b0b79900e6cfdc34.rlib", "libserde_derive-d8e1643ef4697944.so", "libserde_json-eaa3f1f11c64af0a.rlib", "libshell_escape-adb479b6c563c0cc.rlib", "libstatic_assertions-b95da2456a70fa1a.rlib", "libtempfile-3898ff553f5c9672.rlib", "libtoml-359e6e9bd328d4ea.rlib", "libtoml_edit-bdbe3222184e0314.rlib"} opening (RO) extern md /tmp/clis-cargo-config2_0-1-26/anyhow-57432fd275d03fdb.toml => missing TOML
((i+=1)); nvs[i]=privaxy@0.5.2;               oks[i]=ko; nvs_args[i]='--git https://github.com/Barre/privaxy.git --tag=v0.5.2 privaxy' # undefined reference to `__isoc23_strtol'\n          /usr/bin/ld: rand_unix.c:(.text.wait_random_seeded+0x204): undefined reference to `__isoc23_strtol'\n          collect2: error: ld returned 1 exit status\n          \n  = note: some `extern` functions couldn't be found; some native libraries may need to be installed or have their path specified\n  = note: use the `-l` flag to specify native libraries to link\n  = note: use the `cargo:rustc-link-lib` directive to specify the native libraries to link with Cargo (see https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-lib


# ((i+=1)); nvs[i]=cargo-acl@0.8.0;             oks[i]=ok; nvs_args[i]='' passes (unsurprisingly)
((i+=1)); nvs[i]=miri@master;                 oks[i]=ko; nvs_args[i]='--git https://github.com/rust-lang/miri.git --rev=dcd2112' # can't find crate for `either`
((i+=1)); nvs[i]=zed@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/zed-industries/zed.git --tag=v0.149.5 zed' # The pkg-config command could not be found.
((i+=1)); nvs[i]=cargo-udeps@0.1.50;          oks[i]=ko; nvs_args[i]='' # The pkg-config command could not be found.
((i+=1)); nvs[i]=rerun-cli@0.18.0;            oks[i]=ko; nvs_args[i]='' # environment variable `TYPENUM_BUILD_OP` not defined at compile time + TYPENUM_BUILD_CONSTS

((i+=1)); nvs[i]=mirai@main;                  oks[i]=ko; nvs_args[i]='--git https://github.com/facebookexperimental/MIRAI.git --tag=v1.1.9 checker'
# error: could not find `checker` in https://github.com/facebookexperimental/MIRAI.git?tag=v1.1.9 with version `*`
# ---
# #10 0.170 ::STDERR:: info: syncing channel updates for 'nightly-2023-12-30-x86_64-unknown-linux-gnu'
# #10 0.170 ::STDERR:: error: could not download file from 'https://static.rust-lang.org/dist/2023-12-30/channel-rust-nightly.toml.sha256' to '/usr/local/rustup/tmp/56sbia14kl406mlj_file':
#             failed to make network request:
#               error sending request for url (https://static.rust-lang.org/dist/2023-12-30/channel-rust-nightly.toml.sha256):
#                 client error (Connect): dns error: failed to lookup address information:
#                   Temporary failure in name resolution: failed to lookup address information: Temporary failure in name resolution


((i+=1)); nvs[i]=synthesizer@master;          oks[i]=ko; nvs_args[i]='--git https://github.com/hsfzxjy/handwriter.ttf.git --rev=ba4ab89 synthesizer' # (probably needs git-lfs) no binary target

((i+=1)); nvs[i]=a-mir-formality@main;        oks[i]=ko; nvs_args[i]='--git https://github.com/rust-lang/a-mir-formality.git --rev=8cc6aba a-mir-formality' #

# ((i+=1)); nvs[i]=cargo-prusti@master;         oks[i]=ko; nvs_args[i]='--git https://github.com/viperproject/prusti-dev.git --tag=v-2024-03-26-1504 cargo-prusti' #
((i+=1)); nvs[i]=prusti@master;               oks[i]=ko; nvs_args[i]='--git https://github.com/viperproject/prusti-dev.git --tag=v-2024-03-26-1504 prusti' #

((i+=1)); nvs[i]=kani-verifier@0.54.0;        oks[i]=ko; nvs_args[i]='' #

((i+=1)); nvs[i]=creusat@master;              oks[i]=ko; nvs_args[i]='--git https://github.com/sarsko/creusat.git --rev=b36aacd CreuSAT' #

((i+=1)); nvs[i]=cargo-make@0.37.15;          oks[i]=ko; nvs_args[i]='' #


((i+=1)); nvs[i]=coccinelleforrust@main;      oks[i]=ko; nvs_args[i]='--git https://gitlab.inria.fr/coccinelle/coccinelleforrust.git --rev=42eab688 cfr' #

((i+=1)); nvs[i]=ipa@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/seekbytes/IPA.git --rev=3094f92 ipa' # environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time


# ((i+=1)); nvs[i]=cargo-tally@1.0.48;          oks[i]=ok; nvs_args[i]='' #
# ((i+=1)); nvs[i]=cargo-mutants@24.7.1;        oks[i]=ok; nvs_args[i]='' #
# ((i+=1)); nvs[i]=binsider@0.2.0;              oks[i]=ok; nvs_args[i]='' #




#TODO: not a cli but try users of https://github.com/dtolnay/watt
  # curl -s 'https://crates.io/api/v1/crates/rustversion/reverse_dependencies?page=4&per_page=100' --compressed |jq '.versions[]|select(.bin_names != [])|.crate'
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none

# TODO https://github.com/aizcutei/nanometers?tab=readme-ov-file#testing-locally

#FIXME: test with Environment: CARGO_BUILD_RUSTC_WRAPPER or RUSTC_WRAPPER  or Environment: CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER or RUSTC_WORKSPACE_WRAPPER
# => the final invocation is $RUSTC_WRAPPER $RUSTC_WORKSPACE_WRAPPER $RUSTC.

#TODO: look into "writing rust tests inside tmux sessions"

header() {
	cat <<EOF
on: [push]
name: CLIs
jobs:


$(jobdef 'meta-check')
    steps:
    - uses: actions/checkout@v4
    - run: ./hack/clis.sh | tee .github/workflows/clis.yml
    - run: ./hack/self.sh | tee .github/workflows/self.yml
    - run: git --no-pager diff --exit-code
    - name: Run shellcheck
      uses: ludeeus/action-shellcheck@2.0.0
      with:
        check_together: 'yes'
        severity: error

$(jobdef 'bin')
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
    *@main | *@master) echo "${name_at_version%@*}" ;;
    *) echo "$name_at_version" ;;
  esac
}

cli() {
	local name_at_version=$1; shift
  local jobs=$1; shift

	cat <<EOF
$(jobdef "$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")$(if [[ "$jobs" != 1 ]]; then echo '-J'; fi)")
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

$(restore_bin-artifacts)
    - uses: actions/checkout@v4
$(rundeps_versions)

    - name: Envs
      run: /home/runner/.cargo/bin/cargo-green green supergreen env
    - name: Envs again
      run: /home/runner/.cargo/bin/cargo-green green supergreen env

$(cache_usage)
    - name: cargo install net=ON cache=OFF remote=OFF jobs=$jobs
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _
$(postconds _ logs.txt)
$(cache_usage)

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

    - name: Ensure running the same command twice without modifications...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _
$(postcond_fresh _)
$(postconds _ logs.txt)
$(cache_usage)

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

EOF
}

maybe_show_logs() {
  local logfile=$1; shift
  case "$(wc "$logfile")" in '0 0 0 '*) ;;
                                     *) $PAGER "$logfile" ;; esac
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
    if [[ $with_j = 1 ]]; then
      # 3: https://docs.github.com/en/actions/using-github-hosted-runners/about-github-hosted-runners/about-github-hosted-runners#standard-github-hosted-runners-for-public-repositories
      cli "$name_at_version" 3 "${nvs_args["$i"]}"
    fi
  done

  exit
fi


arg1=$1; shift
modifier=${1:-0}

clean=0; if [[ "$modifier" = 'clean' ]]; then clean=1; fi

install_dir=/tmp/cargo-green

# Special first arg handling..
case "$arg1" in
  ok)
    for i in "${!nvs[@]}"; do
      [[ "${oks[$i]}" = 'ok' ]] || continue
      nv=${nvs[$i]}
      "$0" "${nv#*@}" "$modifier"
    done
    exit $? ;;

  build | test)
set -x
    # Keep target dir within PWD so it emulates a local build somewhat
    tmptrgt=$PWD/target/tmp-$arg1
    tmplogs=$tmptrgt.logs.txt
    mkdir -p "$tmptrgt"
    if [[ "$clean" = '1' ]]; then rm -rf "$tmptrgt" || exit 1; fi
    CARGO_TARGET_DIR=$install_dir cargo install --locked --frozen --offline --force --root=$install_dir --path="$PWD"/cargo-green
    ls -lha $install_dir/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs
    xdg-terminal-exec tail -f $tmplogs
    echo "$arg1"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    CARGOGREEN_LOG=trace \
    RUSTCBUILDX_LOG_PATH="$tmplogs" \
    RUSTCBUILDX_CACHE_IMAGE="${RUSTCBUILDX_CACHE_IMAGE:-}" \
    PATH=$install_dir/bin:"$PATH" \
      \cargo green -v fetch
    CARGOGREEN_LOG=trace \
    RUSTCBUILDX_LOG_PATH="$tmplogs" \
    RUSTCBUILDX_CACHE_IMAGE="${RUSTCBUILDX_CACHE_IMAGE:-}" \
    PATH=$install_dir/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \
      \cargo green -v $arg1 --jobs=${jobs:-$(nproc)} --all-targets --all-features --locked --frozen --offline
    #if [[ "$clean" = 1 ]]; then docker buildx du --builder=supergreen --verbose | tee --append "$tmplogs" || exit 1; fi
    # TODO: tag/label buildx storage so things can be deleted with fine filters
    maybe_show_logs "$tmplogs"
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

session_name=$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")
tmptrgt=/tmp/clis-$session_name
tmplogs=$tmptrgt.logs.txt
tmpgooo=$tmptrgt.state
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


    # '&&' '[[ $(expr $(date +%s) - $(stat -c %Y' /home/pete/.cargo/bin/cargo-green ')) -lt 3 ]]' \


gitdir=$(realpath "$(dirname "$(dirname "$0")")")
send \
  CARGO_TARGET_DIR=$install_dir cargo install --locked --frozen --offline --force --root=$install_dir --path="$gitdir"/cargo-green \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

send \
  'until' '[[' -f "$tmpgooo".installed ']];' \
  'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* '&&' \
  CARGOGREEN_LOG=trace \
  RUSTCBUILDX_LOG_PATH="$tmplogs" \
  RUSTCBUILDX_CACHE_IMAGE="${RUSTCBUILDX_CACHE_IMAGE:-}" \
  PATH=$install_dir/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \cargo green -vv install --timings --jobs=${jobs:-1} --root=$tmpbins --locked --force "$(as_install "$name_at_version")" "$args" \
  '&&' 'if' '[[' "$clean" '=' '1' ']];' 'then' docker buildx du --builder=supergreen --verbose '|' tee --append "$tmplogs" '||' 'exit' '1;' 'fi' \
  '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"

echo "$name_at_version"
echo "Target dir: $tmptrgt"
echo "Logs: $tmplogs"
maybe_show_logs "$tmplogs"
