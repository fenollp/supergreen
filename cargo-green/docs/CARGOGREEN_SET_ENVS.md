Pass environment variables through to build runner.

May be useful if a build script exported some vars that a package then reads.
See also:
* `packages`

See about `$GIT_AUTH_TOKEN`: <https://docs.docker.com/build/building/secrets/#git-authentication-for-remote-contexts>

NOTE: this doesn't (yet) accumulate dependencies' set-envs values!
Meaning only the top-level crate's setting is used, for all crates/dependencies.

```toml
set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_SET_ENVS="GIT_AUTH_TOKEN,TYPENUM_BUILD_CONSTS,TYPENUM_BUILD_OP"
```

