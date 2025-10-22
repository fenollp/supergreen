Adds OS packages to the base image with `apt install`, serialized as CSV.

```toml
add.apt = [ "libpq-dev", "pkg-config" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
CARGOGREEN_ADD_APT="libpq-dev,pkg-config"
```
