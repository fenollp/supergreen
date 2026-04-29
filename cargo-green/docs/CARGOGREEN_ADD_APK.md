Adds OS packages to the base image with `apk add`.

See also:
* `add.apt`
* `add.apt-get`
* `base-image`

```toml
add.apk = [ "libpq-dev", "pkgconf" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APK="libpq-dev,pkg-conf"

# Inspect the resulting base stage with:
cargo green supergreen show-rust-base 2>/dev/null
```

