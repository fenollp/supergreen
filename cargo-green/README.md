# [`cargo-green`](https://github.com/fenollp/supergreen/tree/main/cargo-green)
Cached & remote-ready Rust projects builder.

`cargo-green` is a `cargo` plugin that sets a `$RUSTC_WRAPPER` then calls `cargo`.


## Configuration

Reads envs
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* `$CARGOGREEN_LOG`: sets `$RUSTCBUILDX_LOG` level
* `$CARGOGREEN_REMOTE`: *reserved for now*
* [`$DOCKER_HOST`](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables)

Sets
* [`$RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)


## Usage

No more dependencies than [the transitive ones coming from](../rustcbuildx#usage) usage of `rustcbuildx`.

```shell
cargo green build
cargo green b
cargo green check
cargo green c
cargo green clean
cargo green clippy
cargo green doc
cargo green d
cargo green new
cargo green init
cargo green add
cargo green remove
cargo green run
cargo green r
cargo green test
cargo green t
cargo green bench
cargo green update
cargo green search
cargo green publish
cargo green install
cargo green uninstall

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

## Installation

```shell
# Installs to ~/.cargo/bin
cargo install --locked --force --git https://github.com/fenollp/supergreen.git cargo-green rustcbuildx

# Make sur $CARGO_HOME/bin is in your $PATH
which cargo-green && which rustcbuildx
```
