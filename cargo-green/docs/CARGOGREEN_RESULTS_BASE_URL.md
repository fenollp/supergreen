Base URL of a read-only, public store of build results (crate artifact tarballs), tried on a
local-disk cache miss before falling back to building. Defaults to `https://results.cargo.green`.

Downloads are streamed and verified against a published `{name}.tar.gz.sha256` sidecar when present.
Set to the empty string to disable remote fetching. Skipped entirely when offline (`--offline`/`--frozen`).

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_BASE_URL="https://results.cargo.green"
```

