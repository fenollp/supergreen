Sets which BuildKit builder to use, through `$BUILDX_BUILDER`.

See <https://docs.docker.com/build/building/variables/#buildx_builder>

* Unset, or set to `"supergreen"`: uses the *managed* builder, named `"supergreen"`
* Set to `""`: does not pick a builder, so `docker buildx` uses its current one (see `docker buildx use`)
* Set to another name: uses that existing builder, as is (it is an error if it does not exist)

The *managed* builder is the one `cargo green` takes care of:
* it gets created when missing, using the [`docker-container` driver](https://docs.docker.com/build/builders/drivers/docker-container/) and `$CARGOGREEN_BUILDER_IMAGE` (defaults to the latest `moby/buildkit` image)
* it gets re-created when it does not run `$CARGOGREEN_BUILDER_IMAGE`, or when it runs a BuildKit version older than the latest release `cargo green` knows of. Re-creating first tries to keep the builder's state (its build cache): `docker buildx rm --keep-state`

Where the managed builder runs:
* With `$BUILDKIT_HOST` (e.g. `ssh://host`, `unix:///run/buildkit/buildkitd.sock`): it uses that BuildKit daemon through a [`remote` driver](https://docs.docker.com/build/builders/drivers/remote/) builder, named `"supergreen-buildkit-<host>"`. `$CARGOGREEN_BUILDER_IMAGE` cannot apply there.
* With `$DOCKER_HOST` (e.g. `ssh://host`, `unix:///run/user/1000/docker.sock`): it runs on that daemon, named after its address (e.g. `"supergreen-host"`), except for the default `unix:///var/run/docker.sock`
* Otherwise: it runs on the local daemon, named `"supergreen"`

With `ssh://` hosts, connections are pooled: only a few SSH handshakes for the whole build.

Other builders never get modified: when one does not run `$CARGOGREEN_BUILDER_IMAGE` it is an error, and when its BuildKit is outdated a warning shows how to re-create it.

Setting `$CARGOGREEN_BUILDER_IMAGE` along with `$BUILDX_BUILDER=""` is an error, as is setting both `$BUILDKIT_HOST` and a non-empty `$BUILDX_BUILDER`, or both `$BUILDKIT_HOST` and `$DOCKER_HOST` (`remote` builders do not use `$DOCKER_HOST`).

See also
* `$DOCKER_HOST`: <https://docs.docker.com/engine/reference/commandline/cli/#environment-variables>
* `$DOCKER_CONTEXT`: <https://docs.docker.com/reference/cli/docker/#environment-variables>
* `$BUILDKIT_HOST`: <https://docs.docker.com/build/building/variables/#buildkit_host>

