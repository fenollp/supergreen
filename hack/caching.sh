#!/usr/bin/env -S bash -eu
set -o pipefail

export CARGO_TARGET_DIR=$(realpath "$(dirname "$(dirname "$0")")")/target
cargo install --locked --force --path cargo-green/

install_package=buildxargs@1.4.0
install_root=$(mktemp -d)

export CARGOGREEN_FINAL_PATH=./recipes/$install_package.Dockerfile
export CARGOGREEN_SYNTAX_IMAGE=docker-image://docker.io/docker/dockerfile:1@sha256:2780b5c3bab67f1f76c781860de469442999ed1a0d7992a5efdf2cffc0e3d769
export CARGOGREEN_BASE_IMAGE=docker-image://docker.io/library/debian:trixie-slim@sha256:cedb1ef40439206b673ee8b33a46a03a0c9fa90bf3732f54704f99cb061d2c5a
CARGO='cargo +1.85.1'
export CARGOGREEN_LOG=trace
export CARGOGREEN_LOG_PATH=/tmp/cargo-green--hack-caching--$install_package.log
export CARGO_TARGET_DIR=/tmp/cargo-green--hack-caching--target-dir
XDG_CACHE_HOME=$(mktemp -d) # Where cargo-green keeps results: fresh, and only ours
export XDG_CACHE_HOME
results=$XDG_CACHE_HOME/cargo-green/results
mkdir -p $results $CARGO_TARGET_DIR
rm -rf $CARGO_TARGET_DIR/* $CARGOGREEN_LOG_PATH* >/dev/null

if [[ ! -L /usr/local/cargo ]]; then
	{ $CARGO green supergreen setup 2>/dev/null || true; } | sudo /bin/sh -xe
fi

# A builder of our own: this empties its build cache, and reads its data dir
caching_builder=supergreen-caching
docker buildx rm --force --builder $caching_builder >/dev/null 2>&1 || true # Left by an interrupted run
export BUILDX_BUILDER=$caching_builder
$CARGO green supergreen builder create
trap 'docker buildx rm --force --builder $caching_builder || true' EXIT

$CARGO green supergreen env

compute_installed_bin_sha256() {
	sha256sum $install_root/bin/${install_package%@*} | awk '{print $1}'
}

ensure__rewrite_cratesio_index__works() {
	! grep -F '/index.crates.io-' $CARGOGREEN_LOG_PATH | grep -vE '/index.crates.io-[0-9]{16}|original args|env is set|opening .RO. crate tarball|picked'
	! grep -Erl --exclude='*.Dockerfile' --exclude='*.toml' --exclude='externs_*' '/index.crates.io-[0-9]{16}/' $CARGO_TARGET_DIR
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
		[[ -s $CARGOGREEN_LOG_PATH.produced ]] # ensure file not empty
	else
		diff --width=150 -y <(cat $CARGOGREEN_LOG_PATH.produced) <(compute_produced_shas)
	fi
}

compute_results_shas() { # <entries|whole> => per result: each of its entries' header and contents' sha256 | the sha256 of its tarball and of it (compressed)
	local what=$1; shift
	[[ $# -eq 0 ]]
	local f d
	for f in "$results"/*; do
		[[ -f "$f" ]] || continue
		case "$what" in
		entries)
			zstd -dc "$f" | tar -tv --full-time --numeric-owner | sed "s|^|${f##*/} |"
			d=$(mktemp -d)
			zstd -dc "$f" | tar -x -C "$d"
			(cd "$d" && find . -type f -exec sha256sum {} + | sort -k2 | cut -c-16,65-) | sed "s|^|${f##*/} |"
			chmod -R u+w "$d" && rm -rf "$d" ;;
		whole)
			printf '%s tar:%s zst:%s\n' "${f##*/}" \
				"$(zstd -dc "$f" | sha256sum | cut -c-16)" \
				"$(sha256sum <"$f" | cut -c-16)" ;;
		esac
	done
}
ensure__results_same_shas() { # Results hold md.toml, which holds Containerfile parts: one baseline per $CARGOGREEN_EXPERIMENT
	local baseline=$CARGOGREEN_LOG_PATH.results${CARGOGREEN_EXPERIMENT:+.$CARGOGREEN_EXPERIMENT}
	if [[ ! -f $baseline.whole ]]; then
		compute_results_shas entries >$baseline.entries
		compute_results_shas whole >$baseline.whole
		[[ -s $baseline.whole ]] # ensure some results
	else
		# Entry by entry first: says what differs
		diff --width=150 -y <(cat $baseline.entries) <(compute_results_shas entries)
		diff --width=150 -y <(cat $baseline.whole) <(compute_results_shas whole)
	fi
}

