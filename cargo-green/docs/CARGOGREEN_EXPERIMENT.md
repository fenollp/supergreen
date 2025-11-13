A comma-separated list of names of features to activate.

A name that does not match exactly is an error.

* `finalpathnonprimary`:
  - Write final containerfile on every rustc call.
  - Helps e.g. debug builds failing too early.

* `incremental`:
  - Also wrap incremental compilation.
  - See <https://doc.rust-lang.org/cargo/reference/config.html#buildincremental>

* `repro`:
  - Try and test for builds hermeticity and reproducibility.
  - See <https://docs.docker.com/reference/cli/docker/buildx/build/#no-cache-filter>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_EXPERIMENT="finalpathnonprimary,repro"
```

