Grants network access to some dependencies. `rustc` calls and build scripts otherwise run without any (`RUN --network=none`).

Settings are per package name, optionally per version requirement, written as in `Cargo.toml`'s `[dependencies]` (`'0.1.5'` means `>=0.1.5, <0.2.0`):

```toml
[package.metadata.green.deps.openssl-sys]
network = true
[package.metadata.green.deps.zipsign-api.'0.1.5']
network = true
[package.metadata.green.deps.something_else.'0, >=0.11']
network = true
```

A grant covers both compiling the package and running its build script. A version requirement's setting wins over the package's.

Base stages (installing the toolchain and the OS packages of `add`) always have network access, as does fetching sources (`ADD`).

When installing (`cargo green install`) there is no `Cargo.toml` to edit: use the environment variable. Its `;`-separated values are `name`, `name@requirement`.

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_DEPS_NETWORK="openssl-sys;zipsign-api@0.1.5;something_else@0, >=0.11"
```

