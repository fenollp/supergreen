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
# Usage:   final=1 $0 ..                           #=> Generate final Containerfile
#
# Usage:    DOCKER_HOST=.. $0 ..                   #=> Overrides machine
# Usage: BUILDX_BUILDER=.. $0 ..                   #=> Overrides builder (set to "empty" to set BUILDX_BUILDER='')

# TODO: test other runtimes: runc crun containerd buildkit-rootless lima colima
# TODO: set -x in ci

# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;           oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.21.1;          oks[i]=ko; nvs_args[i]='--features=fix' # TODO: re-ok when GitHub Actions runners update to patched BuildKit (>=v0.20)
((i+=1)); nvs[i]=cargo-bpf@2.3.0;             oks[i]=ko; nvs_args[i]='' # (No libelf-dev installed on host) (Wrapper compiles successfully) Build script fails to run: Running `CARGO=.. .../bpf-sys-c62ba29dc4f555d9/build-script-build` ... error: gelf.h: No such file => TODO: see about overriding RUSTC_LINKER=/usr/bin/clang
((i+=1)); nvs[i]=cargo-deny@0.16.1;           oks[i]=ko; nvs_args[i]='' # TODO: re-ok when GitHub Actions runners update to patched BuildKit (>=v0.20)
((i+=1)); nvs[i]=cargo-fuzz@0.12.0;           oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET_PLATFORM` not defined at compile time .. current_platform-0.2.0 + HOST_PLATFORM
((i+=1)); nvs[i]=cargo-green@main;            oks[i]=ok; nvs_args[i]='--git https://github.com/fenollp/supergreen.git --branch=main cargo-green'
((i+=1)); nvs[i]=cargo-llvm-cov@0.5.36;       oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.72;        oks[i]=ko; nvs_args[i]='' # .. environment variable `TARGET` not defined at compile time .. self_update-0.39.0
((i+=1)); nvs[i]=cross@0.2.5;                 oks[i]=ok; nvs_args[i]='--git https://github.com/cross-rs/cross.git --tag=v0.2.5 cross'
((i+=1)); nvs[i]=dbcc@2.2.1;                  oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=diesel_cli@2.3.2;            oks[i]=ok; nvs_args[i]='--no-default-features --features=postgres'
((i+=1)); nvs[i]=hickory-dns@0.25.0-alpha.1;  oks[i]=ok; nvs_args[i]='--features=dns-over-rustls'
((i+=1)); nvs[i]=mussh@3.1.3;                 oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ntpd@1.2.3;                  oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=qcow2-rs@0.1.2;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=ripgrep@14.1.0;              oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=rublk@0.2.0;                 oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=shpool@0.6.2;                oks[i]=ko; nvs_args[i]='' # sudo apt-get install libpam0g-dev
((i+=1)); nvs[i]=statehub@0.14.10;            oks[i]=ko; nvs_args[i]='' # BUG: unexpected crate-type: 'cdylib'
((i+=1)); nvs[i]=torrust-index@3.0.0;         oks[i]=ko; nvs_args[i]='' # /usr/bin/ld: cannot find -lssl: No such file or directory
((i+=1)); nvs[i]=cargo-authors@0.5.5;         oks[i]=ko; nvs_args[i]='' # ERROR: failed to solve: ResourceExhausted: ResourceExhausted: ResourceExhausted: ResourceExhausted: ResourceExhausted: grpc: received message larger than max (6653826 vs. 4194304)
((i+=1)); nvs[i]=vixargs@0.1.0;               oks[i]=ok; nvs_args[i]=''
((i+=1)); nvs[i]=cargo-config2@0.1.34;        oks[i]=ok; nvs_args[i]='--example=get'
((i+=1)); nvs[i]=privaxy@0.5.2;               oks[i]=ko; nvs_args[i]='--git https://github.com/Barre/privaxy.git --tag=v0.5.2 privaxy' # undefined reference to `__isoc23_strtol'\n          /usr/bin/ld: rand_unix.c:(.text.wait_random_seeded+0x204): undefined reference to `__isoc23_strtol'\n          collect2: error: ld returned 1 exit status\n          \n  = note: some `extern` functions couldn't be found; some native libraries may need to be installed or have their path specified\n  = note: use the `-l` flag to specify native libraries to link\n  = note: use the `cargo:rustc-link-lib` directive to specify the native libraries to link with Cargo (see https://doc.rust-lang.org/cargo/reference/build-scripts.html#rustc-link-lib

