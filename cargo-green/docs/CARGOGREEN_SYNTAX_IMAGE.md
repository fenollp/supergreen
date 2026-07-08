Sets which BuildKit frontend syntax to use.

See <https://docs.docker.com/build/buildkit/frontend/#stable-channel>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_SYNTAX_IMAGE="docker-image://docker.io/docker/dockerfile:1"
```

Left at its default, the value must resolve to a digest of the stable
`docker/dockerfile` frontend. Set explicitly, it is trusted and used verbatim, so a
custom or locally-built frontend (e.g. from a private registry) is allowed:
```shell
export CARGOGREEN_SYNTAX_IMAGE="docker-image://localhost:5000/docker/dockerfile@sha256:…"
```

