#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")
source "$repo_root"/hack/ck.sh
source "$repo_root"/hack/binaries.sh

# Usage:           $0                              #=> generate .github/workflows/clis.yml
#
# Usage:           $0 ( <name@version> | <name> )  #=> cargo install name@version
# Usage:           $0   ok                         #=> cargo install all working bins
#
# Usage:           $0 ( build | package | test )   #=> cargo $cmd ./cargo-green
#
# Usage:    jobs=1 $0 ..                           #=> cargo --jobs=$jobs
# Usage: offline=1 $0 ..                           #=> cargo --frozen (defaults to just: --locked)
# Usage:    rmrf=0 $0 ..                           #=> Don't rm -rf $CARGO_TARGET_DIR/* before cargo ...
# Usage:   reset=1 $0 ..                           #=> cargo green supergreen builder rm --clean; cargo ...
# Usage:   clean=1 $0 ..                           #=> Both reset=1 + rmrf=1
# Usage:   final=0 $0 ..                           #=> Don't generate final Containerfile
# Usage:   build=0 $0 ..                           #=> Use already installed cargo-green
# Usage: release=0 $0 ..                           #=> Install cargo-green with --debug (quicker to build, slower to run)
# Usage:     cov=1 $0 ..                           #=> Aggregate coverage (from an instrumented cargo-green, see ./hack/cov.sh)
# Usage:       x=1 $0 ..                           #=> set -x
#
# Usage:          CARGO=.. $0 ..                   #   CARGO='nightly' $0 ..
# Usage:    DOCKER_HOST=.. $0 ..                   #=> Overrides machine
# Usage: BUILDX_BUILDER=.. $0 ..                   #=> Overrides builder (set to "empty" to set BUILDX_BUILDER='')

# TODO: test other runtimes: runc crun containerd buildkit-rootless lima colima
# * CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:buildx-stable-1-rootless"
#   * https://github.com/docker/setup-docker-action testing rootless and containerd
# * a matrix of earlier and earlier versions of: buildkit x buildx/docker x cargo/rustc
# * a local + cached DockerHub proxy

