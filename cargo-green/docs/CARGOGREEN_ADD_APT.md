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

# Inspect the resulting base stage with:
cargo green supergreen show-rust-base 2>/dev/null
```

