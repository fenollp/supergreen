#!/usr/bin/env -S bash -eu
set -o pipefail
set -x

# Usage: $0  <cargo args...>

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

REBUILD=${REBUILD:-0}            # Force patch_and_build
REGISTRY=${REGISTRY:-fenollexai} # DockerHub account hosting the patched images
MSG_SIZE_MB=${MSG_SIZE_MB:-128}  # Replace the 16 MiB default (16 -> 128 MiB)

BUILDKIT_REF=$(cat "$repo_root"/cargo-green/latest_buildkit.txt | sed -E 's/([0-9]+[.][0-9]+).+/v\1/')
TAG=patched-$BUILDKIT_REF
BUILDER=$REGISTRY/moby_buildkit:$TAG
FRONTEND=$REGISTRY/docker_dockerfile:$TAG

patch_and_build() {
  local SRC_DIR=$repo_root/target/cargo-green-patched/buildkit-src
  mkdir -p "$SRC_DIR"
  rm -rf "$SRC_DIR"
  git clone --quiet --depth=1 --branch="$BUILDKIT_REF" https://github.com/moby/buildkit.git "$SRC_DIR"

  sed -i -E \
    -e 's%(package grpcclient)%\1\nimport "github.com/containerd/containerd/v2/defaults"%' \
    -e "s/grpc.MaxCallRecvMsgSize\(16 << 20\)/grpc.MaxCallRecvMsgSize(defaults.DefaultMaxRecvMsgSize)/" \
    -e "s/grpc.MaxCallSendMsgSize\(16 << 20\)/grpc.MaxCallSendMsgSize(defaults.DefaultMaxSendMsgSize)/" \
    "$SRC_DIR"/frontend/gateway/grpcclient/client.go

#actually buildx also needs patching
# git clone --depth 1 --branch v0.35.0 https://github.com/docker/buildx
# go build -mod=vendor -trimpath \
#   -ldflags "-X github.com/docker/buildx/version.Version=v0.35.0-msgsize128 -X github.com/docker/buildx/version.Revision=a319e5b15052cf6557ceb666eb8ff6e32380b782 -X github.com/docker/buildx/version.Package=github.com/docker/buildx" \
#   -o bin/docker-buildx ./cmd/buildx
# cp to ~/.docker/cli-plugins/

  sed -i -E \
    -e "s/DefaultMaxRecvMsgSize = 16 << 20/DefaultMaxRecvMsgSize = $MSG_SIZE_MB << 20/" \
    -e "s/DefaultMaxSendMsgSize = 16 << 20/DefaultMaxSendMsgSize = $MSG_SIZE_MB << 20/" \
    "$SRC_DIR"/vendor/github.com/containerd/containerd/v2/defaults/defaults.go

  docker buildx build "$SRC_DIR" --target buildkit --push --load --tag "$BUILDER"

  docker buildx build "$SRC_DIR" --push --load --tag "$FRONTEND" \
    --file "$SRC_DIR/frontend/dockerfile/cmd/dockerfile-frontend/Dockerfile"
}

if [ $REBUILD = 1 ]; then
  patch_and_build
fi

export CARGOGREEN_BUILDER_IMAGE="docker-image://$(docker inspect -f '{{range .RepoDigests}}{{.}}{{end}}' "$BUILDER")"
export CARGOGREEN_SYNTAX_IMAGE="docker-image://$(docker inspect -f '{{range .RepoDigests}}{{.}}{{end}}' "$FRONTEND")"

if [ $REBUILD = 1 ]; then
  cargo green supergreen builder recreate
fi

# zed's proto crate uses prost-build 0.9 which execs its own bundled protoc binary
# from within its crate sources => mount build scripts' deps' sources.
export CARGOGREEN_EXPERIMENT=buildscriptsources
export CARGOGREEN_ADD_APT='build-essential,clang,cmake,curl,elfutils,g++,gcc,gettext-base,git,jq,libasound2-dev,libfontconfig-dev,libgit2-dev,libglib2.0-dev,libsqlite3-dev,libssl-dev(>=3.5),libva-dev,libvulkan1,libwayland-dev,libx11-xcb-dev,libxkbcommon-x11-dev,libzstd-dev,lld,llvm,make,musl-dev,musl-tools,pipewire,protobuf-compiler,xdg-desktop-portal'
export CARGO_TARGET_DIR=/tmp/zed
# ./hack/cargo.sh  green install zed --git https://github.com/zed-industries/zed.git --tag=v1.0.0 --jobs=1

# export CARGOGREEN_ADD_APT='make'
# export CARGO_TARGET_DIR=/tmp/uv
# # ./hack/cargo.sh  green +1.91 install --locked uv --git https://github.com/astral-sh/uv.git --rev=2748dce --jobs=1

cargo green supergreen env

exec cargo "$@"
