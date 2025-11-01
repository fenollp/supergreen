Filter logs. Equivalent to `$RUST_LOG` (and doesn't conflict with `cargo`'s).

By default, writes logs under [`/tmp`](https://doc.rust-lang.org/stable/std/env/fn.temp_dir.html).

See <https://docs.rs/env_logger/#enabling-logging>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_LOG="trace,cargo_green::build=info"
```

