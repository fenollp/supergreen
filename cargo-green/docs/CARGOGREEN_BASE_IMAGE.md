Sets the base Rust image, as an image URL (or any build context, actually).

If needing additional envs to be passed to rustc or build script, set them in the base image.

This can be done in that same config file with `base-image-inline`.

See also:
* `also-run`
* `base-image-inline`
* `additional-build-arguments`

For remote builds: make sure this is accessible non-locally.

```toml
base-image = "docker-image://docker.io/library/rust:1-slim"
```

The value must start with `docker-image://` and image must be available on the `$DOCKER_HOST`, eg:
```shell
export CARGOGREEN_BASE_IMAGE=docker-image://rustc_with_libs
DOCKER_HOST=ssh://my-remote-builder docker buildx build -t rustc_with_libs - <<EOF
FROM docker.io/library/rust:1.69.0-slim-bookworm@sha256:8bdd28ef184d85c9b4932586af6280732780e806e5f452065420f2e783323ca3
RUN set -eux && apt update && apt install -y libpq-dev libssl3
ENV KEY=value
EOF
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/rust:1-slim"
```

