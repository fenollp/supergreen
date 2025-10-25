Adds OS packages to the base image with `apt-get install`.

See also:
* `add.apk`
* `add.apt`
* `base-image`

```toml
add.apt-get = [ "libpq-dev", "pkg-config" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_ADD_APT_GET="libpq-dev,pkg-config"

# Inspect the resulting base image with:
cargo green supergreen env CARGOGREEN_BASE_IMAGE_INLINE
```

