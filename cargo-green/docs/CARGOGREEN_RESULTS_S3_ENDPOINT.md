S3-compatible endpoint used to *publish* build results, signed with AWS Signature Version 4 — no
proxy or Worker needed. For Cloudflare R2 this is `https://<account-id>.r2.cloudflarestorage.com`.

Publishing happens only when this and the other `$CARGOGREEN_RESULTS_S3_*` variables are all set.

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_S3_ENDPOINT="https://<account-id>.r2.cloudflarestorage.com"
```

