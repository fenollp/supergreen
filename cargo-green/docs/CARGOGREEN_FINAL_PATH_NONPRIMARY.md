Write final containerfile on every rustc call.

Helps e.g. debug builds failing too early.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_FINAL_PATH_NONPRIMARY="1"
```

