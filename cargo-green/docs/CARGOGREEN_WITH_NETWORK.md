Controls runner's `--network none (default) | default | host` setting.

Set this to `"default"` if e.g. your additional stages to the base image call `curl` or `wget` or install any packages.

If adding system packages via `add`, this gets automatically set to `"default"`.

It turns out `--network` is part of BuildKit's cache key, so an initial online build won't cache-hit on later offline builds.

Set to `none` when in `$CARGO_NET_OFFLINE` mode. See
  * <https://doc.rust-lang.org/cargo/reference/config.html#netoffline>
  * <https://github.com/rust-lang/rustup/issues/4289>

```toml
[package.metadata.green]
with-network = "none"
base-image = "docker-image://docker.io/library/ubuntu@sha256:c4a8d5503dfb2a3eb8ab5f807da5bc69a85730fb49b5cfca2330194ebcc41c7b"
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_WITH_NETWORK="none"
```

