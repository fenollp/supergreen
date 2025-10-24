Sets which BuildKit builder to use, through `$BUILDX_BUILDER`.

See <https://docs.docker.com/build/building/variables/#buildx_builder>

* Unset: creates & handles a builder named `"supergreen"`. Upgrades it if too old, while trying to keep old cached data
* Set to `""`: skips using a builder
* Set to `"supergreen"`: uses existing and just warns if too old
* Set: use that as builder, no questions asked

