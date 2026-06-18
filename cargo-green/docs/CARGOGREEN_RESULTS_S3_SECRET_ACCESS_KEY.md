Secret access key for the S3 endpoint used when publishing. A secret: keep it out of `Cargo.toml`;
`cargo green supergreen env` only reports whether it is set, never its value.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY="…"
```

