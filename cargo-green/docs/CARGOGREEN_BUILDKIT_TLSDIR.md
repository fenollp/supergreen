TLS settings to reach a `$BUILDKIT_HOST=tcp://…` daemon: a directory with `ca.pem` (and maybe `cert.pem` & `key.pem`), or `ca.crt` (and maybe `tls.crt` & `tls.key`). Read like `buildctl --tlsdir`.

The daemon's name to verify is the host in `$BUILDKIT_HOST`. Setting this without `$BUILDKIT_HOST=tcp://…` is an error.

See <https://docs.docker.com/build/builders/drivers/remote/>

*Use by setting this environment variable (no `Cargo.toml` setting):*
```shell
export CARGOGREEN_BUILDKIT_TLSDIR="$HOME/.config/buildkit/certs"
```