((i+=1)); nvs[i]=miri@master;                 oks[i]=ko; nvs_args[i]='--git https://github.com/rust-lang/miri.git --rev=dcd2112' # can't find crate for `either`
((i+=1)); nvs[i]=zed@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/zed-industries/zed.git --tag=v0.179.5 zed' # error: could not download file from 'https://static.rust-lang.org/dist/channel-rust-1.85.toml.sha256'
((i+=1)); nvs[i]=verso@main;                  oks[i]=ko; nvs_args[i]='--git https://github.com/versotile-org/verso.git --rev 62e3085 verso' # error: could not download file from 'https://static.rust-lang.org/dist/channel-rust-1.85.0.toml.sha256'
((i+=1)); nvs[i]=cargo-udeps@0.1.55;          oks[i]=hm; nvs_args[i]=''

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


((i+=1)); nvs[i]=coccinelleforrust@main;      oks[i]=ko; nvs_args[i]='--git https://gitlab.inria.fr/coccinelle/coccinelleforrust.git --rev=b06ba306 coccinelleforrust' #--branch=ctl2
# could not download file from 'https://static.rust-lang.org/dist/channel-rust-nightly.toml.sha256' to '/usr/local/rustup/tmp/dmbquji61f3vgrgv_file':
#   failed to make network request: error sending request for url (https://static.rust-lang.org/dist/channel-rust-nightly.toml.sha256):
#     client error (Connect): dns error: failed to lookup address information: Temporary failure in name resolution: failed to lookup address information: Temporary failure in name resolution
#=> has a toolchain file requiring nightly but --network=none
#==> see (1) adapt this file to `cargo +nightly install` (2) pass env to allow network (3) at each crate, read toolchain file to auto-add a corresponding stage

((i+=1)); nvs[i]=ipa@main;                    oks[i]=ko; nvs_args[i]='--git https://github.com/seekbytes/IPA.git --rev=3094f92 ipa' # environment variable `MIME_TYPES_GENERATED_PATH` not defined at compile time


# ((i+=1)); nvs[i]=cargo-tally@1.0.48;          oks[i]=ok; nvs_args[i]='' #
# ((i+=1)); nvs[i]=cargo-mutants@24.7.1;        oks[i]=ok; nvs_args[i]='' #
# ((i+=1)); nvs[i]=binsider@0.2.0;              oks[i]=ok; nvs_args[i]='' #

