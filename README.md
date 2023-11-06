# rustcbuildx
`RUSTC_WRAPPER` that uses `docker buildx bake`

## Usage

* Ensure `$HOME/.cargo/bin` is in `$PATH`
* Ensure [`docker buildx bake`](https://docs.docker.com/engine/reference/commandline/buildx_bake/) is installed
* Known to work on `Ubuntu 22.04` with `github.com/docker/buildx v0.11.2 9872040` and `rust 1.73`

```shell
RUSTC_WRAPPER=rustcbuildx cargo build ...
RUSTC_WRAPPER=rustcbuildx cargo check ...
RUSTC_WRAPPER=rustcbuildx cargo test ...
RUSTC_WRAPPER=rustcbuildx cargo clippy ...
# or
export RUSTC_WRAPPER=rustcbuildx
cargo build ...
cargo check ...
cargo test ...
cargo clippy ...
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
# Installs to $HOME/.cargo/bin
cargo install --locked --force --git https://github.com/fenollp/rustcbuildx.git
```

## Origins

PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh
