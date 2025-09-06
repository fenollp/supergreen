Controls runner's `--network none (default) | default | host` setting.

Set this to `"default"` if e.g. your `base-image-inline` calls curl or wget or installs some packages.

Set to `none` when in `$CARGO_NET_OFFLINE` mode. See
  * <https://doc.rust-lang.org/cargo/reference/config.html#netoffline>
  * <https://github.com/rust-lang/rustup/issues/4289>

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_WITH_NETWORK="none"
```

