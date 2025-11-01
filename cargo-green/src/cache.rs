use serde::{Deserialize, Serialize};

use crate::image_uri::ImageUri;

// TODO: conf for github actions caching
// CARGOGREEN_CACHE_FROM: type=gha
// CARGOGREEN_CACHE_TO: type=gha,mode=max
//=> https://docs.docker.com/build/cache/backends/gha/
// need setup (get vars through an action for urls,tokens) + setting
// * scope=extrafn
// * mode=max
// * ignore-error to same as cache-images
// * timeout to something low (given small layers & running on GH runners)

// TODO: conf for local/file caching
// https://docs.docker.com/build/cache/backends/local/
// >If the src cache doesn't exist, then the cache import step will fail,
// https://github.com/moby/buildkit/issues/1896
// >only grows => cache dance
// also
// >doesn't support concurrent use!
// ==> HAVE to handle per-buildcall type=local cache
//
// see about merging type=local caches /+ concurrent use.
//
// trash cch cch-new/ ; mkdir -p cch cch-new ; CARGOGREEN_CACHE_FROM='type=local,src='$PWD'/cch;type=local,src='$PWD'/cch-new' CARGOGREEN_CACHE_TO=type=local,mode=max,dest=$PWD/cch-new jobs=1 rmrf=1 ./hack/clis.sh vix ; du cch*
//
// path/to/<hashed pwd + cmd | or just cmd if cinstall>-<datetime>/cache-<extrafn>(-new)?
// then rm old + mv -new .
//
// mkdir -p cch cch-new
// CARGOGREEN_CACHE_FROM=type=local,src=/tmp/local-cache CARGOGREEN_CACHE_TO=type=local,mode=max,dest=/tmp/local-cache-new rmrf=1 ./hack/clis.sh vix

// FIXME: [allow exporting cache layers in parallel to the remote registry](https://github.com/moby/buildkit/issues/6123)
// * export these layers in parallel
// * use manifest retrieved in --cache-from to filter out layers which we know remote registry already has
//
//=> leaf build calls: don't use --cache-to, then
//=> cargo call#0: runs a (gRPC?) server that receives just-made-build-call and re-runs it with cacheonly and --cache-to
//then wait for both leafbuild and that server's tasks before exiting.
//==> server needs a task counter

// or: --cache-to local registry during build, then push that to ghcr after.

// or: use --cache-from in non-main branch and both otherwise

// # TODO: along with bin build job
// # * get local registry:3 from cache
// # * create second local registry:3
// # * list images required via green supergreen envs
// # * pull images to second registry using first as cache
// # * destroy first and cache second
// # * pass second through artifacts or cache to next jobs
// # * use that registry through the mirrors env

// #setup 2 registries
// #cachefrom: restore 1 reg from cache, use it with cachefrom
// #cacheto: norestore, build, swap with cachefrom, cache.

macro_rules! ENV_CACHE_FROM_IMAGES {
    () => {
        "CARGOGREEN_CACHE_FROM_IMAGES"
    };
}

macro_rules! ENV_CACHE_IMAGES {
    () => {
        "CARGOGREEN_CACHE_IMAGES"
    };
}

macro_rules! ENV_CACHE_TO_IMAGES {
    () => {
        "CARGOGREEN_CACHE_TO_IMAGES"
    };
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cache {
    #[doc = include_str!(concat!("../docs/",ENV_CACHE_FROM_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-from-images")]
    pub(crate) from_images: Vec<ImageUri>,

    #[doc = include_str!(concat!("../docs/",ENV_CACHE_TO_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-to-images")]
    pub(crate) to_images: Vec<ImageUri>,

    #[doc = include_str!(concat!("../docs/",ENV_CACHE_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-images")]
    pub(crate) images: Vec<ImageUri>,
}
