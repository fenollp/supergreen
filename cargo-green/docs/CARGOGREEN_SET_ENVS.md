Pass environment variables through to build runner.

May be useful if a build script exported some vars that a package then reads.
See also:
* `packages`

See about `$GIT_AUTH_TOKEN`: <https://docs.docker.com/build/building/secrets/#git-authentication-for-remote-contexts>

NOTE: this doesn't (yet) accumulate dependencies' set-envs values!
Meaning only the top-level crate's setting is used, for all crates/dependencies.

Listed variables are set for every crate built, dependencies included (not just the primary package).
Each one that is set is passed as is, even when empty. Unset ones are not passed.
FIXME: find a more fine-grained way to set these to save on caching!

WARNING: values are written as-is into the generated Containerfiles, so they end up wherever these go:
`$CARGOGREEN_FINAL_PATH`, BuildKit's build history & cache, and any cache image pushed to a registry.
They are also part of cache keys: a changing value rebuilds every crate.
Do not pass secrets this way. (Logs only show the first few characters of values not known to be harmless.)

```toml
set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_SET_ENVS="GIT_AUTH_TOKEN,TYPENUM_BUILD_CONSTS,TYPENUM_BUILD_OP"
```

