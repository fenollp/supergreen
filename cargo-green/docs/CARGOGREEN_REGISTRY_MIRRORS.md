Registry mirrors for Docker Hub (docker.io). Defaults to GCP & AWS mirrors.

See <https://docs.docker.com/build/buildkit/configure/#registry-mirror>

Namely hosts with maybe a port and a path:
* `dockerhub.timeweb.cloud`
* `dockerhub1.beget.com`
* `localhost:5000`
* `mirror.gcr.io`
* `public.ecr.aws/docker`

```toml
registry-mirrors = [ "mirror.gcr.io", "public.ecr.aws/docker" ]
```

*This environment variable takes precedence over any `Cargo.toml` settings:*
```shell
# Note: values here are comma-separated.
export CARGOGREEN_REGISTRY_MIRRORS="mirror.gcr.io,public.ecr.aws/docker"
```

