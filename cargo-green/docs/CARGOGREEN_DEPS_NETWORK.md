Grants network access to some dependencies. `rustc` calls and build scripts otherwise run without any (`RUN --network=none`).

Settings are per package name, optionally per version requirement:

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

When installing (`cargo green install`) there is no `Cargo.toml` to edit: use the environment variable. Its `;`-separated values are package expressions:
* `name`: all versions of that package
* `name@requirement`: the versions matching that requirement

Names are those Cargo accepts. Requirements are Cargo's, as written in `Cargo.toml`'s `[dependencies]` (see [Specifying Dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html)):
* `0.1.5` or `^0.1.5`: `>=0.1.5, <0.2.0`
* `~1.2.3`: `>=1.2.3, <1.3.0`
* `1.2.*`: `>=1.2.0, <1.3.0`
* `=1.2.3`: that version only
* `>=1.2, <1.5`: all of these comparisons
* pre-releases only match requirements naming one, such as `1.0.0-alpha`

The build fails when some setting, from either source, matches none of its packages: all of those are then listed. This is checked once `cargo` ran, against `Cargo.lock`. When installing, it is checked against what got compiled, and only when that succeeded: the binary is then installed already.

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_DEPS_NETWORK="openssl-sys;zipsign-api@0.1.5;something_else@0, >=0.11"
```

