Bucket that build results (and their `.sha256` sidecars) get `PUT` into. See `$CARGOGREEN_RESULTS_S3_ENDPOINT`.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_S3_BUCKET="my-results"
```

