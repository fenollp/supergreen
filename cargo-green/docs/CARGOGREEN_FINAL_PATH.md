Write final containerfile to given path.

Helps e.g. create a containerfile of e.g. a binary to use for best caching of dependencies.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_FINAL_PATH="$PWD/my-bin@1.0.0.Dockerfile"
```

