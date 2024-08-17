# [`cargo-green`](https://github.com/fenollp/supergreen/tree/main/cargo-green)
Cached & remote-ready Rust projects builder by forwarding `rustc` calls to [BuildKit](https://github.com/moby/buildkit) builders.

`cargo-green` is
* a `cargo` plugin that sets a `$RUSTC_WRAPPER` then calls `cargo`.
* a [`RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/config.html#buildrustc-wrapper) that builds Dockerfiles


## Configuration

Reads envs
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* `$CARGOGREEN_LOG`: sets `$RUSTCBUILDX_LOG` level
* `$CARGOGREEN_REMOTE`: *reserved for now*
* [`$DOCKER_HOST`](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables)

* `$RUSTCBUILDX`
* `$RUSTCBUILDX_BASE_IMAGE`
* `$RUSTCBUILDX_BUILDER_IMAGE`
* `$RUSTCBUILDX_CACHE_IMAGE`
* `$RUSTCBUILDX_INCREMENTAL`
* `$RUSTCBUILDX_LOG`
* `$RUSTCBUILDX_LOG_PATH`
* `$RUSTCBUILDX_LOG_STYLE`
* `$RUSTCBUILDX_RUNNER`
* `$RUSTCBUILDX_RUNS_ON_NETWORK`
* `$RUSTCBUILDX_SYNTAX`

Sets
* [`$RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)


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
  cargo green supergreen pull            Pulls images (respects $DOCKER_HOST)
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


### En vrac
* Proposal: c8d: expose contentstore API #44369 https://github.com/moby/moby/issues/44369
*  Incremental export transfer #1224 https://github.com/moby/buildkit/issues/1224
* "sending tarball" takes a long time even when the image already exists #107 https://github.com/docker/buildx/issues/107
*  mount=type=cache more in-depth explanation? #1673 https://github.com/moby/buildkit/issues/1673
* Build drivers https://docs.docker.com/build/drivers/
*  Race condition when using cache-mounts with multi-arch builds. #549 https://github.com/docker/buildx/issues/549
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
* [`crater`: Run experiments across parts of the Rust ecosystem!](https://github.com/rust-lang/crater)
* [`cargo-options` Clap parser](https://docs.rs/cargo-options/latest/cargo_options/struct.Build.html)
* /r/Rust scare [Serde has started shipping precompiled binaries with no way to opt out](https://www.reddit.com/r/rust/comments/15va70a/serde_has_started_shipping_precompiled_binaries/)
* [assertion failed: edges.remove(&key) #13889](https://github.com/rust-lang/cargo/issues/13889)
* [How we rescued our build process from 24+ hour nightmares](https://www.reddit.com/r/rust/comments/1emhq19/how_we_rescued_our_build_process_from_24_hour/)

* [Doesn't detect Docker Rootless #4](https://github.com/TheLarkInn/is-docker/issues/4)

* [Compile a crate from its source archive directly](https://github.com/rust-lang/cargo/issues/14373)
* [Build function: ResourceExhausted: grpc: received message larger than max (_ vs. 4194304)](https://github.com/moby/buildkit/issues/5217)
