Sets which BuildKit builder version to use.

See <https://docs.docker.com/build/builders/>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:latest"
```

Non-Docker-Hub images are used verbatim (no digest is fetched), so a custom or
locally-built builder from a private registry is allowed. When it lives on a local
registry (`localhost`/`127.0.0.1`), the managed builder is wired with host networking
and an insecure-registry entry so it can pull it:
```shell
export CARGOGREEN_BUILDER_IMAGE="docker-image://localhost:5000/moby/buildkit@sha256:…"
```

