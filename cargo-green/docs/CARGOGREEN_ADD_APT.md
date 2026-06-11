Adds OS packages to the base image with `apt install`.

Supported syntax:
* `libssl-dev`
* `libssl-dev(>=3.5)`
* `libssl-dev(=3.5.5-1~deb13u2)` *is the encouraged usage for better build cache hits*

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

