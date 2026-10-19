Base URL of a read-only store of build results, e.g. an R2 bucket's public custom domain.

Without a runner (`$CARGOGREEN_RUNNER=none`), a result missing from the local cache is then looked for at `$CARGOGREEN_RESULTS_BASE_URL/<stage>.tar.zst`.
It is kept only when it matches its `<stage>.tar.zst.sha256` sidecar. Any remote failure counts as a miss.

**Results contain executables (build scripts, proc-macros) that get run: only use a store you trust.**

Unset by default. Nothing gets fetched nor published when offline (`$CARGO_NET_OFFLINE=1` or `$CARGOGREEN_WITH_NETWORK=none`).

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_RESULTS_BASE_URL="https://results.example.com"
```

