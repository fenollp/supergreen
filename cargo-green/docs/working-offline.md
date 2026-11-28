Fetch while online, then build with `--offline` (or `--frozen`):

```shell
cargo green fetch
cargo green fetch --target aarch64-unknown-linux-gnu
cargo green build --target aarch64-unknown-linux-gnu --offline
```

Besides your dependencies (from eg. `crates.io` or `git`), `fetch` pulls the base stage builds start from. Git dependencies aren't pulled ahead of time: building them the first time needs the network.

**Note:** when cross compiling, each target has a base stage of its own, with that target's `std`. `fetch` only pulls those of the `--target`s you give it, not of targets set through `build.target`, `$CARGO_BUILD_TARGET` or aliases. The first build for such a target makes its base, which needs the network. `cargo green supergreen sync` also takes `--target`s. `cargo green supergreen show-rust-base` only knows of the host's base.

Crate compilations and build scripts run without network access whether or not you're offline (but for the dependencies granted it, see [`$CARGOGREEN_DEPS_NETWORK`](#cargogreen_deps_network)), so builds made online get reused offline.


