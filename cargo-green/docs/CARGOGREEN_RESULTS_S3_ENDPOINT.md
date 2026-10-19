S3-compatible API endpoint that fresh build results get published to, with AWS Signature Version 4.
For Cloudflare R2: `https://<account-id>.r2.cloudflarestorage.com`.

Publishing happens when this, `$CARGOGREEN_RESULTS_S3_BUCKET`, `$CARGOGREEN_RESULTS_S3_ACCESS_KEY_ID` and `$CARGOGREEN_RESULTS_S3_SECRET_ACCESS_KEY` are all set (setting only some of them is an error).
It is best effort: failures only get logged.

Results of local code (as opposed to crates.io or git dependencies) never get published.
Other results may still embed values from your environment (e.g. through `$CARGOGREEN_SET_ENVS`): they are as public as the bucket's read side.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_S3_ENDPOINT="https://<account-id>.r2.cloudflarestorage.com"
```

