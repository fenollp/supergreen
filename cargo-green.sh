#!/usr/bin/env bash
# cargo-green.sh — run `cargo green` against a locally-built, *patched* BuildKit whose
# gRPC message-size limit is raised from 16 MiB to 64 MiB. This fixes:
#
#   Error: Runner failed to build: failed to solve: ResourceExhausted:
#     trying to send message larger than max (19219065 vs. 16777216)
#
# The 16 MiB cap (16777216) comes from containerd's `DefaultMaxRecvMsgSize` /
# `DefaultMaxSendMsgSize`, referenced at every BuildKit gRPC call site (buildkitd
# server, client, session, dockerfile-frontend gateway). This script:
#
#   1. clones BuildKit (tag from cargo-green/latest_buildkit.txt),
#   2. bumps those `defaults.DefaultMax{Recv,Send}MsgSize` references x4 -> 64 MiB,
#   3. builds the patched builder (moby/buildkit) and frontend (docker/dockerfile)
#      images and pushes them to a local registry,
#   4. builds the patched cargo-green (which learned to accept both images), then
#   5. runs `cargo green "$@"` with:
#        CARGOGREEN_BUILDER_IMAGE -> patched moby/buildkit   (buildkitd)
#        CARGOGREEN_SYNTAX_IMAGE  -> patched docker/dockerfile (frontend)
#
# Steps 1-3 are cached: subsequent runs reuse the pushed images.
#
# Usage:
#   ./cargo-green.sh <cargo-green args...>          e.g. ./cargo-green.sh build --release
#
# Knobs (env vars):
#   CARGOGREEN_PATCH_REBUILD=1   force re-clone / re-patch / re-build of BuildKit
#   BUILDKIT_REF=v0.31.0         BuildKit git tag to patch (default: latest_buildkit.txt)
#   REGISTRY=localhost:5000      local registry hosting the patched images
#   MSG_SIZE_MULT=4              multiplier on the 16 MiB default (4 -> 64 MiB)
#   DOCKER=docker                container runner used to build/push/serve images
#
# Note: assumes a *local* Docker daemon. A remote $DOCKER_HOST cannot reach the
# localhost registry that serves the frontend image.
set -euo pipefail

REPO_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

DOCKER=${DOCKER:-docker}
REGISTRY=${REGISTRY:-localhost:5000}
MSG_SIZE_MULT=${MSG_SIZE_MULT:-4}
BUILDKIT_REF=${BUILDKIT_REF:-v$(tr -d '[:space:]' < "$REPO_DIR/cargo-green/latest_buildkit.txt")}

BUILDER_REPO=$REGISTRY/moby/buildkit
FRONTEND_REPO=$REGISTRY/docker/dockerfile
TAG=patched-$BUILDKIT_REF

STATE_DIR=$REPO_DIR/target/cargo-green-patched
SRC_DIR=$STATE_DIR/buildkit-src
mkdir -p "$STATE_DIR"

log() { printf '\033[1;32m[cargo-green.sh]\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31m[cargo-green.sh] ERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v "$DOCKER" >/dev/null || die "'$DOCKER' not found in PATH"
if [ -n "${DOCKER_HOST:-}${DOCKER_CONTEXT:-}" ]; then
  log "warning: \$DOCKER_HOST/\$DOCKER_CONTEXT is set; the remote daemon must be able to reach ${REGISTRY}."
fi

# echo the "<repo>@sha256:..." pushed to $REGISTRY for a local tag
repo_digest() {
  local tag=$1 repo=$2 d
  d=$($DOCKER inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "$tag" \
        | grep "^${repo}@" | head -n1 || true)
  [ -n "$d" ] || die "no pushed digest found for $tag under $repo (push failed?)"
  printf '%s' "$d"
}

# --- 1. Local registry (persists patched images across runs) -----------------
ensure_registry() {
  local name=cargo-green-registry
  if [ -n "$($DOCKER ps -q -f "name=^${name}$")" ]; then return; fi
  if [ -n "$($DOCKER ps -aq -f "name=^${name}$")" ]; then
    $DOCKER start "$name" >/dev/null
  else
    log "starting local registry '$name' at $REGISTRY"
    $DOCKER run -d --restart=always --name "$name" \
      -p "${REGISTRY##*:}:5000" -v cargo-green-registry-data:/var/lib/registry \
      registry:2 >/dev/null
  fi
}

# --- 2. Clone + patch + build + push the patched BuildKit images --------------
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

  # Local-dir context: the patched .go files are used, and .git (HEAD @ $BUILDKIT_REF,
  # not .dockerignore'd) lets BuildKit's version stage embed the right version so
  # cargo-green does not keep re-creating the builder.
  log "building patched builder image -> $BUILDER_REPO:$TAG"
  $DOCKER buildx build "$SRC_DIR" --target buildkit --load --tag "$BUILDER_REPO:$TAG"
  $DOCKER push "$BUILDER_REPO:$TAG" >/dev/null

  log "building patched frontend image -> $FRONTEND_REPO:$TAG"
  $DOCKER buildx build "$SRC_DIR" \
    --file "$SRC_DIR/frontend/dockerfile/cmd/dockerfile-frontend/Dockerfile" \
    --load --tag "$FRONTEND_REPO:$TAG"
  $DOCKER push "$FRONTEND_REPO:$TAG" >/dev/null

  repo_digest "$BUILDER_REPO:$TAG"  "$BUILDER_REPO"  > "$STATE_DIR/builder.digest"
  repo_digest "$FRONTEND_REPO:$TAG" "$FRONTEND_REPO" > "$STATE_DIR/frontend.digest"
  log "cached digests in $STATE_DIR"
}

ensure_registry

if [ "${CARGOGREEN_PATCH_REBUILD:-0}" = 1 ] \
   || [ ! -s "$STATE_DIR/builder.digest" ] \
   || [ ! -s "$STATE_DIR/frontend.digest" ]; then
  build_patched_buildkit
else
  log "reusing cached patched images (set CARGOGREEN_PATCH_REBUILD=1 to rebuild)"
fi

BUILDER_DIGEST=$(cat "$STATE_DIR/builder.digest")
FRONTEND_DIGEST=$(cat "$STATE_DIR/frontend.digest")

# --- 3. Build the patched cargo-green (image-override support) ----------------
log "building patched cargo-green from $REPO_DIR"
RUSTC_WRAPPER= cargo build --release --manifest-path "$REPO_DIR/Cargo.toml" -p cargo-green
BIN_DIR=${CARGO_TARGET_DIR:-$REPO_DIR/target}/release
[ -x "$BIN_DIR/cargo-green" ] || die "cargo-green binary not found at $BIN_DIR/cargo-green"

# --- 4. Run `cargo green` with the patched images ----------------------------
export CARGOGREEN_BUILDER_IMAGE="docker-image://$BUILDER_DIGEST"
export CARGOGREEN_SYNTAX_IMAGE="docker-image://$FRONTEND_DIGEST"
unset BUILDX_BUILDER   # force cargo-green's managed 'supergreen' builder (= patched buildkitd)

log "CARGOGREEN_BUILDER_IMAGE=$CARGOGREEN_BUILDER_IMAGE"
log "CARGOGREEN_SYNTAX_IMAGE=$CARGOGREEN_SYNTAX_IMAGE"

export PATH="$BIN_DIR:$PATH"
exec cargo green "$@"
