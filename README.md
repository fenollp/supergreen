# [`supergreen`](https://github.com/fenollp/supergreen) ~ Faster Rust builds!

[`cargo-green`](./cargo-green): a cached & remote-ready Rust projects builder.

`cargo-green` is
* a [`cargo`](https://doc.rust-lang.org/cargo/) plugin that sets a `$RUSTC_WRAPPER` then calls `cargo`.
* a [`RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads) that builds Dockerfiles
* by forwarding `rustc` calls to [BuildKit](https://github.com/moby/buildkit) builders


![A rusty crab character named Ferris, featuring a unique hairstyle resembling 'Ruby Road', a vibrant and textured hairdo often seen in flamboyant red](./hack/logo2.png)


- [Installation](#installation)
- [Usage](#usage)
- [How it works](#how-it-works)
- [Remote execution](#remote-execution)
- [Caching](#caching)
- [Configuration](#configuration)
  - [`$CARGOGREEN_LOG_PATH`](#cargogreen_log_path)
  - [`$CARGOGREEN_LOG`](#cargogreen_log)
  - [`$CARGOGREEN_LOG_STYLE`](#cargogreen_log_style)
  - [`$CARGOGREEN_RUNNER`](#cargogreen_runner)
  - [`$BUILDX_BUILDER`](#buildx_builder)
  - [`$CARGOGREEN_BUILDER_IMAGE`](#cargogreen_builder_image)
  - [`$CARGOGREEN_SYNTAX_IMAGE`](#cargogreen_syntax_image)
  - [`$CARGOGREEN_REGISTRY_MIRRORS`](#cargogreen_registry_mirrors)
  - [`$CARGOGREEN_CACHE_IMAGES`](#cargogreen_cache_images)
  - [`$CARGOGREEN_CACHE_FROM_IMAGES`](#cargogreen_from_images)
  - [`$CARGOGREEN_CACHE_TO_IMAGES`](#cargogreen_to_images)
  - [`$CARGOGREEN_FINAL_PATH`](#cargogreen_final_path)
  - [`$CARGOGREEN_BASE_IMAGE`](#cargogreen_base_image)
  - [`$CARGOGREEN_SET_ENVS`](#cargogreen_set_envs)
  - [`$CARGOGREEN_BASE_IMAGE_INLINE`](#cargogreen_base_image_inline)
  - [`$CARGOGREEN_WITH_NETWORK`](#cargogreen_with_network)
  - [`$CARGOGREEN_ADD_APT`](#cargogreen_add_apt)
  - [`$CARGOGREEN_ADD_APT_GET`](#cargogreen_add_apt_get)
  - [`$CARGOGREEN_ADD_APK`](#cargogreen_add_apk)
  - [`$CARGOGREEN_EXPERIMENT`](#cargogreen_experiment)
- [Alternatives](#alternatives)
- [Origins](#origins)
- [Hacking](#hacking)
- [Goals](#goals)
- [Upstream issues & patches](#upstream-issues--patches)
- [En vrac](#en-vrac)

## Installation

```shell
cargo install cargo-green
cargo install --locked --force --git https://github.com/fenollp/supergreen.git cargo-green

# Make sure $CARGO_HOME/bin is in your $PATH
which cargo-green
```

When building locally, ensure either a [`docker`](https://github.com/docker/docker-install) or [`podman`](https://podman.io/docs/installation) *client* is installed.

Minimum requirements:
* `Ubuntu 22.04`
* `buildkit 0.12.0`
* `github.com/docker/buildx v0.11.2`
* `rust 1.73`

## Usage

```shell
# Usage:
  cargo green supergreen env [ENV ...]                           Show used values
  cargo green supergreen doc [ENV ...]                           Documentation of said values
  cargo green fetch                                              Pulls images and crates
  cargo green supergreen sync                                    Pulls everything, for offline usage
  cargo green supergreen push                                    Push cache image (all tags)
  cargo green supergreen builder [ { recreate | rm } --clean ]   Manage local/remote builder
  cargo green supergreen -h | --help
  cargo green supergreen -V | --version
  cargo green ...any cargo subcommand...

# Try:
  cargo clean # Start from a clean slate
  cargo green build
  cargo supergreen env CARGOGREEN_BASE_IMAGE 2>/dev/null
  cargo supergreen help

# Suggestion:
  alias cargo='cargo green'
  # Now try, within your project:
  cargo fetch
  cargo test
```


## How it works

This encapsulates the compilation of Rust programs using Docker's build engine. In other words: this `cargo` plugin registers a `rustc` wrapper that calls to BuildKit to build crates and the execution of their build scripts.

These builds use OCI images pinned to the local Rust installation version, eventually producing a single (huge) Containerfile that can then be shared around and built with eg. `docker build -o=. https://host.my/tiny/binary.Dockerfile`.

Builds reproducibility or hermeticity is guaranteed via:
* Every image is locked by its digest (`@sha256:..`)
* Target directory paths are renamed `/target/..`
* `crates.io` sources paths are renamed to `$CARGO_HOME/registry/src/index.crates.io/..`
  * additionally, this path is created locally.
* `git` dependencies are pinned to their commit hash
* Produced files timestamps' are rewritten to some fixed epoch


## Remote execution

Say you have a bigger machine in your `~/.ssh/config` called `extra-oomph`:

```shell
DOCKER_HOST=ssh://extra-oomph cargo green test
```

This will compile the tests on the remote machine and run then locally.
TODO: also run tests remotely, like `cross` does: <https://github.com/cross-rs/cross/blob/49cd054de9b832dfc11a4895c72b0aef533b5c6a/README.md#supported-targets>

For more knobs to tune, see also:
* [`$BUILDX_BUILDER`](#buildx_builder)
* [`$CARGOGREEN_BUILDER_IMAGE`](#cargogreen_builder_image)


## Caching

Share your build cache with your team and CI, never feel cold starts!

```toml
[package.metadata.green]
cache-images = [ "docker-image://my.org/team/my-project", "docker-image://ghcr.io/me/fork" ]
cache-from-images = [ "docker-image://some.org/global/cache" ]
```


## Configuration

Tune the behavior of `cargo-green` either through environment variables or via the package's `green` metadata section.

```toml
[package]
name = "my-crate"
# ...

[package.metadata.green]
registry-mirrors = [ "mirror.gcr.io", "public.ecr.aws/docker" ] # Default values
```

Environment variables that are prefixed with `$CARGOGREEN_` override TOML settings.

```shell
export CARGOGREEN_REGISTRY_MIRRORS=mirror.gcr.io
cargo green build
```

### `$CARGOGREEN_LOG_PATH`

Path to a text file to write logs.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_LOG_PATH="/tmp/my-logs.txt"
# This needs to be set: nothing is logged by default
export CARGOGREEN_LOG="info"
```

### `$CARGOGREEN_LOG`

Filter logs. Equivalent to `$RUST_LOG` (and doesn't conflict with `cargo`'s).

By default, writes logs under [`/tmp`](https://doc.rust-lang.org/stable/std/env/fn.temp_dir.html).

See <https://docs.rs/env_logger/#enabling-logging>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_LOG="trace,cargo_green::build=info"
```

### `$CARGOGREEN_LOG_STYLE`

Style logs. Equivalent to `$RUST_LOG_STYLE`.

See <https://docs.rs/env_logger/#disabling-colors>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_LOG_STYLE="never"
```

### `$CARGOGREEN_RUNNER`

Pick which executor to use: `"docker"` (default), `"podman"` or `"none"`.

The [runner gets forwarded these environment variables](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables):
* `$BUILDKIT_COLORS`
* [`$BUILDKIT_HOST`](https://docs.docker.com/build/building/variables/#buildkit_host)
* `$BUILDKIT_PROGRESS`
* `$BUILDKIT_TTY_LOG_LINES`
* `$BUILDX_BAKE_GIT_AUTH_HEADER`
* `$BUILDX_BAKE_GIT_AUTH_TOKEN`
* `$BUILDX_BAKE_GIT_SSH`
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* [`$DOCKER_BUILDKIT`](https://docs.docker.com/build/buildkit/#getting-started)
* `$BUILDX_CONFIG`
* `$BUILDX_CPU_PROFILE`
* `$BUILDX_EXPERIMENTAL`
* `$BUILDX_GIT_CHECK_DIRTY`
* `$BUILDX_GIT_INFO`
* `$BUILDX_GIT_LABELS`
* `$BUILDX_MEM_PROFILE`
* `$BUILDX_METADATA_PROVENANCE`
* `$BUILDX_METADATA_WARNINGS`
* `$BUILDX_NO_DEFAULT_ATTESTATIONS`
* `$BUILDX_NO_DEFAULT_LOAD`
* `$DOCKER_API_VERSION`
* `$DOCKER_CERT_PATH`
* `$DOCKER_CONFIG`
* `$DOCKER_CONTENT_TRUST`
* `$DOCKER_CONTENT_TRUST_SERVER`
* [`$DOCKER_CONTEXT`](https://docs.docker.com/reference/cli/docker/#environment-variables)
* `$DOCKER_DEFAULT_PLATFORM`
* `$DOCKER_HIDE_LEGACY_COMMANDS`
* [`$DOCKER_HOST`](https://docs.docker.com/engine/security/protect-access/)
* `$DOCKER_TLS`
* `$DOCKER_TLS_VERIFY`
* `$EXPERIMENTAL_BUILDKIT_SOURCE_POLICY`
* `$HTTP_PROXY`
* `$HTTPS_PROXY`
* `$NO_PROXY`

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RUNNER="docker"
```

### `$BUILDX_BUILDER`

Sets which BuildKit builder to use, through `$BUILDX_BUILDER`.

See <https://docs.docker.com/build/building/variables/#buildx_builder>

* Unset: creates & handles a builder named `"supergreen"`. Upgrades it if too old, while trying to keep old cached data
* Set to `""`: skips using a builder
* Set to `"supergreen"`: uses existing and just warns if too old
* Set: use that as builder, no questions asked

See also
* `$DOCKER_HOST`: <https://docs.docker.com/engine/reference/commandline/cli/#environment-variables>
* `$DOCKER_CONTEXT`: <https://docs.docker.com/reference/cli/docker/#environment-variables>
* `$BUILDKIT_HOST`: <https://docs.docker.com/build/building/variables/#buildkit_host>

### `$CARGOGREEN_BUILDER_IMAGE`

Sets which BuildKit builder version to use.

See <https://docs.docker.com/build/builders/>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:latest"
```

### `$CARGOGREEN_SYNTAX_IMAGE`

Sets which BuildKit frontend syntax to use.

See <https://docs.docker.com/build/buildkit/frontend/#stable-channel>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_SYNTAX_IMAGE="docker-image://docker.io/docker/dockerfile:1"
```

### `$CARGOGREEN_REGISTRY_MIRRORS`

Registry mirrors for Docker Hub (docker.io). Defaults to GCP & AWS mirrors.

See <https://docs.docker.com/build/buildkit/configure/#registry-mirror>

Namely hosts with maybe a port and a path:
* `dockerhub.timeweb.cloud`
* `dockerhub1.beget.com`
* `localhost:5000`
* `mirror.gcr.io`
* `public.ecr.aws/docker`

```toml
registry-mirrors = [ "mirror.gcr.io", "public.ecr.aws/docker" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_REGISTRY_MIRRORS="mirror.gcr.io,public.ecr.aws/docker"
```

### `$CARGOGREEN_CACHE_IMAGES`

Both read and write cached data to and from image registries

Exactly a combination of [Green::cache_from_images] and [Green::cache_to_images].

See
* `type=registry` at <https://docs.docker.com/build/cache/backends/>
* and <https://docs.docker.com/build/cache/backends/registry/>

```toml
cache-images = [ "docker-image://my.org/team/my-project", "docker-image://some.org/global/cache" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_CACHE_IMAGES="docker-image://my.org/team/my-project,docker-image://some.org/global/cache"
```

### `$CARGOGREEN_CACHE_FROM_IMAGES`

Read cached data from image registries

See also [Green::cache_images] and [Green::cache_to_images].

```toml
cache-from-images = [ "docker-image://my.org/team/my-project-in-ci", "docker-image://some.org/global/cache" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_CACHE_FROM_IMAGES="docker-image://my.org/team/my-project-in-ci,docker-image://some.org/global/cache"
```

### `$CARGOGREEN_CACHE_TO_IMAGES`

Write cached data to image registries

Note that errors caused by failed cache exports are not ignored.

See also [Green::cache_images] and [Green::cache_from_images].

```toml
cache-to-images = [ "docker-image://my.org/team/my-fork" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_CACHE_TO_IMAGES="docker-image://my.org/team/my-fork"
```

### `$CARGOGREEN_FINAL_PATH`

Write final containerfile to given path.

Helps e.g. create a containerfile of e.g. a binary to use for best caching of dependencies.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_FINAL_PATH="$PWD/my-bin@1.0.0.Dockerfile"
```

### `$CARGOGREEN_BASE_IMAGE`

Sets the base Rust image, as an image URL (or any build context, actually).

If needing additional envs to be passed to rustc or build script, set them in the base image.

This can be done in that same config file with `base-image-inline`.

See also:
* `also-run`
* `base-image-inline`
* `additional-build-arguments`

For remote builds: make sure this is accessible non-locally.

```toml
base-image = "docker-image://docker.io/library/rust:1-slim"
```

The value must start with `docker-image://` and image must be available on the `$DOCKER_HOST`, eg:
```shell
export CARGOGREEN_BASE_IMAGE=docker-image://rustc_with_libs
DOCKER_HOST=ssh://my-remote-builder docker buildx build -t rustc_with_libs - <<EOF
FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
RUN set -eux && apt update && apt install -y libpq-dev libssl3
ENV KEY=value
EOF
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/rust:1-slim"
```

### `$CARGOGREEN_SET_ENVS`

Pass environment variables through to build runner.

May be useful if a build script exported some vars that a package then reads.
See also:
* `packages`

See about `$GIT_AUTH_TOKEN`: <https://docs.docker.com/build/building/secrets/#git-authentication-for-remote-contexts>

NOTE: this doesn't (yet) accumulate dependencies' set-envs values!
Meaning only the top-level crate's setting is used, for all crates/dependencies.

```toml
set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_SET_ENVS="GIT_AUTH_TOKEN,TYPENUM_BUILD_CONSTS,TYPENUM_BUILD_OP"
```

### `$CARGOGREEN_BASE_IMAGE_INLINE`

Sets the base Rust image for root package and all dependencies, unless themselves being configured differently.

See also:
* `with-network`
* `additional-build-arguments`

In order to avoid unexpected changes, you may want to pin the image using an immutable digest.

Note that carefully crafting crossplatform stages can be non-trivial.

```toml
base-image-inline = """
FROM --platform=$BUILDPLATFORM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
```

```toml
# This must also be set so digest gets pinned automatically.
base-image = "docker-image://rust:1"
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
IFS='' read -r -d '' CARGOGREEN_BASE_IMAGE_INLINE <<"EOF"
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
EOF
echo "$CARGOGREEN_BASE_IMAGE_INLINE" # (with quotes to preserve newlines)
export CARGOGREEN_BASE_IMAGE_INLINE
export CARGOGREEN_BASE_IMAGE=docker-image://rust:1
```

### `$CARGOGREEN_WITH_NETWORK`

Controls runner's `--network none (default) | default | host` setting.

Set this to `"default"` if e.g. your `base-image-inline` calls curl or wget or installs some packages.

If adding system packages with `add`, this gets automatically set to `"default"`.

It turns out `--network` is part of BuildKit's cache key, so an initial online build won't cache-hit on later offline builds.

Set to `none` when in `$CARGO_NET_OFFLINE` mode. See
  * <https://doc.rust-lang.org/cargo/reference/config.html#netoffline>
  * <https://github.com/rust-lang/rustup/issues/4289>

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_WITH_NETWORK="none"
```

### `$CARGOGREEN_ADD_APT`

Adds OS packages to the base image with `apt install`.

See also:
* `add.apk`
* `add.apt-get`
* `base-image`

```toml
[package.metadata.green]
add.apt = [ "libpq-dev", "pkg-config" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APT="libpq-dev,pkg-config"

# Inspect the resulting base image with:
cargo green supergreen env CARGOGREEN_BASE_IMAGE_INLINE
```

### `$CARGOGREEN_ADD_APT_GET`

Adds OS packages to the base image with `apt-get install`.

See also:
* `add.apk`
* `add.apt`
* `base-image`

```toml
add.apt-get = [ "libpq-dev", "pkg-config" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APT_GET="libpq-dev,pkg-config"

# Inspect the resulting base image with:
cargo green supergreen env CARGOGREEN_BASE_IMAGE_INLINE
```

### `$CARGOGREEN_ADD_APK`

Adds OS packages to the base image with `apk add`.

See also:
* `add.apt`
* `add.apt-get`
* `base-image`

```toml
add.apk = [ "libpq-dev", "pkgconf" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APK="libpq-dev,pkg-conf"

# Inspect the resulting base image with:
cargo green supergreen env CARGOGREEN_BASE_IMAGE_INLINE
```

### `$CARGOGREEN_EXPERIMENT`

A comma-separated list of names of features to activate.

A name that does not match exactly is an error.

* `finalpathnonprimary`:
  - Write final containerfile on every rustc call.
  - Helps e.g. debug builds failing too early.

* `incremental`:
  - Also wrap incremental compilation.
  - See <https://doc.rust-lang.org/cargo/reference/config.html#buildincremental>

* `repro`:
  - Try and test for builds hermeticity and reproducibility.
  - See <https://docs.docker.com/reference/cli/docker/buildx/build/#no-cache-filter>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_EXPERIMENT="finalpathnonprimary,repro"
```

## Alternatives
In no particular order:
* **ipetkov**'s `crane`: [A Nix library for building cargo projects. Never build twice thanks to incremental artifact caching.](https://github.com/ipetkov/crane)
  * [crane.dev](https://crane.dev/)
  * `=>` Very complete! Relies on `nix` tools and language.
* **Mozilla**'s `sccache`: [sccache - Shared Compilation Cache](https://github.com/mozilla/sccache)
  * [Cargo Book ~ Build cache ~ Shared cache](https://doc.rust-lang.org/cargo/reference/build-cache.html#shared-cache)
  * `=>` Relies on everyone having the same paths and doesn't cache all crate types.
* **garentyler**'s `distrustc`: [A Rust-compatible distcc implementation](https://github.com/garentyler/distrustc)
  * [`distcc`'s manpage](https://linux.die.net/man/1/distcc)
* **LukeMathWalker**'s `cargo-chef`: [A cargo-subcommand to speed up Rust Docker builds using Docker layer caching.](https://github.com/LukeMathWalker/cargo-chef)
  * [5x Faster Rust Docker Builds with cargo-chef](https://www.lpalmieri.com/posts/fast-rust-docker-builds/)
  * `=>` Relies on everyone having the same paths + cache isn't crate-granular.
* **Bazel**'s `rules_rust`: [Rules Rust](https://bazelbuild.github.io/rules_rust/)
  * [Building a Rust workspace with Bazel](https://www.tweag.io/blog/2023-07-27-building-rust-workspace-with-bazel/)
  * [Rust rules for Bazel](https://github.com/bazelbuild/rules_rust)
  * `=>` Replaces `cargo` with `bazel`.
* **sgeisler**'s `cargo-remote`: [cargo subcommand to compile rust projects remotely](https://github.com/sgeisler/cargo-remote)
  * [Building on a remote server](https://www.reddit.com/r/rust/comments/im7bb1/building_on_a_remote_server/)
  * `=>` Uses `rsync` and `ssh`; *seems unmaintained*.
* **Swatinem**'s `rust-cache`: [A GitHub Action that implements smart caching for rust/cargo projects](https://github.com/Swatinem/rust-cache)
  * `=>` A GitHub Action.
* **cross-rs**'s `cargo-cross`: [“Zero setup” cross compilation and “cross testing” of Rust crates](https://github.com/cross-rs/cross)
  * Look at all these (compilation + testing) [Supported targets](https://github.com/cross-rs/cross#supported-targets)!
  * Remote building and caching with [Data Volumes](https://github.com/cross-rs/cross/blob/main/docs/remote.md#data-volumes)
  * `=>` *seldomly* maintained but a lifesaver for cross compilation.
See also this article on what `cargo-green` does (perfect layering):
* [Better support of Docker layer caching in Cargo](https://hackmd.io/@kobzol/S17NS71bh)
  * [Exploring the problem of faster Cargo Docker builds](https://www.reddit.com/r/rust/comments/126xeyx/exploring_the_problem_of_faster_cargo_docker/)
  * [Another reddit discussion](https://www.reddit.com/r/rust/comments/126whnc/better_support_of_docker_layer_caching_in_cargo/)

## Origins
* PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh
* Initial blog post https://fenollp.github.io/faster-rust-builds-docker_host

## Hacking

### `./hack/cli.sh ...`
```shell
# Usage:           $0                              #=> generate CI
#
# Usage:           $0 ( <name@version> | <name> )  #=> cargo install name@version
# Usage:           $0   ok                         #=> cargo install all working bins
#
# Usage:           $0 ( build | package | test )   #=> cargo build ./cargo-green
#
# Usage:    jobs=1 $0 ..                           #=> cargo --jobs=$jobs
# Usage: offline=1 $0 ..                           #=> cargo --frozen (defaults to just: --locked)
# Usage:    rmrf=1 $0 ..                           #=> rm -rf $CARGO_TARGET_DIR/*; cargo ...
# Usage:   reset=1 $0 ..                           #=> docker buildx rm $BUILDX_BUILDER; cargo ...
# Usage:   clean=1 $0 ..                           #=> Both reset=1 + rmrf=1
# Usage:   final=0 $0 ..                           #=> Don't generate final Containerfile
#
# Usage:    DOCKER_HOST=.. $0 ..                   #=> Overrides machine
# Usage: BUILDX_BUILDER=.. $0 ..                   #=> Overrides builder (set to "empty" to set BUILDX_BUILDER='')
```

### `./hack/recipes.sh`
Syncs `./recipes/*.Dockerfile` files.

### `./hack/caching.sh`
Verifies properties about caching crates & granularity.
> docker buildx prune --all --force

### `./hack/hit.sh`
Estimate the of amount of crates reused through compilation of `./recipes/` `--> ~5%`!
Expecting more with larger/more representative corpus + smart locking of transitive deps.
```
recipes/buildxargs@1.4.0.Dockerfile
8< 8< 8<
3: dep-l-utf8parse-0.2.1-522ff71b25340e24
5: dep-l-bitflags-1.3.2-70ce9f1f2fa253bc
5: dep-l-strsim-0.10.0-fd42a4ea370e31ec
5: dep-l-unicode-ident-1.0.12-4c1dc76c11b3deb8
6: dep-l-cfg-if-1.0.0-da34da6838abd7f1

Total recipes: 15
Total stages: 1065
Stages in common: 58
5.44%
```

### `./hack/portable.sh`
Count "portable" recipes in this repo (not portable = usage of local build contexts).

### When `git-bisect`ing
```make
all:
	cargo +nightly fmt --all
	./hack/clis.sh | tee .github/workflows/clis.yml
	./hack/self.sh | tee .github/workflows/self.yml
	CARGO_TARGET_DIR=$${CARGO_TARGET_DIR:-target/clippy} cargo clippy --locked --frozen --offline --all-targets --all-features -- --no-deps -W clippy::cast_lossless -W clippy::redundant_closure_for_method_calls -W clippy::str_to_string -W clippy::unnecessary_wraps
	RUST_MIN_STACK=8000000 cargo nextest run --all-targets --all-features --locked --frozen --offline --no-fail-fast
	git --no-pager diff --exit-code
```

## Goals
* [x] seamlessly build on another machine (with more cores, more cache)
  * [x] support remote builds by setting env `DOCKER_HOST=` with e.g. `ssh://me@beaffy-machine.internal.net`
    * [x] Build cache is saved remotely, artifacts are saved locally
    * [x] Tests building happens on remote machine, execution happens on local machine
* [x] seamlessly integrate with normal `cargo` usage
  * [x] only pull sources from local filesystem
  * [x] produce the same intermediary artefacts as local `cargo` does
  * [x] fallback to normal, local `rustc` anytime
    * switching from this wrapper back to local `rustc` does necessitate a fresh build
* [ ] wrap `rustc` calls in `buildkit`-like calls (`docker`, `podman`)
  * [x] `docker`
  * [ ] `podman` TODO: test in CI
  * [ ] deps compatibility
    * [x] handle Rust-only deps
    * [ ] handle all the other deps (expand this list) (use `crater`)
      * [x] `C` deps
      * [ ] ...
  * [x] runner compatibility
    * [x] set `.dockerignore`s (to be authoritative on srcs)
  * [x] trace these outputs (STDOUT/STDERR) for debugging
* [x] available as a `rustc` wrapper through `$RUSTC_WRAPPER`
* [ ] available as a `cargo` subcommand
  * [ ] configuration profiles (user, team, per-workspace, per-crate, CI, ...)
  * [x] seamlessly use current/local `rustc` version
    * [x] support overriding `rustc` base image
  * [ ] seamlessly use current/local tools (`mold`, ...)
    * [ ] config expressions on top of base image config?
    * [ ] just suggest an inline `Dockerfile` stage?
  * [ ] support CRUD-ish operations on local/remotes cache
  * [x] `[SEC]` support building a crate without it having network access
* [ ] integrate with shipping OCI images
* [ ] share cache with the World cf. [`user-wide-cache`](https://rust-lang.github.io/rust-project-goals/2024h2/user-wide-cache.html)
  * [x] never rebuild a dep (for a given version of `rustc`, ...)
    * [x] ensure finest cache granularity (crate-level)
    * [x] free users from cache key built from `Cargo.lock` (changes on every release cut!)
  * [ ] share cache with other projects on local machine
    * [ ] fix `WORKDIR`s + rewrite paths with `remap-path-prefix` 
  * [ ] share cache with CI and team
    * [ ] share cache with CI (at least for a single user)
  * [ ] `[SEC]` ensure private deps don't leak through/to cache
  * [ ] CLI gives the Dockerfile that `cargo install`'s any crate
* [ ] suggest a global cache -faciliting configuration profile
* [ ] integrate with `cross`
  * [ ] build for a non-local target
  * [ ] run/test for a non-local target (with `cross`'s same caveats ie. QEMU)


## Upstream issues & patches
* [ ] [`rust`: Compile a crate from its source archive directly](https://github.com/rust-lang/rust/issues/128884)
* [ ] [`cargo`: Tell `rustc` wrappers which envs to pass through to allow env sandboxing](https://github.com/rust-lang/cargo/issues/14444)
* [ ] [`buildkit`: `docker/dockerfile` image tags are late](https://github.com/moby/buildkit/issues/6118)
* gRPC buffers too small
  * [x] [`buildkit`: Build function: ResourceExhausted: grpc: received message larger than max (_ vs. 4194304)](https://github.com/moby/buildkit/issues/5217)
  * [x] [`buildx`: `ResourceExhausted: grpc: received message larger than max (_ vs. 4194304)`](https://github.com/docker/buildx/issues/2453)
  * [ ] [`buildkit`: remote `docker buildx build` with large dockerfile gives `trying to send message larger than max (22482550 vs. 16777216)` error](https://github.com/moby/buildkit/issues/6097)
  * [ ] [`buildx`: `error reading from server: connection error: COMPRESSION_ERROR`](https://github.com/docker/buildx/issues/3637)
* [ ] [`buildkit`: Support multiple input dockerfiles (single frontend)](https://github.com/moby/buildkit/issues/6508)
* [x] [`buildkit`: Support extracting `ADD --checksum=.. https://.. ..`](https://github.com/moby/buildkit/issues/4907)
* [ ] [`buildkit`: `RUN --no-cache` to skip reading & writing a RUN layer to cache](https://github.com/moby/buildkit/issues/6303)
* [ ] [`buildkit`: FR: an option to delay --cache-to pushes](https://github.com/docker/buildx/issues/3150)
* [x] [`buildkit`: Looking for a consistently "latest" BuildKit image tag](https://github.com/moby/buildkit/discussions/6134)
* [x] [`buildkit`: docker/dockerfile image tags are late](https://github.com/moby/buildkit/issues/6118)
* [ ] [`buildx`: Support --format=json for buildx du --verbose](https://github.com/docker/buildx/issues/3367)
* TODO
  1. buildkit flag to disable dockerignore and save disk read
    * --ignore-file (closed) [Add support for specifying .dockerignore file with -i/--ignore](https://github.com/moby/moby/issues/12886)
    * --no-ignore-file
  1. docker build support multiple input files
    * --file-part
    * >=1 stage per Dockerfile part file
      * order doesn't matter: order is fixed when consolidation happens (internally)
  1. cargo + docker
    * [cargo build --dependencies-only](https://github.com/rust-lang/cargo/issues/2644#issuecomment-2304774192)
* [Getting an image's digest fast, within a docker-container builder](https://github.com/docker/buildx/discussions/3363)
  * [Inspect image manifest without pushing to registry or load to local docker daemon](https://github.com/moby/buildkit/issues/4854)
  * [Proposal: introduce enhanced image resolution gateway API](https://github.com/moby/buildkit/issues/2944)
* [ ] [`buildkit`: allow exporting cache layers in parallel to the remote registry](https://github.com/moby/buildkit/issues/6123)
* [ ] [`buildkit`: remote docker buildx build with large dockerfile gives trying to send message larger than max (22482550 vs. 16777216) error](https://github.com/moby/buildkit/issues/6097)
* [ ] [`buildx`: `--cache-from` takes longer than actual (cached) build](https://github.com/docker/buildx/issues/3491)
* [ ] [`buildx`: The cache export step hangs](https://github.com/docker/buildx/issues/537)
* [ ] [`buildkit`: `COPY --rewrite-timestamp ...` to apply SOURCE_DATE_EPOCH build arg value to the timestamps of the files](https://github.com/moby/buildkit/issues/6348)
* [ ] [`buildkit`: Dockerfile frontend: `ADD --checksum=.. https://..` hides HTTP error](https://github.com/moby/buildkit/issues/6380)
* [ ] [`buildkit`: Support passing `--local context=FILE`](https://github.com/moby/buildkit/issues/6410)


## En vrac
* Proposal: c8d: expose contentstore API #44369 https://github.com/moby/moby/issues/44369
*  Incremental export transfer #1224 https://github.com/moby/buildkit/issues/1224
* "sending tarball" takes a long time even when the image already exists #107 https://github.com/docker/buildx/issues/107
* Build drivers https://docs.docker.com/build/drivers/
* https://docs.docker.com/build/ci/github-actions/configure-builder/#max-parallelism
* https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
* https://github.com/moby/buildkit#export-cache
* `tunnel tty into a docker build through http`
* docker build `remote` driver https://docs.docker.com/build/drivers/remote
* rootless `k8s` driver https://docs.docker.com/build/drivers/kubernetes/#rootless-mode
* tune many options https://docs.docker.com/build/drivers/docker-container/
  * https://docs.docker.com/config/containers/resource_constraints/
  * https://hub.docker.com/r/moby/buildkit
    * https://github.com/moby/buildkit/releases
* https://docs.docker.com/build/attestations/sbom/
  * https://github.com/moby/buildkit/blob/647a997b389757068760410053873745acabfc80/docs/attestations/sbom.md?plain=1#L48
  * `BUILDKIT_SBOM_SCAN_CONTEXT and BUILDKIT_SBOM_SCAN_STAGE`
* [Support extracting `ADD --checksum=.. https://.. ..` #4907](https://github.com/moby/buildkit/issues/4907)
* [`docker image ls --filter=reference=docker.io/$MY/$IMG` != `docker image ls --filter=reference=$MY/$IMG` #47809](https://github.com/moby/moby/issues/47809)
* [Proposal: csv syntax for git repos #4905](https://github.com/moby/buildkit/issues/4905)
* [`prune`: filtering out `ADD --checksum=... https://...` entries #2448](https://github.com/docker/buildx/issues/2448)
* [`-o=.`: `open $HOME/.local/share/docker/overlay2/066f6../work/work: permission denied` #2219](https://github.com/docker/buildx/issues/2219)

* `cargo restrict targets of crate`
*  Target configuration for binaries #9208 https://github.com/rust-lang/cargo/issues/9208
*  Unsafe fields #3458 https://github.com/rust-lang/rfcs/pull/3458
*  Warning when large binary files are included into the bundle #9058 https://github.com/rust-lang/cargo/issues/9058
*  Hermetic build mode #9506 https://github.com/rust-lang/cargo/issues/9506
*  Feature Request static asserts #2790 https://github.com/rust-lang/rfcs/issues/2790
* greater supply chain attack risk due to large dependency trees? https://www.reddit.com/r/rust/comments/102yz60/greater_supply_chain_attack_risk_due_to_large/
  * https://github.com/rust-secure-code/cargo-supply-chain
* https://doc.rust-lang.org/rustc/command-line-arguments.html#option-emit
* https://rust-lang.github.io/rustup/overrides.html
* https://docs.rs/rustflags/0.1.4/rustflags/index.html
* [Provide better diagnostics for why crates are rebuilt](https://github.com/rust-lang/cargo/issues/2904)
* `[build] rustflags = ["--remap-path-prefix"`
  * [RFC: `trim-paths`](https://rust-lang.github.io/rfcs/3127-trim-paths.html)
  * [RFC: `-Zremap-cwd-prefix=.`](https://github.com/rust-lang/rust/issues/89434)
* [`crater`: Run experiments across parts of the Rust ecosystem!](https://github.com/rust-lang/crater)
* [`cargo-options` Clap parser](https://docs.rs/cargo-options/latest/cargo_options/struct.Build.html)
* /r/Rust scare [Serde has started shipping precompiled binaries with no way to opt out](https://www.reddit.com/r/rust/comments/15va70a/serde_has_started_shipping_precompiled_binaries/)
* [assertion failed: edges.remove(&key) #13889](https://github.com/rust-lang/cargo/issues/13889)
* [How we rescued our build process from 24+ hour nightmares](https://www.reddit.com/r/rust/comments/1emhq19/how_we_rescued_our_build_process_from_24_hour/)
* [Partially sandbox your Rust builds](https://www.reddit.com/r/rust/comments/hjxh2a/partially_sandbox_your_rust_builds/)
* [Is just me or Rust is too heavy for my computer to handle?](https://www.reddit.com/r/rust/comments/1f6bvw3/is_just_me_or_rust_is_too_heavy_for_my_computer/)
* [Doesn't detect Docker Rootless #4](https://github.com/TheLarkInn/is-docker/issues/4)
* [Using S3 as a container registry](https://ochagavia.nl/blog/using-s3-as-a-container-registry/)
* [What's the best practice for caching compilation of Rust dependencies?](https://www.reddit.com/r/rust/comments/sunme5/whats_the_best_practice_for_caching_compilation/)
* [Enable Fast Compiles](https://bevy.org/learn/quick-start/getting-started/setup/#enable-fast-compiles-optional)
  * [Compiling is slow...](https://www.reddit.com/r/bevy/comments/1mrvcis/compiling_is_slow/)
* [Everytime I try to use Tauri for Android... Why?](https://www.reddit.com/r/rust/comments/1mlzz5l/media_everytime_i_try_to_use_tauri_for_android_why/)
  * on size of compilation artifacts
  * > ultimately the "real" solution has got to be a complete overhaul of the entire compilation system to be entirely on-demand in a granular basis, rather than "compile every crate in the dependency tree wholesale".
  * suggestion to use a shared target folder