((i+=1)); nvs[i]=stu@0.7.1;                   oks[i]=ko; nvs_args[i]='' # BUG: unexpected crate-type: 'cdylib' error: could not compile `crc64fast-nvme` (lib)


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
  [[ $# -eq 0 ]]
  cat <<EOF
on: [push]
name: CLIs
jobs:


$(jobdef 'meta-check')
    steps:
    - uses: actions/checkout@v5
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
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: stable
        cache-all-crates: true
        cache-workspace-crates: true

    - uses: actions/checkout@v5

$(while read -r name path; do
  cat <<EOW
    - name: Cache \`cargo fetch\` $name
      uses: actions/cache@v4
      with:
        path: $path
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-$name-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-$name-

EOW
  done < <(printf '%s ~/.cargo/registry/index/\n%s ~/.cargo/registry/cache/\n%s ~/.cargo/git/db/\n' index cache gitdb)
)

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

    - uses: actions/upload-artifact@v4
      with:
        name: cargo-green
        path: /home/runner/.cargo/bin/cargo-green

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
    cargo-authors@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,zlib1g-dev') ;;
    cargo-udeps@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,zlib1g-dev') ;;
    dbcc@*) envvars+=(CARGOGREEN_SET_ENVS='TYPENUM_BUILD_CONSTS,TYPENUM_BUILD_OP') ;;
    diesel_cli@*) envvars+=(CARGOGREEN_ADD_APT='libpq-dev') ;;
    hickory-dns@*) envvars+=(CARGOGREEN_SET_ENVS='RING_CORE_PREFIX') ;;
    mussh@*) envvars+=(CARGOGREEN_ADD_APT='libsqlite3-dev,libssl-dev,zlib1g-dev') ;;
    ntpd@*) envvars+=(CARGOGREEN_SET_ENVS='NTPD_RS_GIT_DATE,NTPD_RS_GIT_REV,RING_CORE_PREFIX') ;;
    # cargo-bpf@*) envvars+=(CARGOGREEN_ADD_APT='libelf-dev') ;;
    # privaxy@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,openssl' CARGOGREEN_SET_ENVS='DEP_OPENSSL_LIBRESSL_VERSION_NUMBER,DEP_OPENSSL_VERSION_NUMBER' CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/rust:1) ;;
    # shpool@*) envvars+=(CARGOGREEN_ADD_APT='libpam0g-dev') ;;
    # torrust-index@*) envvars+=(CARGOGREEN_ADD_APT='libssl-dev,zlib1g-dev' CARGOGREEN_SET_ENVS='MIME_TYPES_GENERATED_PATH,RING_CORE_PREFIX') ;;
    # stu@*) envvars+=(CARGOGREEN_SET_ENVS=RING_CORE_PREFIX) ;;
    *) ;;
  esac
  if [[ -n "${CARGOGREEN_CACHE_IMAGES:-}" ]]; then
    envvars+=(CARGOGREEN_CACHE_IMAGES="$CARGOGREEN_CACHE_IMAGES")
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
  local jobs=$1; shift
  local envvars=()
 as_env "$name_at_version"

	cat <<EOF
$(jobdef "$(slugify "$name_at_version")_$jobs")
    continue-on-error: \${{ matrix.toolchain != 'stable' }}
    strategy:
      matrix:
        toolchain:
        - stable
        - 1.86.0
    env:
      CARGO_TARGET_DIR: /tmp/clis-$(slugify "$name_at_version")
      CARGOGREEN_FINAL_PATH: recipes/$name_at_version.Dockerfile
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
        cache-all-crates: true
        cache-workspace-crates: true
$(
	case "$name_at_version" in
		cargo-llvm-cov@*) printf '    - run: rustup component add llvm-tools-preview\n' ;;
		*) ;;
	esac
)

$(restore_bin)
    - uses: actions/checkout@v5
$(rundeps_versions)

    - name: Envs
      run: ~/.cargo/bin/cargo-green green supergreen env
    - if: \${{ matrix.toolchain != 'stable' }}
      run: ~/.cargo/bin/cargo-green green supergreen env CARGOGREEN_BASE_IMAGE | grep '\${{ matrix.toolchain }}'
    - name: Envs again
      run: ~/.cargo/bin/cargo-green green supergreen env

$(cache_usage)
    - name: cargo install net=ON cache=OFF remote=OFF jobs=$jobs
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _
    - name: cargo install net=ON cache=ON remote=OFF jobs=1
      if: \${{ failure() }}
      run: |
        rm _
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --jobs=1 --locked --force $(as_install "$name_at_version") $@ |& tee _
    - if: \${{ matrix.toolchain != 'stable' }}
      uses: actions/upload-artifact@v4
      name: Upload recipe
      with:
        name: $name_at_version.Dockerfile
        path: \${{ env.CARGOGREEN_FINAL_PATH }}
$(postconds _)
$(cache_usage)

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh \$CARGO_TARGET_DIR || true

    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        env ${envvars[@]} \\
          cargo green -vv install --jobs=$jobs --locked --force $(as_install "$name_at_version") $@ |& tee _
$(postcond_fresh _)
$(postconds _)
$(cache_usage)

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh \$CARGO_TARGET_DIR || true

EOF
}

maybe_show_logs() {
  local logfile=$1; shift
  [[ $# -eq 0 ]]
  case "$(wc "$logfile")" in '0 0 0 '*) ;;
                                     *) $PAGER "$logfile" ;; esac
}

# No args: generate CI file
if [[ $# = 0 ]]; then
  header

  for i in "${!nvs[@]}"; do
    [[ "${oks[$i]}" = 'ko' ]] && continue
    name_at_version=${nvs["$i"]}
    case "$name_at_version" in
      cargo-green@*) continue ;;
    esac
    # 3: https://docs.github.com/en/actions/using-github-hosted-runners/about-github-hosted-runners/about-github-hosted-runners#standard-github-hosted-runners-for-public-repositories
    cli "$name_at_version" 3 "${nvs_args["$i"]}"
  done

  exit
fi


arg1=$1; shift

rmrf=${rmrf:-0}
reset=${reset:-0}
[[ "${clean:-0}" = 1 ]] && rmrf=1 && reset=1
jobs=${jobs:-$(nproc)}
frozen=--locked ; [[ "${offline:-}" = '1' ]] && frozen=--frozen
final=${final:-0}

case "${BUILDX_BUILDER:-}" in
  '') BUILDX_BUILDER=supergreen ;;
  'empty') BUILDX_BUILDER= ;;
  *) ;;
esac

install_dir=$repo_root/target
CARGO=${CARGO:-cargo}

# Special first arg handling..
case "$arg1" in
  # Try all, sequentially
  ok)
    for i in "${!nvs[@]}"; do
      case "${oks[$i]}" in ok|hm) ;; *) continue ;; esac
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
    CARGO_TARGET_DIR=$install_dir $CARGO install $frozen --force --root=$install_dir --path="$PWD"/cargo-green
    ls -lha $install_dir/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs

    case "$OSTYPE" in
      darwin*) osascript -e "$(printf 'tell app "Terminal" \n do script "tail -f %s" \n end tell' $tmplogs)" ;;
      *)       xdg-terminal-exec tail -f $tmplogs ;;
    esac

    echo "$arg1"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    CARGOGREEN_LOG=trace \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green-fetched.Dockerfile" \
    PATH=$install_dir/bin:"$PATH" \
      $CARGO green -v fetch
    CARGOGREEN_LOG=trace \
    CARGOGREEN_LOG_PATH="$tmplogs" \
    CARGOGREEN_FINAL_PATH="$tmptrgt/cargo-green.Dockerfile" \
    PATH=$install_dir/bin:"$PATH" \
    CARGO_TARGET_DIR="$tmptrgt" \
      $CARGO green -v $arg1 --jobs=$jobs --all-targets --all-features $frozen -p cargo-green
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

session_name=$(slugify "$name_at_version")$(slugify "${DOCKER_HOST:-_}")
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
    '&&' CARGO_TARGET_DIR=$install_dir $CARGO install $frozen --force --root=$install_dir --path="$gitdir"/cargo-green \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

envvars=(CARGOGREEN_LOG=trace)
envvars+=(CARGOGREEN_LOG_PATH="$tmplogs")
envvars+=(PATH=$install_dir/bin:"$PATH")
envvars+=(CARGO_TARGET_DIR="$tmptrgt")
if [[ "$final" = '1' ]]; then
  envvars+=(CARGOGREEN_FINAL_PATH=recipes/$name_at_version.Dockerfile)
  envvars+=(CARGOGREEN_FINAL_PATH_NONPRIMARY=1)
fi
# envvars+=(CARGOGREEN_SYNTAX_IMAGE=docker-image://docker.io/docker/dockerfile:1@sha256:4c68376a702446fc3c79af22de146a148bc3367e73c25a5803d453b6b3f722fb)
# envvars+=(CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/rust:1.86.0-slim@sha256:3f391b0678a6e0c88fd26f13e399c9c515ac47354e3cadfee7daee3b21651a4f)
as_env "$name_at_version"
send \
  'until' '[[' -f "$tmpgooo".installed ']];' 'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* \
  '&&' 'if' '[[' "$reset" '=' '1' ']];' 'then' docker buildx rm "$BUILDX_BUILDER" --force '||' 'exit' '1;' 'fi' \
  '&&' 'case' "$name_at_version" 'in' ntpd'@*)' export NTPD_RS_GIT_REV=$ntpd_locked_commit '&&' export NTPD_RS_GIT_DATE=$ntpd_locked_date ';;' '*)' ';;' 'esac' \
  '&&' "${envvars[@]}" $CARGO green -vv install --timings --jobs=$jobs --root=$tmpbins $frozen --force "$(as_install "$name_at_version")" "$args" \
  '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"

echo "$name_at_version"
echo "Target dir: $tmptrgt"
echo "Logs: $tmplogs"
maybe_show_logs "$tmplogs"
