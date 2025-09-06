Pick which executor to use: `"docker"` (default), `"podman"` or `"none"`.

The [runner gets forwarded these environment variables](https://docs.docker.com/engine/reference/commandline/cli/#environment-variables):
* `$BUILDKIT_COLORS`
* [`$BUILDKIT_HOST`](https://docs.docker.com/build/building/variables/#buildkit_host)
* `$BUILDKIT_PROGRESS`
* `$BUILDKIT_TTY_LOG_LINES`
* `$BUILDX_BAKE_GIT_AUTH_HEADER`
* `$BUILDX_BAKE_GIT_AUTH_TOKEN`
* `$BUILDX_BAKE_GIT_SSH`
* [`$BUILDX_BUILDER`](https://docs.docker.com/build/building/variables/#buildx_builder)
* [`$DOCKER_BUILDKIT`](https://docs.docker.com/build/buildkit/#getting-started)
* `$BUILDX_CONFIG`
* `$BUILDX_CPU_PROFILE`
* `$BUILDX_EXPERIMENTAL`
* `$BUILDX_GIT_CHECK_DIRTY`
* `$BUILDX_GIT_INFO`
* `$BUILDX_GIT_LABELS`
* `$BUILDX_MEM_PROFILE`
* `$BUILDX_METADATA_PROVENANCE`
* `$BUILDX_METADATA_WARNINGS`
* `$BUILDX_NO_DEFAULT_ATTESTATIONS`
* `$BUILDX_NO_DEFAULT_LOAD`
* `$DOCKER_API_VERSION`
* `$DOCKER_CERT_PATH`
* `$DOCKER_CONFIG`
* `$DOCKER_CONTENT_TRUST`
* `$DOCKER_CONTENT_TRUST_SERVER`
* [`$DOCKER_CONTEXT`](https://docs.docker.com/reference/cli/docker/#environment-variables)
* `$DOCKER_DEFAULT_PLATFORM`
* `$DOCKER_HIDE_LEGACY_COMMANDS`
* [`$DOCKER_HOST`](https://docs.docker.com/engine/security/protect-access/)
* `$DOCKER_TLS`
* `$DOCKER_TLS_VERIFY`
* `$EXPERIMENTAL_BUILDKIT_SOURCE_POLICY`
* `$HTTP_PROXY`
* `$HTTPS_PROXY`
* `$NO_PROXY`

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RUNNER="docker"
```

