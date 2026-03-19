Tells `rustup` to add the given components to the base image.

See <https://rust-lang.github.io/rustup/concepts/components.html>

```toml
[package.metadata.green]
components = [ "rust-src", "llvm-tools-preview" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_COMPONENTS="rust-src,llvm-tools-preview"
```

