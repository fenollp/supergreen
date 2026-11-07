A comma-separated list of names of features to activate.

A name that does not match exactly is ignored, with a warning.

* `bitrepro`:
  - Make BuildKit's build cache bit-reproducible (as far as Containerfiles go):
    RUNs `touch` what they wrote to `$SOURCE_DATE_EPOCH`, and `rm` logs & caches.
  - When disabled: rely on cache keys being made of contents (not mtimes) & on
    the TAR exporter rewriting results' timestamps.
  - Results are bit-reproducible either way.

* `buildscriptsources`:
  - Mounts build scripts' dependencies' sources so build scripts that
    read a dependency's bundled files at execution time may find them
    (eg. <https://lib.rs/crates/protoc-bin-vendored> ships `protoc` binaries).

* `cachebuildkit`:
  - Write BuildKit build cache to disk
  - Reuse it from disk

* `finalpathcomments`:
  - Write final containerfile on every rustc call.
  - Contains internal debugging structs: as commented TOML
  - Helps e.g. debug builds failing too early.

* `finalpathnonprimary`:
  - Write final containerfile on every rustc call.
  - Perfect format to share the file.
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