registry_blobs() {
	local dir=$1; shift
	[[ $# -eq 0 ]]
	find $dir/docker/registry/v2/blobs/sha256/ -type f | while read -r p; do basename $(dirname $p); done | sort -u
}

echo Sortons nos cartes!
echo


#---


rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf "$results"/* >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked                            $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # => just computes shas
ensure__results_same_shas # => just computes shas
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
rm -rf "$results"/* >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git add $CARGOGREEN_FINAL_PATH
ensure__produces_same_shas # rebuild => same shas
ensure__results_same_shas # rebuild => same shas
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # rebuild => no change

echo Re-builds fine
echo


#---


# BuildKit build cache (exported to registries, and as found in the builder's data dir): is it bit-reproducible?
# Results (above) are: the tar exporter rewrites timestamps to $SOURCE_DATE_EPOCH.
# The build cache isn't by default: cache keys are made of contents, not mtimes, so it needn't be.
# With the bitrepro experiment, our RUNs timestamp what they write. What's left then is BuildKit's:
# * the cache config: records' createdAt, and the order of records with the same digest
# * dirs that WORKDIR and ADD --unpack create (parents of WORKDIR's, all of ADD's)
# Once BuildKit has them all bit-reproducible (with bitrepro), this script exits with 42: then make these warnings errors.

registry_proxy=mirror.gcr.io # dockerhub gets annoying
start_registry() { # <port> => a fresh registry, serving localhost:<port>
	local port=$1; shift
	[[ $# -eq 0 ]]
	local dir; dir=$(mktemp -d)
	docker rm -f "regis3-$port" >/dev/null 2>&1 || true
	docker run --rm -d --name "regis3-$port" --user "$(id -u):$(id -g)" -p "$port:5000" -v "$dir:/var/lib/registry" "$registry_proxy/registry:3" >/dev/null
	echo "$dir"
}
reproducible=() # One row per comparison below
bitrepro_rows=0 bitrepro_identical=0
warn_unless_same() { # <what> <file> <file>
	local what=$1 a=$2 b=$3; shift 3
	[[ $# -eq 0 ]]
	local same differ verdict=differ
	same=$(comm -12 <(sort "$a") <(sort "$b") | wc -l)
	differ=$(comm -23 <(sort "$a") <(sort "$b") | wc -l)
	if [[ -s "$a" ]] && diff -q "$a" "$b" >/dev/null; then
		echo "OK: $what (bitrepro=$bitrepro)"
		verdict=identical
	else
		echo "WARNING: $what (bitrepro=$bitrepro): differ (or are empty)"
		diff --width=150 -y "$a" "$b" | head -n 20 || true
	fi
	if [[ $bitrepro = 1 ]]; then
		bitrepro_rows=$((bitrepro_rows + 1))
		[[ $verdict = differ ]] || bitrepro_identical=$((bitrepro_identical + 1))
	fi
	reproducible+=("$(printf '%-70s %8s %6s same %6s differ  %s' "$what" "$bitrepro" "$same" "$differ" "$verdict")")
}
builder_snapshots() { # => mtime and sha256 of every file in the builder's snapshots (minus their ids and paths)
	local data; data=$($CARGO green supergreen sync data)
	if [[ -z "$data" ]]; then echo "WARNING: builder has no data dir (not a docker-container driver?)" >&2; return 0; fi
	# Read through a container: no need for sudo
	docker run --rm -v "$data:/data:ro" "$registry_proxy/library/busybox:1" \
		find /data/runc-overlayfs/snapshots/snapshots -type f -exec sh -c 'for f; do echo "$(stat -c %Y "$f") $(sha256sum <"$f" | cut -d" " -f1)"; done' _ {} + | sort || true
}
build_writing_cache_to() { # <cache image>...
	local to; to=$(IFS=,; echo "$*")
	CARGOGREEN_CACHE_TO_IMAGES=$to $CARGO green supergreen builder recreate --clean # Empty build cache, new builder knows these registries
	rm -rf "${CARGO_TARGET_DIR:?}"/* "$CARGOGREEN_LOG_PATH" "$results"/* "${install_root:?}"/* >/dev/null
	# Write only: reading would just copy blobs over
	CARGOGREEN_CACHE_TO_IMAGES=$to $CARGO green install --locked --frozen --offline --force "$install_package" --root="$install_root"
	git add "$CARGOGREEN_FINAL_PATH"
	ensure__produces_same_shas # rebuild => same shas
	ensure__results_same_shas # rebuild => same shas
	ensure__rewrite_cratesio_index__works
	ensure__rewrite_target_dir__works
	"$install_root/bin/${install_package%@*}" --help >/dev/null
	[[ $install_sha = $(compute_installed_bin_sha256) ]] # rebuild => no change
}

for bitrepro in 0 1; do # Without, then with, the bitrepro experiment
	if [[ $bitrepro = 1 ]]; then export CARGOGREEN_EXPERIMENT=bitrepro; else unset CARGOGREEN_EXPERIMENT; fi

	# 1. One build, writing its cache to two registries at once: same blobs?
	reg1=$(start_registry $((12345 + bitrepro)))
	reg2=$(start_registry $((23456 + bitrepro)))
	build_writing_cache_to docker-image://localhost:$((12345 + bitrepro))/ca/ching docker-image://localhost:$((23456 + bitrepro))/ca/ching
	registry_blobs "$reg1" >$CARGOGREEN_LOG_PATH.$bitrepro.reg1
	registry_blobs "$reg2" >$CARGOGREEN_LOG_PATH.$bitrepro.reg2
	warn_unless_same 'one build, written to two registries: blobs' $CARGOGREEN_LOG_PATH.$bitrepro.reg1 $CARGOGREEN_LOG_PATH.$bitrepro.reg2
	builder_snapshots >$CARGOGREEN_LOG_PATH.$bitrepro.snapshots1

	# 2. The same build, from an empty build cache again: same blobs?
	reg3=$(start_registry $((34567 + bitrepro)))
	build_writing_cache_to docker-image://localhost:$((34567 + bitrepro))/ca/ching
	registry_blobs "$reg3" >$CARGOGREEN_LOG_PATH.$bitrepro.reg3
	warn_unless_same 'two builds, from an empty build cache each: blobs' $CARGOGREEN_LOG_PATH.$bitrepro.reg1 $CARGOGREEN_LOG_PATH.$bitrepro.reg3
	builder_snapshots >$CARGOGREEN_LOG_PATH.$bitrepro.snapshots2
	warn_unless_same "two builds, from an empty build cache each: builder's snapshots" $CARGOGREEN_LOG_PATH.$bitrepro.snapshots1 $CARGOGREEN_LOG_PATH.$bitrepro.snapshots2

	for port in $((12345 + bitrepro)) $((23456 + bitrepro)) $((34567 + bitrepro)); do docker stop --timeout 2 "regis3-$port" >/dev/null; done
	rm -rf "$reg1" "$reg2" "$reg3" >/dev/null
	unset reg1 reg2 reg3
done
unset CARGOGREEN_EXPERIMENT bitrepro
rm -rf "${CARGO_TARGET_DIR:?}"/* "$CARGOGREEN_LOG_PATH" "$results"/* "${install_root:?}"/* >/dev/null

echo Re-builds fine, writing build cache to registries
echo


#---


invocation=$(grep -vE '^## ' $CARGOGREEN_FINAL_PATH | grep -E '^# ' | tail -n1 | cut -c2- | head -n1 | cut -d'<' -f1 | sed "s%--output=.%-o=$CARGO_TARGET_DIR/release/deps/%")
$invocation --call=format=json,check   <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation --call=format=json,outline <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation --call=format=json,targets <$CARGOGREEN_FINAL_PATH | jq 'del(.sources[0])'
$invocation                            <$CARGOGREEN_FINAL_PATH
$CARGO_TARGET_DIR/release/deps/${install_package%@*} --help >/dev/null
[[ $install_sha = $(sha256sum $CARGO_TARGET_DIR/release/deps/${install_package%@*} | awk '{print $1}') ]] # rebuild => no change

echo Builds fine and in a standalone way
echo


#---


CARGO='cargo +1.85.0'

rm -rf $CARGO_TARGET_DIR/* >/dev/null
rm -rf $CARGOGREEN_LOG_PATH >/dev/null
rm -rf "$results"/* >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git --no-pager diff -- $CARGOGREEN_FINAL_PATH
echo 'Change rustc => changes base image (at least)'
cat <<EOF | diff -u - <(git --no-pager diff -- $CARGOGREEN_FINAL_PATH | head -n12 | tail -n+9)
-FROM scratch AS rustup-1.85.1-x86_64-unknown-linux-gnu
+FROM scratch AS rustup-1.85.0-x86_64-unknown-linux-gnu
 ADD --chmod=u+x --checksum=sha256:4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10 \\
   https://static.rust-lang.org/rustup/archive/1.29.0/x86_64-unknown-linux-gnu/rustup-init /rustup-init
EOF
git add $CARGOGREEN_FINAL_PATH
echo 'Change rustc => changes shas' && ! ensure__produces_same_shas
rm -f $CARGOGREEN_LOG_PATH.produced >/dev/null
ensure__produces_same_shas # (here, we just re-compute shas)
echo 'Change rustc => changes shas' && ! ensure__results_same_shas
rm -f $CARGOGREEN_LOG_PATH.results.entries $CARGOGREEN_LOG_PATH.results.whole >/dev/null
ensure__results_same_shas # (here, we just re-compute shas)
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
rm -rf "$results"/* >/dev/null
rm -rf $install_root/* >/dev/null
$CARGO green install --locked --frozen --offline --force $install_package --root=$install_root
git --no-pager diff -- $CARGOGREEN_FINAL_PATH
echo 'Change CARGO_TARGET_DIR => no diff!'
cat <<'EOF' | diff -u - <(git --no-pager diff -- $CARGOGREEN_FINAL_PATH | tail -n+7)
EOF
unset old_target_dir
! ensure__produces_same_shas # change targetdir => changes shas (here, we just re-compute shas)
ensure__produces_same_shas # change targetdir => same results
ensure__rewrite_cratesio_index__works
ensure__rewrite_target_dir__works
$install_root/bin/${install_package%@*} --help >/dev/null
[[ $install_sha = $(compute_installed_bin_sha256) ]] # change targetdir => no binary changes
git --no-pager diff -- $CARGOGREEN_FINAL_PATH
git add $CARGOGREEN_FINAL_PATH

echo Changing CARGO_TARGET_DIR only changes runner call!
echo


#---


echo "BuildKit build cache bit-reproducibility:"
printf '  %-70s %8s\n' 'comparison' 'bitrepro'
printf '  %s\n' "${reproducible[@]}"
if [[ $bitrepro_rows -gt 0 ]] && [[ $bitrepro_identical -eq $bitrepro_rows ]]; then
	echo "All identical with bitrepro: BuildKit's build cache is bit-reproducible!"
	exit 42
fi
