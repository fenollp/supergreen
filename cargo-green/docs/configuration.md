Tune the behavior of `cargo-green` either through environment variables or via the package's `green` metadata section.

```toml
[package]
name = "my-crate"
# ...

[package.metadata.green]
registry-mirrors = [ "mirror.gcr.io", "public.ecr.aws/docker" ] # Default values
```

Environment variables that are prefixed with `$CARGOGREEN_` override TOML settings.

```shell
export CARGOGREEN_REGISTRY_MIRRORS=mirror.gcr.io
cargo green build
```

