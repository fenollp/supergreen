Reads envs (show values with `cargo green supergreen env`)
* `$CARGOGREEN_FINAL_PATH`: if set, this file will end up with the final Dockerfile that reproduces the build
* `$CARGOGREEN_LOG_PATH`: logfile path
* `$CARGOGREEN_LOG_STYLE`
* `$CARGOGREEN_LOG`: equivalent to `$RUST_LOG` (and doesn't conflict with `cargo`'s)
* `$CARGOGREEN_SYNTAX_IMAGE`: use a [`dockerfile:1`](https://hub.docker.com/r/docker/dockerfile)-derived BuildKit frontend

Also [passes these envs through to the runner](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables):
* `BUILDKIT_PROGRESS`
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* `DOCKER_API_VERSION`
* `DOCKER_CERT_PATH`
* `DOCKER_CONFIG`
* `DOCKER_CONTENT_TRUST_SERVER`
* `DOCKER_CONTENT_TRUST`
* `DOCKER_CONTEXT`
* `DOCKER_DEFAULT_PLATFORM`
* `DOCKER_HIDE_LEGACY_COMMANDS`
* [`$DOCKER_HOST`](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables)
* `DOCKER_TLS_VERIFY`
* `DOCKER_TLS`
* `HTTP_PROXY`
* `HTTPS_PROXY`
* `NO_PROXY`

Sets
* [`$RUSTC_WRAPPER`](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)
* `$CARGOGREEN=1`
