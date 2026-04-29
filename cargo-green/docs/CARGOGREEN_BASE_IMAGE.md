Sets the base as an image URL, with scheme: `docker-image://`.

On top of this, [rustup](https://rustup.rs/) installs the toolchain.
This toolchain is picked by your local Rust installation or through `cargo +toolchain ..`.

If needing additional envs to be passed to rustc or build script, set them in the base image.

See also:
* `components`
* `also-run`
* `additional-build-arguments`

```toml
base-image = "docker-image://docker.io/library/debian:trixie-slim"
```

For remote builds, make sure this image is accessible non-locally.
```shell
export CARGOGREEN_BASE_IMAGE=docker-image://my_ubuntu_with_libs_and_envs
DOCKER_HOST=ssh://my-remote-builder docker buildx build -t my_ubuntu_with_libs_and_envs - <<EOF
FROM ubuntu:latest@sha256:c4a8d5503dfb2a3eb8ab5f807da5bc69a85730fb49b5cfca2330194ebcc41c7b
RUN set -eux && apt update && apt install -y libpq-dev libssl3
ENV KEY=value
EOF
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
export CARGOGREEN_BASE_IMAGE="docker-image://docker.io/library/debian:trixie-slim"
```

