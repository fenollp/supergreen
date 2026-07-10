#!/usr/bin/env -S bash -eu
set -o pipefail
set -x

# Usage:
#   ./cargo-green.sh <cargo-green args...>
#
# Knobs (env vars):
#   REBUILD=1   force re-clone / re-patch / re-build of BuildKit
#   REGISTRY=localhost:5000      local registry hosting the patched images
#   MSG_SIZE_MULT=4              multiplier on the 16 MiB default (4 -> 64 MiB)
#
# Note: assumes a *local* Docker daemon. A remote $DOCKER_HOST cannot reach the
# localhost registry that serves the frontend image.

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

REGISTRY=fenollexai # ${REGISTRY:-localhost:5000}
MSG_SIZE_MULT=6 # ${MSG_SIZE_MULT:-4}
BUILDKIT_REF=v0.31 # $(tr -d '[:space:]' <"$repo_root"/cargo-green/latest_buildkit.txt)

BUILDER_REPO=$REGISTRY/moby_buildkit
FRONTEND_REPO=$REGISTRY/docker_dockerfile
TAG=patched-$BUILDKIT_REF

STATE_DIR=$repo_root/target/cargo-green-patched
SRC_DIR=$STATE_DIR/buildkit-src
mkdir -p "$STATE_DIR"

log() { printf '\033[1;32m[cargo-green.sh]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[cargo-green.sh] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

build_patched_buildkit() {
  log "cloning BuildKit $BUILDKIT_REF into $SRC_DIR"
  rm -rf "$SRC_DIR"
  git clone --quiet --depth 1 --branch "$BUILDKIT_REF" \
    https://github.com/moby/buildkit.git "$SRC_DIR"

  log "patching gRPC max message size: 16 MiB x ${MSG_SIZE_MULT} = $((16 * MSG_SIZE_MULT)) MiB"
  local files=()
  mapfile -t files < <(grep -rlE '\.DefaultMax(Recv|Send)MsgSize' \
    --include='*.go' "$SRC_DIR" | grep -v '/vendor/' || true)
  [ "${#files[@]}" -gt 0 ] || die "no DefaultMax{Recv,Send}MsgSize references found in BuildKit $BUILDKIT_REF"
  local f
  for f in "${files[@]}"; do
    # Wrap each `<pkg>.DefaultMax{Recv,Send}MsgSize` as `(<pkg>.DefaultMax... * N)`.
    # Keeps the import used (compiles) and multiplies the 16 MiB default by N.
    sed -i -E \
      -e "s/([A-Za-z_][A-Za-z0-9_]*\.DefaultMaxRecvMsgSize)/(\1 * ${MSG_SIZE_MULT})/g" \
      -e "s/([A-Za-z_][A-Za-z0-9_]*\.DefaultMaxSendMsgSize)/(\1 * ${MSG_SIZE_MULT})/g" \
      "$f"
    log "  patched ${f#"$SRC_DIR"/}"
  done
  log "patched message-size call sites:"
  grep -rnE '\.DefaultMax(Recv|Send)MsgSize \* ' --include='*.go' "$SRC_DIR" \
    | grep -v '/vendor/' | sed "s|$SRC_DIR/|    |" >&2 || true

  docker buildx build "$SRC_DIR" --target buildkit --push --load --tag "$BUILDER_REPO:$TAG"

  docker buildx build "$SRC_DIR" \
    --file "$SRC_DIR/frontend/dockerfile/cmd/dockerfile-frontend/Dockerfile" \
    --push --load --tag "$FRONTEND_REPO:$TAG"
}

repo_digest() {
  local tag=$1 repo=$2 d
  d=$(docker inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$tag" \
        | grep "^${repo}@" | head -n1 || true)
  [ -n "$d" ] || die "no pushed digest found for $tag under $repo (push failed?)"
  printf '%s' "$d"
}

if [ "${REBUILD:-0}" = 1 ]; then
  build_patched_buildkit

  export CARGOGREEN_BUILDER_IMAGE="docker-image://$(repo_digest "$BUILDER_REPO:$TAG"  "$BUILDER_REPO")"
  export CARGOGREEN_SYNTAX_IMAGE="docker-image://$(repo_digest "$FRONTEND_REPO:$TAG" "$FRONTEND_REPO")"

  cargo green supergreen builder recreate
else
  export CARGOGREEN_BUILDER_IMAGE="docker-image://$(repo_digest "$BUILDER_REPO:$TAG"  "$BUILDER_REPO")"
  export CARGOGREEN_SYNTAX_IMAGE="docker-image://$(repo_digest "$FRONTEND_REPO:$TAG" "$FRONTEND_REPO")"
fi

# export CARGOGREEN_ADD_APT='build-essential,clang,cmake,curl,elfutils,g++,gcc,gettext-base,git,jq,libasound2-dev,libfontconfig-dev,libgit2-dev,libglib2.0-dev,libsqlite3-dev,libssl-dev(>=3.5),libva-dev,libvulkan1,libwayland-dev,libx11-xcb-dev,libxkbcommon-x11-dev,libzstd-dev,lld,llvm,make,musl-dev,musl-tools,pipewire,xdg-desktop-portal'
# export CARGO_TARGET_DIR=/tmp/zed
# exec cargo green install --locked zed --git https://github.com/zed-industries/zed.git --tag=v1.0.0 --jobs=1


export CARGOGREEN_ADD_APT='make'
export CARGO_TARGET_DIR=/tmp/uv
# exec cargo green +1.91 install --locked uv --git https://github.com/astral-sh/uv.git --rev=2748dce --jobs=1
# exec cargo green +1.91 install --locked uv --git https://github.com/astral-sh/uv.git --rev=2748dce

exec cargo "$@"
