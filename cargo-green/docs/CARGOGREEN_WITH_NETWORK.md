Controls runner's `--network none (default) | default | host` setting.

Set this to `"default"` if e.g. your `base-image-inline` calls `curl` or `wget` or installs some packages.

If adding system packages with `add`, this gets automatically set to `"default"`.

It turns out `--network` is part of BuildKit's cache key, so an initial online build won't cache-hit on later offline builds.

Set to `none` when in `$CARGO_NET_OFFLINE` mode. See
  * <https://doc.rust-lang.org/cargo/reference/config.html#netoffline>
  * <https://github.com/rust-lang/rustup/issues/4289>

```toml
[package.metadata.green]
with-network = "none"
base-image = "docker-image://docker.io/library/rust:1@sha256:72724f1a416c449b405a2b7ed6bac56058163e6dfb1b5ccb40839882141dd237"
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_WITH_NETWORK="none"
```

