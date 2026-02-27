#!/usr/bin/env -S bash -eux
set -o pipefail

export CARGO_TARGET_DIR=$(realpath "$(dirname "$(dirname "$0")")")/target
cargo install --locked --force --path cargo-green/

install_package=buildxargs@1.4.0
install_root=$(mktemp -d)

export CARGOGREEN_FINAL_PATH=./recipes/$install_package.Dockerfile
export CARGOGREEN_SYNTAX_IMAGE=docker-image://docker.io/docker/dockerfile:1@sha256:38387523653efa0039f8e1c89bb74a30504e76ee9f565e25c9a09841f9427b05
export CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/rust:1.84.1-slim@sha256:69fbd6ab81b514580bc14f35323fecb09feba9e74c5944ece9a70d9a2a369df0
CARGO='cargo +1.84.1'
export CARGOGREEN_LOG=trace
export CARGOGREEN_LOG_PATH=/tmp/cargo-green--hack-caching--$install_package.log
export CARGO_TARGET_DIR=/tmp/cargo-green--hack-caching--target-dir
mkdir -p $CARGO_TARGET_DIR
rm -rf $CARGO_TARGET_DIR/* $CARGOGREEN_LOG_PATH* >/dev/null

$CARGO green supergreen env

compute_installed_bin_sha256() {
	sha256sum $install_root/bin/${install_package%@*} | awk '{print $1}'
}

ensure__rewrite_cratesio_index__works() {
	! grep -F '/index.crates.io-' $CARGOGREEN_LOG_PATH | grep -vE '/index.crates.io-0{16}|original args|env is set|opening .RO. crate tarball|picked'
# ! grep -Erl --exclude='*.Dockerfile' --exclude='*.toml' --exclude='externs_*' '/index.crates.io-0{16}/' $CARGO_TARGET_DIR
}
ensure__rewrite_target_dir__works() {
	! grep -F "$CARGO_TARGET_DIR" $CARGOGREEN_FINAL_PATH
}

compute_produced_shas() {
	grep -E produced.+0x $CARGOGREEN_LOG_PATH | awk '{print $8,$9}' | sort
}
ensure__produces_same_shas() {
	if [[ ! -f $CARGOGREEN_LOG_PATH.produced ]]; then
		compute_produced_shas >$CARGOGREEN_LOG_PATH.produced
	else
		diff --width=150 -y <(cat $CARGOGREEN_LOG_PATH.produced) <(compute_produced_shas)
	fi
}

registry_blobs() {
    local dir=$1; shift
    [[ $# -eq 0 ]]
    find $dir/docker/registry/v2/blobs/sha256/??/ -type d | awk -F/ '{print $NF}' | sort -u
}

echo Sortons nos cartes!
echo


#---


rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked                            $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # => just computes shas
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
install_sha=$(compute_installed_bin_sha256)

grep -A2 '# Pipe this file to:' $CARGOGREEN_FINAL_PATH
echo Builds fine
echo


#---


rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # rebuild => same shas
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # rebuild => no change

echo Re-builds fine
echo


#---


reg1=$(mktemp -d)
reg2=$(mktemp -d)
registry_proxy=mirror.gcr.io # dockerhub gets annoying
docker run --rm -it --name regis3-1 -d --user $(id -u):$(id -g) -p 12345:5000 -v $reg1:/var/lib/registry $registry_proxy/registry:3
docker run --rm -it --name regis3-2 -d --user $(id -u):$(id -g) -p 23456:5000 -v $reg2:/var/lib/registry $registry_proxy/registry:3
export CARGOGREEN_CACHE_IMAGES=docker-image://localhost:12345/ca/ching,docker-image://localhost:23456/ca/ching
$CARGO green supergreen builder recreate

rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # rebuild => same shas
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # rebuild => no change

echo Re-re-builds fine and both remote registries are equal
echo

docker stop --timeout 2 regis3-2
diff --width=150 -y <(registry_blobs $reg1) <(registry_blobs $reg2)

unset CARGOGREEN_CACHE_IMAGES
export CARGOGREEN_CACHE_TO_IMAGES=docker-image://localhost:12345/ca/ching
export CARGOGREEN_EXPERIMENT=repro

rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # rebuild without reading cache => new layers written to cache!!
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # rebuild => no change

docker stop --timeout 2 regis3-1
unset CARGOGREEN_EXPERIMENT
unset CARGOGREEN_CACHE_TO_IMAGES
[[ $(registry_blobs $reg1 | wc -l) -gt $(registry_blobs $reg2 | wc -l) ]]
[[ $( ( diff --width=150 -y <(registry_blobs $reg1) <(registry_blobs $reg2) || true ) | wc -l ) = $(registry_blobs $reg1 | wc -l) ]]
[[ $( ( diff --width=150 -y <(registry_blobs $reg1) <(registry_blobs $reg2) || true ) | grep '<' | wc -l ) -ge $(registry_blobs $reg2 | wc -l) ]]
[[ $( ( diff --width=150 -y <(registry_blobs $reg1) <(registry_blobs $reg2) || true ) | grep '|' | wc -l ) = 0 ]]
[[ $( ( diff --width=150 -y <(registry_blobs $reg1) <(registry_blobs $reg2) || true ) | grep '>' | wc -l ) = 0 ]]
rm -rf $reg1 $reg2 >/dev/null

echo Re-re-re-builds fine but remote registry cache keeps growing '(albeit slowly)'...
echo "TODO: https://github.com/moby/buildkit/issues/6348 about 'remote cache not being static'"
echo


#---


case "${BUILDX_BUILDER:-}" in
  '') export BUILDX_BUILDER=supergreen ;;
  'empty') export BUILDX_BUILDER= ;;
  *) ;;
esac

rm $CARGO_TARGET_DIR/release/deps/${install_package%@*}-????????????????
invocation=$(grep -vE '^## ' $CARGOGREEN_FINAL_PATH | grep -E '^# ' | tail -n1 | cut -c2- | head -n1 | cut -d'<' -f1 | sed "s%--output=.%-o=$CARGO_TARGET_DIR/release/deps/%")
$invocation --call=format=json,check   <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation --call=format=json,outline <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation --call=format=json,targets <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation                            <$CARGOGREEN_FINAL_PATH
$CARGO_TARGET_DIR/release/deps/${install_package%@*} --help >/dev/null
[[ $install_sha = $(sha256sum $CARGO_TARGET_DIR/release/deps/${install_package%@*} | awk '{print $1}') ]] # rebuild => no change

unset BUILDX_BUILDER

echo Builds fine and in a standalone way
echo


#---


export CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/rust:1.84.0-slim@sha256:0ec205a9abb049604cb085f2fdf7630f1a31dad1f7ad4986154a56501fb7ca77
CARGO='cargo +1.84.0'

rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
REPO=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="cargo-green").repository')
VSN=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name=="cargo-green").version')
git --no-pager diff --ignore-matching-lines='^##' -- $CARGOGREEN_FINAL_PATH
echo 'Change rustc => changes base image (at least)'
cat <<EOF | diff -u - <(git --no-pager diff --ignore-matching-lines='^##' -- $CARGOGREEN_FINAL_PATH | head -n11 | tail -n+6)
 # check=error=true
 # Generated by $REPO v$VSN
 
-FROM --platform=\$BUILDPLATFORM docker.io/library/rust:1.84.1-slim@sha256:69fbd6ab81b514580bc14f35323fecb09feba9e74c5944ece9a70d9a2a369df0 AS rust-base
+FROM --platform=\$BUILDPLATFORM docker.io/library/rust:1.84.0-slim@sha256:0ec205a9abb049604cb085f2fdf7630f1a31dad1f7ad4986154a56501fb7ca77 AS rust-base
 ARG SOURCE_DATE_EPOCH=42
EOF
git add $CARGOGREEN_FINAL_PATH
echo 'Change rustc => changes shas' && ! ensure__produces_same_shas
rm -rf $CARGOGREEN_LOG_PATH.produced >/dev/null
ensure__produces_same_shas # (here, we just re-compute shas)
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
echo 'Change rustc => change final bin' && [[ $install_sha != $(compute_installed_bin_sha256) ]]
install_sha=$(compute_installed_bin_sha256)

# https://github.com/rust-lang/cargo/issues/10367#issuecomment-1053678306
# > This is currently intentional behavior. There are situations where RUSTC changes, but we don't want that to trigger a full recompile. If one rustc emits the same version output as another, then cargo assumes they essentially behave the same, even if they are from different paths. I'm not sure this is something that can be changed without causing unwanted recompiles in some situations.
#=> no changes to final path (except for that base image)

echo Changing rustc may change crates metadata
echo


#---


# Adding -vv => s/'--cap-lints' 'allow'/'--cap-lints' 'warn'/g
# TODO: cargo -vv test != cargo test: => the rustc flags will change => Dockerfile needs new cache key
# => otherwise docker builder cache won't have the correct hit
# https://rustc-dev-guide.rust-lang.org/backend/libs-and-metadata.html
#=> a filename suffix with content hash?

# Adding +nightly => changes '-C' 'metadata=' and '-C' 'extra-filename='
# Changing CARGOGREEN_LOG_LEVEL shouldn't evict cache

# rm -rf $CARGO_TARGET_DIR/* >/dev/null
# rm -rf $CARGOGREEN_LOG_PATH >/dev/null
# rm -rf $install_root/* >/dev/null
# $CARGO green +nightly install --locked --frozen --offline --force $install_package --root=$install_root
# git --no-pager diff --color-words=. --exit-code -- $CARGOGREEN_FINAL_PATH


#---


old_target_dir=$CARGO_TARGET_DIR
rm -rf $old_target_dir/* >/dev/null
export CARGO_TARGET_DIR=/tmp/cargo-green--hack-caching
mkdir -p $CARGO_TARGET_DIR

rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git --no-pager diff --ignore-matching-lines='^##' -- $CARGOGREEN_FINAL_PATH
echo 'Change CARGO_TARGET_DIR => no diff!'
cat <<'EOF' | diff -u - <(git --no-pager diff -- $CARGOGREEN_FINAL_PATH | tail -n+7)
EOF
unset old_target_dir
! ensure__produces_same_shas # change targetdir => changes shas (here, we just re-compute shas)
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # change targetdir => no binary changes
git --no-pager diff --ignore-matching-lines='^##' -- $CARGOGREEN_FINAL_PATH
git add $CARGOGREEN_FINAL_PATH

echo Changing CARGO_TARGET_DIR only changes runner call!
echo