as_env() {
  local name_at_version=$1; shift
  [[ $# -eq 0 ]]
  local envvars_bin; mapfile -t envvars_bin < <(bin_envs "$name_at_version")
  envvars+=("${envvars_bin[@]}")

# # zed's proto crate uses prost-build 0.9 which execs its own bundled protoc binary
# # from within its crate sources => mount build scripts' deps' sources.
# export CARGOGREEN_EXPERIMENT=buildscriptsources,finalpathnonprimary
# export CARGOGREEN_FINAL_PATH=recipes/zed@main.Dockerfile

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
  elif [[ "${fromcli:-0}" = 0 ]]; then
    envvars+=(CARGOGREEN_LOG=debug)
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

# Binary-specific envs are passed to `env` as-is (not via $GITHUB_ENV: that would keep literal quotes
# and GITHUB_* names such as GITHUB_SHA can't be overridden there)
bin_envs_snippet() {
  [[ $# -eq 0 ]]
  printf '%s\n' "mapfile -t envs < <(jq -r '(. // {}) | to_entries[] | \"\\(.key)=\\(.value)\"' <<<'\${{ toJSON(matrix.bin.envs) }}')"
}

# Prints the single workflow running every CI binary (see ./hack/binaries.sh) as a matrix
workflow() {
  [[ $# -eq 0 ]]
  local registry=/tmp/.local-registry
  local registry_new=$registry-new
  local root=/tmp
  local bins; bins=$("$repo_root"/hack/binaries.sh json | jq -c '.[]' | sed 's/^/        - /')

  # binaries.sh only lists what is not a default and what can't be computed from nv:
  local cargo_x="\${{ matrix.bin.cargo || 'cargo' }}"
  local args_x="\${{ matrix.bin.args }}"
  # GitHub expressions have no string replace function: these come from the "Names" step
  local install_x="\$INSTALL"

  cat <<EOF
on:
  push:
  # Lets closing then reopening a PR start CI: pushes made with GITHUB_TOKEN (e.g. the audit-fix PR) trigger no push event.
  pull_request:
    types: [reopened]
name: CLIs
permissions: {}
jobs:


$(bin_job)

$(jobdef cli)
    name: \${{ matrix.bin.nv }} \${{ matrix.toolchain }}
    continue-on-error: \${{ matrix.toolchain != '$stable' }}
    strategy:
      fail-fast: false
      matrix:
        toolchain:
        - $stable
        - $fixed
        exclude:
        - toolchain: \${{ github.ref != 'refs/heads/main' && '$stable' }}
        # Generated by ./hack/binaries.sh json
        bin:
$bins
    env:
    # CARGOGREEN_CACHE_FROM_IMAGES: docker-image://localhost:12345/\${{ github.repository }}
    # CARGOGREEN_CACHE_TO_IMAGES: docker-image://localhost:23456/\${{ github.repository }}
      CARGOGREEN_FINAL_PATH: recipes/\${{ matrix.bin.nv }}.Dockerfile
      CARGOGREEN_EXPERIMENT: finalpathnonprimary # dumps on each build call
      CARGOGREEN_LOG: debug
      CARGOGREEN_LOG_PATH: logs.txt
    needs: bin
    steps:
    - name: Names
      # Same as as_install and slugify (hack/binaries.sh)
      run: |
        nv='\${{ matrix.bin.nv }}'
        install=\${nv%@main}
        install=\${install%@master}
        slug=\$(sed 's%@%_%g;s%+%_%g;s%\\.%-%g;s%/%%g;s%:%%g' <<<"\$nv")
        echo "INSTALL=\$install" >>"\$GITHUB_ENV"
        echo "CARGO_TARGET_DIR=/tmp/clis-\$slug" >>"\$GITHUB_ENV"
    - name: Cache the Rust toolchain
      uses: $action__cache
      with:
        path: |
          ~/.rustup/toolchains/
          ~/.rustup/update-hashes/
        key: \${{ github.job }}-\${{ runner.os }}-rustup-\${{ matrix.toolchain }}
    - name: Install the Rust toolchain
      # Not actions-rust-lang/setup-rust-toolchain: it would only add caching of target/ (cargo-green's job) and annotations
      run: |
        rustup toolchain install \${{ matrix.toolchain }} --profile minimal --no-self-update
        rustup default \${{ matrix.toolchain }}


$(restore_bin)
$(restore_builder_data)
    - uses: $action__checkout
      with:
        persist-credentials: false
$(login_to_readonly_hub)

    - name: Cache \`cargo fetch\` (per CLI)
      uses: $action__cache
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ matrix.bin.sha1 }}

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
        key: localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ matrix.bin.nv }}-\${{ github.run_id }}
        restore-keys: |
          localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ matrix.bin.nv }}-
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
    - name: 🔵 $cargo_x green install
      id: do-try
      timeout-minutes: 11
      continue-on-error: true
      run: |
$(unset_action_envs)
        $(bin_envs_snippet)
        env "\${envs[@]}" \\
          $cargo_x green -vv install --locked --force --root=$root $install_x $args_x |& tee _
    - name: 🔵 $cargo_x green install --jobs=1
      id: do-try-jobs1
      timeout-minutes: 11
      if: \${{ steps.do-try.outcome == 'failure' }}
      run: |
$(unset_action_envs)
        $(bin_envs_snippet)
        env "\${envs[@]}" \\
          $cargo_x green -vv install --jobs=1 --locked --force --root=$root $install_x $args_x |& tee _ && printf '' >_
    - if: \${{ always() && matrix.toolchain != '$stable' }}
      uses: $action__upload_artifact
      name: Upload recipe
      with:
        name: \${{ matrix.bin.nv }}.Dockerfile
        path: \${{ env.CARGOGREEN_FINAL_PATH }}
        if-no-files-found: error
$(postconds _)
$(cache_usage)
$(disk_usage)
$(check_bin_help_and_set_hash "$root" '${{ matrix.bin.bin }}')

    - name: 🔵 Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        $(bin_envs_snippet)
        env "\${envs[@]}" \\
          $cargo_x green -vv install --locked --force --root=$root $install_x $args_x |& tee _
$(postcond_fresh _)
$(postconds _)
$(check_bin_hash "$root" '${{ matrix.bin.bin }}')

    - name: 🔵 Try CARGOGREEN_RUNNER=none with the same command twice without modifications...
      run: |
$(unset_action_envs)
        $(bin_envs_snippet)
        env "\${envs[@]}" CARGOGREEN_RUNNER=none \\
          $cargo_x green -vv install --locked --force --root=$root $install_x $args_x |& tee _
$(postcond_fresh _)
$(postconds _)
$(check_bin_hash "$root" '${{ matrix.bin.bin }}')

    - run: rm -rf $root/bin/* \$CARGO_TARGET_DIR/* >/dev/null
    - name: 🔵 Reuse locally cached results...
      run: |
$(unset_action_envs)
        $(bin_envs_snippet)
        env "\${envs[@]}" CARGOGREEN_RUNNER=none \\
          $cargo_x green -vv install --locked --force --root=$root $install_x $args_x |& tee _
    - name: ...is blazingly fearlessly lightspeed fast
      run: |
        grep Finished ./_ || exit 1
        grep Finished ./_ | grep -E ....s || exit 1
$(postconds _)
$(check_bin_hash "$root" '${{ matrix.bin.bin }}')

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
  #     key: localprivatereg-\${{ runner.os }}-\${{ matrix.toolchain }}-\${{ matrix.bin.nv }}-\${{ github.run_id }}

$(cache_usage)
$(disk_usage)

EOF
}

# No args: generate the CI workflow, a matrix over ./hack/binaries.sh
#   debug webui at https://github.com/fenollp/supergreen/actions/workflows/clis.yml
if [[ $# = 0 ]]; then
  workflow >"$repo_root"/.github/workflows/clis.yml
  populate_binaries
  slows=() ; actual=0
  for i in "${!nvs[@]}"; do
    if in_ci "$i"; then ((actual+=1)); elif [[ "${oks[$i]:0:1}" = O ]]; then slows+=("${nvs[$i]}"); fi
  done
  echo Too slow to use:
  printf '  %s\n' "${slows[@]}" | sort
  echo Produced: "$actual" / "$((actual + ${#slows[@]}))" jobs in 1 workflow
  exit
fi


arg1=$1; shift

rmrf=${rmrf:-1}
reset=${reset:-0}
[[ "${clean:-0}" = 1 ]] && rmrf=1 && reset=1
jobs=${jobs:-''} ; [[ "$jobs" != '' ]] && jobs="--jobs=$jobs"
frozen=--locked ; [[ "${offline:-}" = '1' ]] && frozen=--frozen
final=${final:-1}
build=${build:-1}
release=${release:-1} ; [[ "$release" = 1 ]] && release='' ; [[ "$release" = 0 ]] && release=--debug
cov=${cov:-0}
x=${x:-0} ; [[ "$x" = 1 ]] && x='set -x' ; [[ "$x" = 0 ]] && x=:

envvars=()

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
envvars+=(PATH=$shortPATH)

if [[ "$cov" = 1 ]]; then
  "$repo_root"/hack/cov.sh install "$install_dir"/bin
  build=0
  envvars+=("$("$repo_root"/hack/cov.sh env)")
fi


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
      name_at_version=${nvs[$i]}
      "$0" "${name_at_version#*@}"
    done
    exit $? ;;

  build | package | test)
    $x
    # Keep target dir within PWD so it emulates a local build somewhat
    tmptrgt=$PWD/target/tmp-$arg1
    tmplogs=$tmptrgt.logs.txt
    mkdir -p "$tmptrgt"
    if [[ "$rmrf" = '1' ]]; then rm -rf "$tmptrgt"/* || exit 1; fi
    if [[ "$reset" = 1 ]]; then cargo green supergreen builder rm --clean || exit 1; fi
    if [[ "$build" = 1 ]]; then CARGO_TARGET_DIR=$install_dir cargo install $release $frozen --force --root=$install_dir --path="$PWD"/cargo-green || exit 1; fi
    ls -lha $install_dir/bin/cargo-green
    rm $tmplogs >/dev/null 2>&1 || true
    touch $tmplogs

    # case "$OSTYPE" in
    #   darwin*) osascript -e "$(printf 'tell app "Terminal" \n do script "tail -f %s" \n end tell' $tmplogs)" ;;
    #   *)       xdg-terminal-exec tail -f $tmplogs ;;
    # esac

    cargo=cargo ; [[ "${CARGO:-}" != '' ]] && cargo="cargo +$CARGO"

    envvars+=(CARGO_TARGET_DIR=$tmptrgt)
    envvars+=(CARGOGREEN_EXPERIMENT=finalpathnonprimary)
    envvars+=(CARGOGREEN_FINAL_PATH=$tmptrgt/cargo-green.Dockerfile)
    envvars+=(CARGOGREEN_LOG_PATH=$tmplogs)
    as_env cargo-green

    echo "$arg1"
    echo "Target dir: $tmptrgt"
    echo "Logs: $tmplogs"
    env "${envvars[@]}" $cargo green $arg1 $jobs --all-features $frozen -p cargo-green
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
    name_at_version=${nvs[$picked]}
    args=${nvs_args[$picked]}
    cargo="cargo +${toolchain["$picked"]:-${CARGO:-$fixed}}"
    timings=; [[ '1.61' = "$(printf "%s\n1.61\n" "${cargo/cargo +}" | sort -V | head -n1)" ]] && timings=--timings
    ;;
esac


session_name=$(slugify "$name_at_version") #$(slugify "${DOCKER_HOST:-}")
tmptrgt=/tmp/clis-$session_name
tmplogs=$tmptrgt.logs.txt
tmpgooo=$tmptrgt.state
tmpbins=/tmp

envvars+=(CARGO_INCREMENTAL=0)
envvars+=(CARGOGREEN_LOG_PATH="$tmplogs")
envvars+=(CARGO_TARGET_DIR="$tmptrgt")
if [[ "$final" = '1' ]]; then
  envvars+=(CARGOGREEN_FINAL_PATH=recipes/$name_at_version.Dockerfile)
  envvars+=(CARGOGREEN_EXPERIMENT=finalpathnonprimary) #,finalpathcomments)
fi



RUNNER_PATCHED=${RUNNER_PATCHED:-0}                    # Use custom BuildKit+buildx
if [[ $RUNNER_PATCHED = 1 ]]; then
  $x
  RUNNER_PATCHED_REBUILD=${RUNNER_PATCHED_REBUILD:-0}  # Force patch_and_build_buildkit & _buildx
  HUBACCOUNT=${HUBACCOUNT:-fenollexai}                 # DockerHub account hosting the patched images
  MSG_SIZE_MULT=${MSG_SIZE_MULT:-8}                    # Multiply the 16 MiB default (8*16 -> 128 MiB)

  BUILDKIT_REF=9037f21f69a23c8839693910d66ef9e502f63c09
  BUILDKIT_REF=${BUILDKIT_REF:-$(cat "$repo_root"/cargo-green/latest_buildkit.txt | sed -E 's/([0-9]+[.][0-9]+).+/v\1/')}
  TAG=patched-$BUILDKIT_REF
  BUILDER=$HUBACCOUNT/moby_buildkit:$TAG
  FRONTEND=$HUBACCOUNT/docker_dockerfile:$TAG

  patch_msgsize() {
    local dir=$1; shift
    [[ $# -eq 0 ]]
    for f in $(grep -rlE 'defaults.DefaultMax(Recv|Send)MsgSize' "$dir"); do
      case "$f" in
#buildx
        # */vendor/github.com/moby/buildkit/client/client.go) ;; # needed
        */vendor/github.com/moby/buildkit/session/grpc.go) continue ;; #not needed

#buildkit
        # */client/client.go) continue ;; # needed
        # */cmd/buildkitd/main.go) continue ;; # needed
        # */cmd/buildkitd/main_oci_worker.go) continue ;; # not needed (weirdly)
        # */frontend/gateway/gateway.go) continue ;; # needed
        # */frontend/gateway/grpcclient/client.go) continue ;; # needed
        */session/grpc.go) continue ;; # not needed
        */vendor/github.com/containerd/containerd/v2/client/client.go) continue ;; # not needed
      esac

      sed -i -E \
        -e "s/\((defaults\.DefaultMaxRecvMsgSize)\)/($MSG_SIZE_MULT * \1)/g" \
        -e "s/\((defaults\.DefaultMaxSendMsgSize)\)/($MSG_SIZE_MULT * \1)/g" \
        "$f"
    done
  }

  patch_and_build_buildkit() {
    local SRC_DIR=$repo_root/target/cargo-green-patched/buildkit-src
    mkdir -p "$SRC_DIR"
    rm -rf "$SRC_DIR"
    git clone --quiet --depth=1 https://github.com/moby/buildkit.git "$SRC_DIR"
    pushd "$SRC_DIR"
    git fetch --quiet --depth=1 origin "$BUILDKIT_REF"
    git checkout --quiet "$BUILDKIT_REF"
    popd
    patch_msgsize "$SRC_DIR"
    docker buildx build "$SRC_DIR" --target buildkit --push --load \
      --tag "$BUILDER"
    docker buildx build "$SRC_DIR" --push --load --file "$SRC_DIR/frontend/dockerfile/cmd/dockerfile-frontend/Dockerfile" \
      --tag "$FRONTEND"
  }

  patch_and_build_buildx() {
    local ref=v0.35.0
    local SRC_DIR=$repo_root/target/cargo-green-patched/buildx-src
    mkdir -p "$SRC_DIR"
    rm -rf "$SRC_DIR"
    git clone --quiet --depth=1 --branch="$ref" https://github.com/docker/buildx "$SRC_DIR"
    patch_msgsize "$SRC_DIR"
    pushd "$SRC_DIR"
    go build -mod=vendor -trimpath \
      -ldflags "-X github.com/docker/buildx/version.Version=patched-$ref -X github.com/docker/buildx/version.Revision=a319e5b15052cf6557ceb666eb8ff6e32380b782 -X github.com/docker/buildx/version.Package=github.com/docker/buildx" \
      -o bin/docker-buildx ./cmd/buildx
    popd
    cp "$SRC_DIR"/bin/docker-buildx ~/.docker/cli-plugins/
    docker buildx version | grep -F "$ref"
  }

  if [[ $RUNNER_PATCHED_REBUILD = 1 ]]; then
    patch_and_build_buildkit
    patch_and_build_buildx
  fi
  builder_image="$(docker inspect -f '{{range .RepoDigests}}{{.}}{{end}}' "$BUILDER")"
  syntax_image="$(docker inspect -f '{{range .RepoDigests}}{{.}}{{end}}' "$FRONTEND")"
  if [[ $RUNNER_PATCHED_REBUILD = 1 ]]; then
    docker buildx rm --builder=superpatched --force || true
    # NOTE: missing many settings with just this command:
    docker buildx create --bootstrap --name=superpatched --driver=docker-container --driver-opt=image="$builder_image"
  fi
  envvars+=(CARGOGREEN_BUILDER_IMAGE=docker-image://$builder_image)
  envvars+=(CARGOGREEN_SYNTAX_IMAGE=docker-image://$syntax_image)
  envvars+=(BUILDX_BUILDER=superpatched)
  set +x
fi



if [[ "$rmrf" = '1' ]]; then
  rm -rf "$tmptrgt"/*
fi

rm -f "$tmpgooo".*
# Always target our own session ("=" is an exact match): untargeted tmux commands hit whichever pane/session
# is current, e.g. the one running this very script when it is itself inside tmux.
tmux new-session -d -s "$session_name"
tmux_t="=$session_name:"
tmux select-window -t "=$session_name:0"

send() {
  tmux send-keys -t "$tmux_t" " $* && tmux select-layout -t \"$tmux_t\" even-vertical && exit" C-m
}

gitdir=$(realpath "$(dirname "$(dirname "$0")")")
send \
  "$x" \
    '&&' 'if' '[[ ' "$build" = 1 ']];' 'then' CARGO_TARGET_DIR=$install_dir cargo install $release $frozen --force --root=$install_dir --path="$gitdir"/cargo-green '||' 'exit' '1;' 'fi' \
    '&&' touch "$tmpgooo".installed \
    '&&' "rm $tmplogs >/dev/null 2>&1; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout -t "$tmux_t" even-vertical
tmux split-window -t "$tmux_t"

# RUSTFLAGS="--remap-path-prefix=$tmptrgt="

as_env "$name_at_version"
send \
  'until' '[[' -f "$tmpgooo".installed ']];' 'do' sleep '.1;' 'done' '&&' rm "$tmpgooo".* \
  '&&' 'if' '[[' "$reset" '=' '1' ']];' 'then' $cargo green supergreen builder rm --clean '||' 'exit' '1;' 'fi' \
  '&&' "$(printf '%q ' "${envvars[@]}")" $cargo green -vv install $timings $jobs --root=$tmpbins $frozen --force "$(as_install "$name_at_version")" "$args" \
  '&&' tmux kill-session -t "=$session_name"
tmux select-layout -t "$tmux_t" even-vertical

tmux attach-session -t "$tmux_t"

if [[ "$final" = '1' ]]; then
  cat recipes/$name_at_version.Dockerfile | sed "s%/home/$USER/%/home/runner/%g" >recipes/$name_at_version.Dockerfile-
  mv recipes/$name_at_version.Dockerfile- recipes/$name_at_version.Dockerfile
fi

echo "$name_at_version"
echo "Target dir: $tmptrgt"
echo "Logs: $tmplogs"

for arg in "$@"; do
  "$0" "$arg"
done
