# [`cargo-green`](https://github.com/fenollp/supergreen/tree/main/cargo-green)
Cached & remote-ready Rust projects builder by forwarding `rustc` calls to [BuildKit](https://github.com/moby/buildkit) builders.

`cargo-green` is
* a `cargo` plugin that sets a `$RUSTC_WRAPPER` then calls `cargo`.
* a [`RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-wrapper) that builds Dockerfiles


## Configuration

Reads envs (show values with `cargo green supergreen env`)
* `$CARGOGREEN_ADD_APK`: `add.apk` TOML setting
* `$CARGOGREEN_ADD_APT_GET`: `add.apt-get` TOML setting
* `$CARGOGREEN_ADD_APT`: `add.apt` TOML setting
* `$CARGOGREEN_BASE_IMAGE_INLINE`: `base-image-inline` TOML setting
* `$CARGOGREEN_BASE_IMAGE`: `base-image` TOML setting
* `$CARGOGREEN_BUILDER_IMAGE`: image to use when creating builder container
* `$CARGOGREEN_CACHE_IMAGES`: `cache-images` TOML setting
* `$CARGOGREEN_FINAL_PATH`: if set, this file will end up with the final Dockerfile that reproduces the build
* `$CARGOGREEN_INCREMENTAL`: `incremental` TOML setting
* `$CARGOGREEN_LOG_PATH`: logfile path
* `$CARGOGREEN_LOG_STYLE`
* `$CARGOGREEN_LOG`: equivalent to `$RUST_LOG` (and doesn't conflict with `cargo`'s)
* `$CARGOGREEN_REMOTE`: *reserved for now*
* `$CARGOGREEN_RUNNER`: use `docker` or `podman` or `none` as build runner
* `$CARGOGREEN_SET_ENVS`: `set-envs` TOML setting
* `$CARGOGREEN_SYNTAX`: use a [`dockerfile:1`](https://hub.docker.com/r/docker/dockerfile)-derived BuildKit frontend
* `$CARGOGREEN_WITH_NETWORK`: `with-network` TOML setting

Also [passes these envs through to the runner](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables):
* `BUILDKIT_PROGRESS`
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* `DOCKER_API_VERSION`
* `DOCKER_CERT_PATH`
* `DOCKER_CONFIG`
* `DOCKER_CONTENT_TRUST_SERVER`
* `DOCKER_CONTENT_TRUST`
* `DOCKER_CONTEXT`
* `DOCKER_DEFAULT_PLATFORM`
* `DOCKER_HIDE_LEGACY_COMMANDS`
* [`$DOCKER_HOST`](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables)
* `DOCKER_TLS_VERIFY`
* `DOCKER_TLS`
* `HTTP_PROXY`
* `HTTPS_PROXY`
* `NO_PROXY`

Sets
* [`$RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)
* `$CARGOGREEN=1`


## Usage

* Ensure at least either a [`docker`](https://github.com/docker/docker-install) or [`podman`](https://podman.io/docs/installation) *client* is installed
* Known to work on `Ubuntu 22.04` with `github.com/docker/buildx v0.11.2 9872040` and `rust 1.73`

```shell
cargo green add
cargo green bench
cargo green build
cargo green check
cargo green clean
cargo green clippy
cargo green doc
cargo green init
cargo green install
cargo green new
cargo green package
cargo green publish
cargo green remove
cargo green run
cargo green search
cargo green test
cargo green uninstall
cargo green update

# or, setting an alias in e.g. ~/.bashrc
alias cargo='cargo green'

# With this, one may also use this set of subcommands: [UNSTABLE API] (refacto into a `cache` cmd)
cargo supergreen config get   VAR*
cargo supergreen config set   VAR VAL
cargo supergreen config unset VAR
cargo supergreen pull-images             Pulls latest versions of images used for the build, no cache (respects $DOCKER_HOST)
cargo supergreen pull-cache              Pulls all from `--cache-from`
cargo supergreen push-cache              Pushes all to `--cache-to`
```

### Fine tuning settings

```shell
cargo-green@0.6.0: Cargo plugin and $RUSTC_WRAPPER to sandbox & cache cargo builds and execute jobs remotely
    https://github.com/fenollp/supergreen

Usage:
  cargo green supergreen env             Show used values
  cargo green fetch                      Pulls images (respects $DOCKER_HOST)
  cargo green supergreen push            Push cache image (all tags)
  cargo green supergreen -h | --help
  cargo green supergreen -V | --version
```


## Remote execution

Say you have a bigger machine in your `~/.ssh/config` called `extra_oomph`:

```shell
export DOCKER_HOST=ssh://extra_oomph
cargo green test ...
```


## Installation

```shell
# Installs to ~/.cargo/bin
cargo install --locked --force --git https://github.com/fenollp/supergreen.git cargo-green

# Make sur $CARGO_HOME/bin is in your $PATH
which cargo-green
```


## Alternatives
In no particular order:
* **ipetkov**'s `crane`: [A Nix library for building cargo projects. Never build twice thanks to incremental artifact caching.](https://github.com/ipetkov/crane)
  * [crane.dev](https://crane.dev/)
  * `=>` Very complete! Relies on `nix` tools and language.
* **Mozilla**'s `sccache`: [sccache - Shared Compilation Cache](https://github.com/mozilla/sccache)
  * [Cargo Book ~ Build cache ~ Shared cache](https://doc.rust-lang.org/cargo/reference/build-cache.html#shared-cache)
  * `=>` Relies on everyone having the same paths and doesn't cache all crate types.
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


### En vrac
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
*  Consider making the src cache read-only. #9455 https://github.com/rust-lang/cargo/issues/9455
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
