Sets the base Rust image for root package and all dependencies, unless themselves being configured differently.

See also:
* `with-network`
* `additional-build-arguments`

In order to avoid unexpected changes, you may want to pin the image using an immutable digest.

Note that carefully crafting crossplatform stages can be non-trivial.

```toml
base-image-inline = """
FROM --platform=$BUILDPLATFORM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
```

```toml
# This must also be set so digest gets pinned automatically.
base-image = "docker-image://rust:1"
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
IFS='' read -r -d '' CARGOGREEN_BASE_IMAGE_INLINE <<"EOF"
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
EOF
echo "$CARGOGREEN_BASE_IMAGE_INLINE" # (with quotes to preserve newlines)
export CARGOGREEN_BASE_IMAGE_INLINE
export CARGOGREEN_BASE_IMAGE=docker-image://rust:1
```

