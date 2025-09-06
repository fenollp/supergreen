Both read and write cached data to and from image registries

Exactly a combination of [Green::cache_from_images] and [Green::cache_to_images].

See
* `type=registry` at <https://docs.docker.com/build/cache/backends/>
* and <https://docs.docker.com/build/cache/backends/registry/>

```toml
cache-images = [ "docker-image://my.org/team/my-project", "docker-image://some.org/global/cache" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_CACHE_IMAGES="docker-image://my.org/team/my-project,docker-image://some.org/global/cache"
```

