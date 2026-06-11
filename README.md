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
  - [`$CARGOGREEN_WITH_NETWORK`](#cargogreen_with_network)
  - [`$CARGOGREEN_COMPONENTS`](#cargogreen_COMPONENTS)
  - [`$CARGOGREEN_ADD_APT`](#cargogreen_add_apt)
  - [`$CARGOGREEN_ADD_APK`](#cargogreen_add_apk)
  - [`$CARGOGREEN_EXPERIMENT`](#cargogreen_experiment)
- [Alternatives](#alternatives)
- [Origins](#origins)

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
  cargo green supergreen setup                                   Create required symlinks
  cargo green supergreen env [ENV ...]                           Show used values
  cargo green supergreen doc [ENV ...]                           Documentation of said values
  cargo green supergreen show-rust-base                          Show base stage in use
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
  cargo green supergreen env CARGOGREEN_BASE_IMAGE 2>/dev/null
  cargo green supergreen help
  { cargo green supergreen setup 2>/dev/null || true; } | sudo /bin/sh -xe

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
  * additionally, this `index.crates.io` path is created locally.
* `$CARGO_HOME` has to get linked to the one used inside the image (`/usr/local/cargo`): `sudo ln -s ~/.cargo /usr/local/cargo`
* `git` dependencies are pinned to their commit hash
* Produced files timestamps' are rewritten to some fixed epoch (`SOURCE_DATE_EPOCH`)

Switching from `cargo build` to `cargo green build` requires a `cargo clean` because of the additional work `cargo-green` does.
Running `cargo build` after `cargo green build` works normally though: the same files are produced.

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

When runner is set to `none`, the above runner-specific environment variables are ineffective and they are ignored.

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

Exactly a combination of [Cache::from_images] and [Cache::to_images].

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

See also [Cache::images] and [Cache::to_images].

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

See also [Cache::images] and [Cache::from_images].

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

Sets the base as an image URL, with scheme: `docker-image://`.

On top of this, [rustup](https://rustup.rs/) installs the toolchain.
This toolchain is picked by your local Rust installation or through `cargo +toolchain ..`.

If needing additional envs to be passed to rustc or build script, set them in the base image.

See also:
* `components`
* `also-run`
* `additional-build-arguments`

```toml
base-image = "docker-image://docker.io/library/debian:trixie-slim"
```

For remote builds, make sure this image is accessible non-locally.
```shell
export CARGOGREEN_BASE_IMAGE=docker-image://my_ubuntu_with_libs_and_envs
DOCKER_HOST=ssh://my-remote-builder docker buildx build -t my_ubuntu_with_libs_and_envs - <<EOF
FROM ubuntu:latest@sha256:c4a8d5503dfb2a3eb8ab5f807da5bc69a85730fb49b5cfca2330194ebcc41c7b
RUN set -eux && apt update && apt install -y libpq-dev libssl3
ENV KEY=value
EOF
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/debian:trixie-slim"
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

### `$CARGOGREEN_WITH_NETWORK`

Controls runner's `--network none (default) | default | host` setting.

Set this to `"default"` if e.g. your additional stages to the base image call `curl` or `wget` or install any packages.

If adding system packages via `add`, this gets automatically set to `"default"`.

It turns out `--network` is part of BuildKit's cache key, so an initial online build won't cache-hit on later offline builds.

Set to `none` when in `$CARGO_NET_OFFLINE` mode. See
  * <https://doc.rust-lang.org/cargo/reference/config.html#netoffline>
  * <https://github.com/rust-lang/rustup/issues/4289>

```toml
[package.metadata.green]
with-network = "none"
base-image = "docker-image://docker.io/library/ubuntu@sha256:c4a8d5503dfb2a3eb8ab5f807da5bc69a85730fb49b5cfca2330194ebcc41c7b"
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_WITH_NETWORK="none"
```

### `$CARGOGREEN_COMPONENTS`

Tells `rustup` to add the given components to the base image.

See <https://rust-lang.github.io/rustup/concepts/components.html>

```toml
[package.metadata.green]
components = [ "rust-src", "llvm-tools-preview" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_COMPONENTS="rust-src,llvm-tools-preview"
```

### `$CARGOGREEN_ADD_APT`

Adds OS packages to the base image with `apt install`.

Supported syntax:
* `libssl-dev`
* `libssl-dev(>=3.5)`
* `libssl-dev(=3.5.5-1~deb13u2)` *is the encouraged usage for better build cache hits*

See also:
* `add.apk`
* `base-image`

```toml
[package.metadata.green]
add.apt = [ "libpq-dev", "pkg-config" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APT="libpq-dev,pkg-config"

# Inspect the resulting base stage with:
cargo green supergreen show-rust-base 2>/dev/null
```

### `$CARGOGREEN_ADD_APK`

Adds OS packages to the base image with `apk add`.

See also:
* `add.apt`
* `base-image`

```toml
add.apk = [ "libpq-dev", "pkgconf" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APK="libpq-dev,pkg-conf"

# Inspect the resulting base stage with:
cargo green supergreen show-rust-base 2>/dev/null
```

### `$CARGOGREEN_EXPERIMENT`

A comma-separated list of names of features to activate.

A name that does not match exactly is an error.

* `finalpathcomments`:
  - Write final containerfile on every rustc call.
  - Contains internal debugging structs: as commented TOML
  - Helps e.g. debug builds failing too early.

* `finalpathnonprimary`:
  - Write final containerfile on every rustc call.
  - Perfect format to share the file.
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
  * [`sccache`'s Known Caveats](https://github.com/mozilla/sccache/tree/5d52f91da50fee9b48fa5f6db1f19bc149a7a0f2#known-caveats)
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
