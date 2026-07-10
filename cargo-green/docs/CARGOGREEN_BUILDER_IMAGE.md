Sets which BuildKit builder version to use.

See <https://docs.docker.com/build/builders/>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:latest"
```

