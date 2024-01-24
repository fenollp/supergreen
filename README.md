# rustcbuildx
`RUSTC_WRAPPER` that uses `docker buildx bake`

## Usage

* Ensure `~/.cargo/bin` is in `$PATH`
* Ensure [`docker buildx bake`](https://docs.docker.com/engine/reference/commandline/buildx_bake/) is installed
* Known to work on `Ubuntu 22.04` with `github.com/docker/buildx v0.11.2 9872040` and `rust 1.73`

```shell
RUSTC_WRAPPER=rustcbuildx cargo build ...
RUSTC_WRAPPER=rustcbuildx cargo check ...
RUSTC_WRAPPER=rustcbuildx cargo clippy ...
RUSTC_WRAPPER=rustcbuildx cargo install ...
RUSTC_WRAPPER=rustcbuildx cargo test ...
# or
export RUSTC_WRAPPER=rustcbuildx
cargo build ...
cargo check ...
cargo clippy ...
cargo install ...
cargo test ...
```

### Fine tuning settings

```shell
rustcbuildx@version: $RUSTC_WRAPPER tool to sandbox cargo builds and execute jobs remotely
    https://github.com/fenollp/rustcbuildx

Usage:
  rustcbuildx env             Show used values
  rustcbuildx pull            Pulls images (respects $DOCKER_HOST)
  rustcbuildx -h | --help
  rustcbuildx -V | --version
```

## Remote execution

Say you have a bigger machine in your `~/.ssh/config` called `extra_oomph`:

```shell
export DOCKER_HOST=ssh://extra_oomph
# Then
export RUSTC_WRAPPER=rustcbuildx
cargo test ...
```

* Build cache is saved remotely
* Build artifacts are saved locally
* Tests building happens on remote machine
* Tests execution happens on local machine

## Installation

```shell
# Installs to ~/.cargo/bin
cargo install --locked --force --git https://github.com/fenollp/rustcbuildx.git

# Make sure it's in your $PATH
which rustcbuildx
```

## Origins

PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh


## docker / podman / buildkit
* Proposal: c8d: expose contentstore API #44369 https://github.com/moby/moby/issues/44369
*  --load with moby containerd store should use the oci exporter #1813 https://github.com/docker/buildx/pull/1813
*  Make --load faster #626 https://github.com/docker/buildx/issues/626
*  Incremental export transfer #1224 https://github.com/moby/buildkit/issues/1224
* "sending tarball" takes a long time even when the image already exists #107 https://github.com/docker/buildx/issues/107
*  mount=type=cache more in-depth explanation? #1673 https://github.com/moby/buildkit/issues/1673
* Build drivers https://docs.docker.com/build/drivers/
*  Race condition when using cache-mounts with multi-arch builds. #549 https://github.com/docker/buildx/issues/549
* https://docs.docker.com/build/ci/github-actions/configure-builder/#max-parallelism
* https://docs.docker.com/engine/reference/builder/#buildkit-built-in-build-args
* https://github.com/moby/buildkit#export-cache
* `tunnel tty into a docker build through http`
* file:///home/pete/wefwefwef/docker-bake-invalidate-child-cache.git

## rustc / cargo
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

## cross
*  Convert --target-dir to use absolute paths. https://github.com/cross-rs/cross/commit/2504e04375a4a8f62f5dc62f95745701521c590e
