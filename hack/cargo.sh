#!/usr/bin/env -S bash -eu
set -o pipefail
set -x

# Usage:
#   ./cargo-green.sh <cargo-green args...>
#
# Knobs (env vars):
#   REBUILD=1                    force re-clone / re-patch / re-build of BuildKit
#   REGISTRY=fenollexai          Docker Hub account hosting the patched images
#   MSG_SIZE_MB=128              replace the 16 MiB default (16 -> 128 MiB)

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

REBUILD=${REBUILD:-0}
REGISTRY=${REGISTRY:-fenollexai}
MSG_SIZE_MB=${MSG_SIZE_MB:-128}
BUILDKIT_REF=$(cat "$repo_root"/cargo-green/latest_buildkit.txt | sed -E 's/([0-9]+[.][0-9]+).+/v\1/')

BUILDER_REPO=$REGISTRY/moby_buildkit
FRONTEND_REPO=$REGISTRY/docker_dockerfile
TAG=patched-$BUILDKIT_REF

patch_and_build() {
  local SRC_DIR=$repo_root/target/cargo-green-patched/buildkit-src
  mkdir -p "$SRC_DIR"
  rm -rf "$SRC_DIR"
  git clone --quiet --depth=1 --branch "$BUILDKIT_REF" \
    https://github.com/moby/buildkit.git "$SRC_DIR"

  sed -i -E \
    -e "s/DefaultMaxRecvMsgSize = 16 << 20/DefaultMaxRecvMsgSize = $MSG_SIZE_MB << 20/" \
    -e "s/DefaultMaxSendMsgSize = 16 << 20/DefaultMaxSendMsgSize = $MSG_SIZE_MB << 20/" \
    "$SRC_DIR"/vendor/github.com/containerd/containerd/v2/defaults/defaults.go

  sed -i -E \
    -e 's%(package grpcclient)%\1\nimport "github.com/containerd/containerd/v2/defaults"%' \
    -e "s/grpc.MaxCallRecvMsgSize\(16 << 20\)/grpc.MaxCallRecvMsgSize(defaults.DefaultMaxRecvMsgSize)/" \
    -e "s/grpc.MaxCallSendMsgSize\(16 << 20\)/grpc.MaxCallSendMsgSize(defaults.DefaultMaxSendMsgSize)/" \
    "$SRC_DIR"/frontend/gateway/grpcclient/client.go

  docker buildx build "$SRC_DIR" --target buildkit --push --load --tag "$BUILDER_REPO:$TAG"

  docker buildx build "$SRC_DIR" \
    --file "$SRC_DIR/frontend/dockerfile/cmd/dockerfile-frontend/Dockerfile" \
    --push --load --tag "$FRONTEND_REPO:$TAG"
}

repo_digest() {
  local tag=$1 repo=$2 d
  d=$(docker inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$tag" | grep "^${repo}@" | head -n1)
  printf '%s' "$d"
}

if [ $REBUILD = 1 ]; then
    patch_and_build
fi

export CARGOGREEN_BUILDER_IMAGE="docker-image://$(repo_digest "$BUILDER_REPO:$TAG"  "$BUILDER_REPO")"
export CARGOGREEN_SYNTAX_IMAGE="docker-image://$(repo_digest "$FRONTEND_REPO:$TAG" "$FRONTEND_REPO")"

if [ $REBUILD = 1 ]; then
  cargo green supergreen builder recreate
fi

# export CARGOGREEN_ADD_APT='build-essential,clang,cmake,curl,elfutils,g++,gcc,gettext-base,git,jq,libasound2-dev,libfontconfig-dev,libgit2-dev,libglib2.0-dev,libsqlite3-dev,libssl-dev(>=3.5),libva-dev,libvulkan1,libwayland-dev,libx11-xcb-dev,libxkbcommon-x11-dev,libzstd-dev,lld,llvm,make,musl-dev,musl-tools,pipewire,xdg-desktop-portal'
# export CARGO_TARGET_DIR=/tmp/zed
# exec cargo green install --locked zed --git https://github.com/zed-industries/zed.git --tag=v1.0.0 --jobs=1


export CARGOGREEN_ADD_APT='make'
export CARGO_TARGET_DIR=/tmp/uv
# exec cargo green +1.91 install --locked uv --git https://github.com/astral-sh/uv.git --rev=2748dce --jobs=1
# exec cargo green +1.91 install --locked uv --git https://github.com/astral-sh/uv.git --rev=2748dce

exec cargo "$@"
