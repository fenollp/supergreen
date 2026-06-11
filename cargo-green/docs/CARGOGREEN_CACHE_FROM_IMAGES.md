Read cached data from image registries

See also [Cache::images] and [Cache::to_images].

```toml
cache-from-images = [ "docker-image://my.org/team/my-project-in-ci", "docker-image://some.org/global/cache" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_CACHE_FROM_IMAGES="docker-image://my.org/team/my-project-in-ci,docker-image://some.org/global/cache"
```

