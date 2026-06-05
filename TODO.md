# TODO

Braindump

---

https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/hint-mostly-unused.html
how to cache with this tho

---

moby/ubildkit tags
https://github.com/moby/buildkit/pull/5944#issue-3015470909
https://hub.docker.com/r/moby/buildkit

---

```
CARGOGREEN_LOG=trace CARGOGREEN_LOG_PATH=_ CARGO_TARGET_DIR=green cargo green clippy --jobs=1 --locked --frozen --offline --all-targets -- --no-deps

130 43s ipam.git main λ rm -rf _ ./green ; cargo green fetch && CARGOGREEN_LOG=trace CARGOGREEN_LOG_PATH=_ CARGOGREEN_SET_ENVS=RING_CORE_PREFIX CARGO_TARGET_DIR=green  cargo green clippy --jobs=1 --locked --frozen --offline --all-targets -- --no-deps
```

```
     validator.git add-custom-returning-multiple-errs-0.16 λ cfmt && CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-target/clippy} cargo clippy --locked --frozen --offline --all-targets -- --no-deps  
rm -rf _ ./green ; cargo green fetch && CARGOGREEN_LOG=trace CARGOGREEN_LOG_PATH=_ CARGO_TARGET_DIR=green  cargo green clippy --locked --frozen --offline --all-targets -- --no-deps
```

---

> DOCKER_HOST=ssh://oomphy jobs=1 ./hack/clis.sh rublk clean

> ssh root@oomphy journalctl -fu docker

---

only transfers missing layers
https://news.ycombinator.com/item?id=44314085
https://github.com/psviderski/unregistry

registry 
https://github.com/dragonflyoss/nydus


https://depot.dev/dockerfile-explorer


https://oci.dag.dev/
 Registry Explorer

https://github.com/google/go-containerregistry

> docker run --rm gcr.io/go-containerregistry/crane ls rust

https://github.com/google/go-containerregistry/blob/main/cmd/crane/doc/crane.md

```
docker run --rm gcr.io/go-containerregistry/crane digest rust:1
sha256:52e36cdd822b813542e13e06a816953234ecad01ebae2d0d7ec4a084c7cda6bd
```


> "BUILDX_EXPERIMENTAL=1 docker buildx build <args> --invoke /bin/sh"
There's actually a whole debugging client available.
https://news.ycombinator.com/item?id=39277451

---

https://lib.rs/crates/tryhard

---

> --ulimit nofile=1024:1024

https://github.com/docker/buildx/issues/379#issuecomment-3114962233
https://github.com/cross-rs/cross/pull/1065/files

---

musl platform test https://github.com/fujiapple852/trippy/blob/7fd4098ae50dbc930fd3b0503f66801a127cafff/Dockerfile

---

> So how does one achieve a persistent ssh connection to a remote docker-container builder? A conn that stays alive in between multiple docker buildx build calls, without having to re-establish the con.

> If you wish for your ssh connection to not drop after buildx/docker exist then that is controlled by your SSH config.

> Usually smth like
```
ControlPath ~/.ssh/cm-%r@%h:%p
ControlMaster auto
```

First google link: cyberciti.biz/faq/linux-unix-reuse-openssh-connection

---

mulitplatform 
https://docs.docker.com/desktop/features/containerd/
containerd image store

The classic Docker image store is limited in the types of images that it supports. For example, it doesn't support image indices, containing manifest lists. When you create multi-platform images, for example, the image index resolves all the platform-specific variants of the image. An image index is also required when building images with attestations.

---

todo: build self + push image + multiplat (+ tag per branch?) + main as :latest

---

https://github.com/guidance-ai/llguidance/blob/94fa39128ef184ffeda33845f6d333f332a34b4d/parser/Cargo.toml#L38

---

https://github.com/awslabs/aws-sdk-rust/issues/113

---

https://github.com/moby/buildkit/issues/4854
    Inspect image manifest without pushing to registry or load to local docker daemon #4854

https://github.com/moby/buildkit/issues/1251
    Cache pushed from one machine can not be reused on another machine #1251

https://github.com/search?q=repo%3Amoby%2Fbuildkit+export+cache+reproducible&type=issues

https://github.com/moby/buildkit/issues/3009
    Reproducibility is broken when re-building the exact same image multiple times because sometimes the moby.buildkit.cache.v0 entry changes #3009

https://github.com/moby/buildkit/pull/4057
     exporter/containerimage: new option: rewrite-timestamp (Apply SOURCE_DATE_EPOCH to file timestamps) #4057 

---

gha validate workflow files

https://github.com/dorny/paths-filter#example

---

```
 cargo: show our shit then \\r\r\r that on success
same for "Calling ..."
=> see about ci logs tho
```
 https://crates.io/crates/tracing-indicatif
 https://github.com/emersonford/tracing-indicatif/blob/main/examples/build_console.rs

---

interact with https://lib.rs/crates/jobserver
esp. when building on remote machine(s)

---

-#((i+=1)); nvs[i]=kani-verifier@0.66.0;       oks[i]=ok; nvs_args[i]='--bin=cargo-kani --bin=kani'
- ((i+=1)); nvs[i]=kani-verifier@0.66.0;       oks[i]=ok; nvs_args[i]='--bin=cargo-kani'

non-determinism in generated Dockerfile
```
repro:
    onn gre; while gs -s recipes/kani-verifier@0.66.0.Dockerfile | grep -E '^M '; do rmrf=1 CARGOGREEN_EXPERIMENT=repro ./hack/clis.sh kani ; sleep 1; done
potentially:
    2 possible bins (cargo-kani + kani) => both picked
        *=> check final COPY (no mdid) is correct!
        *=> check how to sort that (=> cinstall --bins a,b)
        ==> final scratch stage MUST contain all --bins!
```

https://github.com/model-checking/kani/blob/727135d50cf1577612d3f8207c8e58fbc0d47693/Cargo.toml#L24-L30

```
error: unexpected value 'cargo-kani,kani' for '--bins' found; no more were expected
Usage: cargo install --timings[=<FMTS>] --root <DIR> --locked --force --bins [CRATE[@<VER>]]...
```
=> attempt sort on finalpath generation

---

https://github.com/moby/moby/issues/12843
Global .dockerignore #12843

https://github.com/moby/moby/issues/40319
[epic] builder: collected issues on improving .dockerignore #40319

=> ask for passing dockerignore file / strings as cli arg, when --build-context=NAME=DIRPATH, to not have to write to DIRPATH

---

https://lib.rs/crates/lychee
https://lib.rs/crates/redbpf
https://lib.rs/crates/s3m
https://lib.rs/crates/cargo-resources
https://lib.rs/crates/rustup-mirror
https://lib.rs/crates/aati
https://crates.io/crates/voila
https://crates.io/crates/slugid
https://lib.rs/crates/rmd
https://lib.rs/crates/cratery
https://lib.rs/crates/hfile
https://lib.rs/crates/gauth
https://lib.rs/crates/muid
https://lib.rs/crates/pbcli
https://lib.rs/crates/yayo
https://lib.rs/crates/duplicate-checker
https://lib.rs/crates/mediafire_rs
https://lib.rs/crates/pw

https://crates.io/crates/cargo-lambda
https://crates.io/crates/meilisearch-importer
https://github.com/wezterm/wezterm
https://crates.io/crates/cargo-wdk
https://github.com/AeneasVerif/charon/
https://github.com/asaaki/wargo

---

https://github.com/moby/buildkit/issues/5340
    Load metadata even when the image is locally available #5340

```
[worker.oci]
enabled = false

[worker.containerd]
enabled = true
namespace = "default"
```

---

https://github.com/moby/buildkit/issues/2120
    cache-from and COPY invalidates all layers instead of only the ones after COPY #2120

---

https://github.com/probe-rs/probe-rs

---

https://github.com/uutils/coreutils/releases/tag/0.5.0

https://github.com/Gnurou/awer


https://github.com/google/crosvm

---

if any issue with submodules
    peek at https://github.com/rust-lang/cargo/pull/16246/files
    after 1.92

---

https://github.com/nix-community/trustix
 Trustix: Distributed trust and reproducibility tracking for binary caches [maintainer=@adisbladis] 

---

CARGOGREEN_LOG_PATH=- CARGOGREEN_LOG=debug

write logs with eprintln

https://stackoverflow.com/a/73734760/1418165

---

--target armv7-unknown-linux-musleabihf

https://github.com/fenollp/reMarkable-tools/blob/master/Makefile#L82

https://github.com/cross-rs/cross/pkgs/container/armv7-unknown-linux-musleabihf/68145882?tag=0.2.5

https://github.com/cross-rs/cross/blob/v0.2.5/docker/Dockerfile.armv7-unknown-linux-musleabihf


https://github.com/cross-rs/cross/wiki/Contributing#how-cross-works

---

buildctl runner

https://github.com/denzp/rust-buildkit
https://github.com/cicadahq/buildkit-rs

https://users.rust-lang.org/t/is-it-possible-to-incorporate-one-executable-program-into-your-rust-code/58854/21 ?

---

```
1 20s supergreen.git toolz 🔗 crun green supergreen env

   Compiling pico-args v0.5.0
   Compiling nutype v0.6.2
   Compiling cargo-green v0.22.0 (/Users/pierre/wefwefwef/supergreen.git/cargo-green)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 18.74s
     Running `target/debug/cargo-green green supergreen env`
Using runner /usr/local/bin/docker
Calling DOCKER_BUILDKIT="1" /usr/local/bin/docker buildx ls --format=json
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/local/bin/docker buildx du --verbose --filter=type=regular --filter=description~=pulled.from
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/local/bin/docker inspect --format={{index .RepoDigests 0}} docker.io/docker/dockerfile:1
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/local/bin/docker inspect --format={{index .RepoDigests 0}} docker.io/library/rust:1.92.0-slim
GETing https://registry.hub.docker.com/v2/repositories/library/rust/tags/1.92.0-slim
Error: Failed getting digest for docker-image://docker.io/library/rust:1.92.0-slim: Failed to decode response from registry: missing field `digest` at line 1 column 130
{"message":"httperror 404: tag '1.92.0-slim' not found","errinfo":{"namespace":"library","repository":"rust","tag":"1.92.0-slim"}}
```

TODO: handle older digest format
    docker.io/library/rust:1.44.0-slim

```
info: syncing channel updates for 1.44.0-x86_64-unknown-linux-gnu
info: latest update on 2020-06-04 for version 1.44.0 (49cae5576 2020-06-01)
info: downloading 6 components
$BUILDX_BUILDER is set to "supergreen"
Using runner /usr/bin/docker
Calling DOCKER_BUILDKIT="1" /usr/bin/docker buildx ls --format=json
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/bin/docker buildx du --verbose --filter=type=regular --filter=description~=pulled.from
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/bin/docker inspect --format={{index .RepoDigests 0}} docker.io/docker/dockerfile:1
GETing https://registry.hub.docker.com/v2/repositories/docker/dockerfile/tags/1
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/bin/docker inspect --format={{index .RepoDigests 0}} docker.io/library/rust:1.44.0-slim
GETing https://registry.hub.docker.com/v2/repositories/library/rust/tags/1.44.0-slim
Error: Failed getting digest for docker-image://docker.io/library/rust:1.44.0-slim: Failed to decode response from registry: missing field `digest` at line 1 column 1643
{"creator":1156886,"id":103577370,"images":[{"architecture":"amd64","features":"","variant":null,"digest":"sha256:71e1fcc6901b89f329c5cc5d8427d6cba5ef9c31750c5807018d9862571e2e40","os":"linux","os_features":"","os_version":null,"size":207268936,"status":"active","last_pulled":"2026-03-16T09:11:16.626819836Z","last_pushed":"2020-06-09T20:22:13Z"},{"architecture":"arm64","features":"","variant":"v8","digest":"sha256:5da86b1b6035bc3d7c307f8ef5d1e7c3f04642253fda21c8c547c208196322fb","os":"linux","os_features":"","os_version":null,"size":210411823,"status":"active","last_pulled":"2026-03-16T09:11:18.807793857Z","last_pushed":"2020-06-09T13:58:56Z"},{"architecture":"arm","features":"","variant":"v7","digest":"sha256:20a44b32ffcadefc668666152a4976e22e074e165e2c62a07353d54a1e6bd963","os":"linux","os_features":"","os_version":null,"size":203309061,"status":"active","last_pulled":"2026-03-16T09:11:17.692384945Z","last_pushed":"2020-06-09T14:35:01Z"},{"architecture":"386","features":"","variant":null,"digest":"sha256:cbc5f55f9af251daefb595e67fe7b531e9b12e61037ca6a3878ce6e29e612974","os":"linux","os_features":"","os_version":null,"size":228338913,"status":"active","last_pulled":"2026-03-16T09:11:19.611354789Z","last_pushed":"2020-06-09T16:42:52Z"}],"last_updated":"2020-06-09T20:45:50.400981Z","last_updater":1156886,"last_updater_username":"doijanky","name":"1.44.0-slim","repository":1726866,"full_size":0,"v2":true,"tag_status":"active","tag_last_pulled":"2026-03-16T09:11:19.611354789Z","tag_last_pushed":"2020-06-09T20:45:50.400981Z","media_type":"application/vnd.docker.distribution.manifest.list.v2+json","content_type":"image"}
```

```json
{"creator":1156886,"id":105496357,"images":[{"architecture":"arm64","features":"","variant":"v8","digest":"sha256:92e0980cc684e652780a18ff7c064d5ef48775c46bff3dc22cc2d0e246568104","os":"linux","os_features":"","os_version":null,"size":210995124,"status":"active","last_pulled":"2026-03-16T09:11:57.07229608Z","last_pushed":"2020-06-19T00:09:52Z"},{"architecture":"amd64","features":"","variant":null,"digest":"sha256:354b345047ce24c3b879e4427ab1662f608c6a0f6c6ac08ef22a971dfdc3056e","os":"linux","os_features":"","os_version":null,"size":207775593,"status":"active","last_pulled":"2026-03-17T03:03:28.263623736Z","last_pushed":"2020-06-18T19:25:05Z"},{"architecture":"arm","features":"","variant":"v7","digest":"sha256:aef5e2e31a2962e3bc789b16a201df05f3891c8b6351e479ca235de0130fe842","os":"linux","os_features":"","os_version":null,"size":203947983,"status":"active","last_pulled":"2026-03-16T09:11:51.910948279Z","last_pushed":"2020-06-18T19:20:36Z"},{"architecture":"386","features":"","variant":null,"digest":"sha256:6a71e80ff8fa21bb1a5acfee60e064f9a7d5632b257e4b8fee5d8a4dac7ca3d1","os":"linux","os_features":"","os_version":null,"size":228879583,"status":"active","last_pulled":"2026-03-16T09:11:57.563830963Z","last_pushed":"2020-06-18T20:14:55Z"}],"last_updated":"2020-06-19T02:45:26.871928Z","last_updater":1156886,"last_updater_username":"doijanky","name":"1.44.1-slim","repository":1726866,"full_size":0,"v2":true,"tag_status":"active","tag_last_pulled":"2026-03-17T03:03:28.263623736Z","tag_last_pushed":"2020-06-19T02:45:26.871928Z","media_type":"application/vnd.docker.distribution.manifest.list.v2+json","content_type":"image"}
```

rust docker hub client get digest
Drop custom code and use?
https://lib.rs/crates/dkregistry

```rust
use dkregistry::v2::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = "registry-1.docker.io";
    let image = "library/alpine";
    let tag = "latest";

    // 1. Configure the client for Docker Hub
    let client = Client::configure()
        .registry(host)
        .insecure_registry(false)
        .build()?;

    // 2. Authenticate (Docker Hub requires a token even for public images)
    let login_scope = format!("repository:{}:pull", image);
    let auth_client = client.authenticate(&[&login_scope]).await?;

    // 3. Fetch the manifest
    // The digest is often found in the response metadata or can be 
    // calculated from the body.
    let manifest = auth_client.get_manifest(image, tag).await?;
    
    // Most registry clients return the digest in the headers of the manifest response
    println!("Fetched manifest for {}/{}", image, tag);
    
    Ok(())
}
```


---

TODO: check cache-to works with output=cacheonly

```
>>> call:/usr/bin/docker build --cache-from=type=registry,ref=localhost:5000/fenollp/supergreen --cache-to=type=registry,ref=localhost:5000/fenollp/supergreen,mode=max,ignore-error=false --network=none --platform=local --pull=false --target=rust-base --output=type=cacheonly -
#0 building with "supergreen" instance using docker-container driver

#1 [internal] load build definition from Dockerfile
#1 transferring dockerfile: 382B done
#1 DONE 0.0s

#2 resolve image config for docker-image://docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
#2 DONE 0.9s

#3 docker-image://docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6
#3 resolve docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6 done
#3 sha256:77246a01651da592b7bae79e0e20ed3b4f2e4c00a1b54b7c921c91ae3fa9ef07 0B / 13.57MB 0.2s
#3 sha256:77246a01651da592b7bae79e0e20ed3b4f2e4c00a1b54b7c921c91ae3fa9ef07 11.53MB / 13.57MB 0.5s
#3 sha256:77246a01651da592b7bae79e0e20ed3b4f2e4c00a1b54b7c921c91ae3fa9ef07 13.57MB / 13.57MB 0.5s done
#3 extracting sha256:77246a01651da592b7bae79e0e20ed3b4f2e4c00a1b54b7c921c91ae3fa9ef07 0.1s done
#3 DONE 0.6s

#4 [internal] load metadata for docker.io/library/rust:1.90.0-slim@sha256:7fa728f3678acf5980d5db70960cf8491aff9411976789086676bdf0c19db39e
#4 DONE 0.7s

#5 [internal] load .dockerignore
#5 transferring context: 2B done
#5 DONE 0.0s

#6 importing cache manifest from localhost:5000/fenollp/supergreen
#6 inferred cache manifest type: application/vnd.docker.distribution.manifest.v2+json done
#6 DONE 0.0s

#7 [1/1] FROM docker.io/library/rust:1.90.0-slim@sha256:7fa728f3678acf5980d5db70960cf8491aff9411976789086676bdf0c19db39e
#7 resolve docker.io/library/rust:1.90.0-slim@sha256:7fa728f3678acf5980d5db70960cf8491aff9411976789086676bdf0c19db39e done
#7 DONE 0.0s

#8 exporting cache to registry
#8 skipping cache export for empty result done
#8 preparing build cache for export done
#8 DONE 0.0s
```

---

https://github.com/docker/buildx/issues/429
how should I know which node is selected ? #429

> It is the first node that supports the target platform. Nodes that define platforms manually on buildx create are preferred (signified by * in output).
> it always selects the first node named arm0 ?
> Yes. It is different for k8s driver, then there is a consistent hash computed per project and you can't pick the subnode.

---

add: alphasort pkgs before containerfile

also
add: should be per distro
* one cargotoml to support multiple distros
    * how to detect which distro is in use?
      => starlark + give it computed info on baseimage

apt apk :
* vars for both apt + apk: {apt=libwut-dev, apk=libwut}
* another var to say which tool  to use: apt

---

https://www.buildbuddy.io/pricing

https://abseil.io/resources/swe-book/html/ch18.html

https://www.docker.com/pricing/

https://depot.dev/pricing

https://runs-on.com/pricing/

https://www.ubicloud.com/docs/about/pricing

---

xx missing SHELL

also use <<EOR
=> simpler "also-run" string

---

https://github.com/fredrik-hammar/egui-android-demo

https://github.com/rust-mobile/xbuild

https://github.com/rust-lang/rustup/blob/2d80024a0fe21bd9f082d89f672a471ef638562e/ci/docker/android/Dockerfile

---

image-manifest=true

https://gitlab.com/gitlab-org/container-registry/-/issues/407
    Docker Buildkit uploading image manifest lists/indexes with invalid references

https://github.com/moby/buildkit/issues/2251
    Support remote cache based on OCI Image Manifest layout #2251

https://github.com/moby/buildkit/pull/3724
     Import/export support for OCI compatible image manifest version of cache manifest (opt-in on export, inferred on import) #3724 

https://github.com/moby/buildkit/issues/5864
    Make image-manifest=true default for cache export #5864

=> add it + note how it's default since april

---

To avoid that your image registry fills up with cache images, I generally recommend that you configure some kind of image retention policy in your container image registry, which automatically deletes cache-images, e.g. if they have not been pulled for a week or two.


```toml
cache-to-images = [ "docker-image://my.org/team/my-fork" ]
```
docker-image://my.org/team/my-project:cached-{branch}


tags allowed!

---

https://lib.rs/crates/crabz

---

https://github.com/stratis-storage/stratisd

---

https://git.deuxfleurs.fr/Deuxfleurs/garage

https://crates.io/crates/defaults-rs

---

https://www.reddit.com/r/rust/s/efgskGd2ag

https://www.reddit.com/r/rust/comments/1pre6pg/rustup_1290_beta_call_for_testing_inside_rust_blog/

---

```
cinstall rqbit --no-default-features -F default-tls,postgres
+cinstall rqbit --no-default-features -F default-tls,postgres --git https://github.com/ikatson/rqbit --rev 2f725e3
```

---

https://github.com/apache/iggy

https://arborium.bearcove.eu/#rust

---

https://github.com/Rust-GPU/rust-cuda
https://github.com/Rust-GPU/rust-gpu/tree/main/examples
https://github.com/arlyon/openfrust

---

```diff
diff --git a/cargo-green/src/experiments.rs b/cargo-green/src/experiments.rs
index ba9341e..a58f9e8 100644
--- a/cargo-green/src/experiments.rs
+++ b/cargo-green/src/experiments.rs
@@ -8,6 +8,7 @@ macro_rules! ENV_EXPERIMENT {

 pub(crate) const EXPERIMENTS: &[&str] = &[
     //
+    "depsnopruning",
     "finalpathnonprimary",
     "incremental",
     "repro",
@@ -22,6 +23,7 @@ macro_rules! experiment {
 }

 impl Green {
+    experiment!(depsnopruning);
     experiment!(finalpathnonprimary);
     experiment!(incremental);
     experiment!(repro);
```

---

```diff
diff --git a/cargo-green/src/builder.rs b/cargo-green/src/builder.rs
index b2312dd..05d1c7e 100644
--- a/cargo-green/src/builder.rs
+++ b/cargo-green/src/builder.rs
@@ -144,6 +144,8 @@ then run your cargo command again.
                 }
             }

+            //recreate if command isn't the same (ntoe: contains hashed configtoml)
+
             if recreate {
                 // First try keeping state...
                 if self.try_removing_builder(name, true).await.is_err() {
```

---

https://github.com/ariel-os/ariel-os

---

https://github.com/checkpoint-restore/criu-image-streamer

---

Distributed build system providing cryptographic proofs-of-reproducibility via Byzantine Fault Tolerant (BFT) consensus
https://github.com/iqlusioninc/synchronicity?tab=readme-ov-file

Deploy self-contained binaries from GCP Container Registry (gcr.io) as systemd service units
https://github.com/iqlusioninc/canister

Execute your code on the Rust ecosystem.
https://github.com/rust-lang/rustwide

---

https://github.com/rust-lang/cargo/issues/5931#issuecomment-3482870594
> some foundation crates bump versions a lot and projects are unlikely to be on a coordinated set of those packages

==> cache-aware deps locking
====> ask cache with a dep version range, get back most used / most hit version

---

https://github.com/uandere/semwave

---

I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #32 [run-z-anyhow-1.0.100-d45b6249b823f856 1/3] WORKDIR /target/release/build/anyhow-d45b6249b823f856/out
I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #32 CACHED
I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #33 [run-z-kani-verifier-0.66.0-5b332a90b563b71d 1/3] WORKDIR /target/release/build/kani-verifier-5b332a90b563b71d/out
I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #33 CACHED
I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #34 exporting to client tarball
I 26/02/18 12:57:00.113 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #34 sending tarball 0.0s done
I 26/02/18 12:57:00.167 N kani-verifier 0.66.0 9e144b88b270f21c ✖ #34 DONE 0.0s
I 26/02/18 12:57:00.175 N kani-verifier 0.66.0 9e144b88b270f21c Terminating task CACHED:23 DONE:11 {"context": " 2B", "dockerfile": " 33.56kB"}
T 26/02/18 12:57:00.175 N kani-verifier 0.66.0 9e144b88b270f21c deregistering event source from poller

==> acc "sending tarball 0.0s"

---

salti@0.5.1

---

https://github.com/sonos/tract

---

sudo apt install build-essential libasound2-dev libpulse-dev libgtk-4-dev libsoup-3.0-dev libadwaita-1-dev libdbus-1-dev -y
cargo install songrec --no-default-features -F gui,ffmpeg,pulse,mpris
https://github.com/marin-m/SongRec
https://lib.rs/crates/songrec
0.6.4

---

https://github.com/taiki-e/cargo-hack
=> verify usage as subcommand

---

https://github.com/moby/buildkit/issues/2805#issuecomment-4034926285
Proposal: Add mode attribute to local exporter #2805
    mode=continue

---

https://github.com/BuilderHub/buildkit-metrics-agent
 A lightweight BuildKit metrics agent. 

---

https://github.com/tonistiigi/xx#rust

https://github.com/cross-rs/cross-toolchains

https://docs.docker.com/build/ci/github-actions/share-image-jobs/

https://github.com/docker/build-push-action

https://github.com/rust-lang/docker-rust/blob/3a5e32f235c2be1989511f9e7a6b48c9cf140b2e/stable/trixie/Dockerfile

---

https://github.com/googlefonts/fontations/blob/fa64a3d65f68f981a41314b1a3e48c2dbddd55b0/skera/Cargo.toml#L15

https://github.com/libjxl/jxl-rs/blob/7cf3a662ea0f487c3b47ad871aab3575d1a3146a/jxl_cli/Cargo.toml

https://github.com/pdeljanov/Symphonia/blob/e5313f610d0c0a233c61da51d89c1bca29a85e9e/symphonia-play/Cargo.toml#L2

one day!
https://chromium.googlesource.com/chromium/src/third_party/rust/+/refs/heads/main
https://github.com/mozilla-firefox/firefox/tree/main/third_party/rust

---

Better way to express system deps (per OS / distro / pkmgr)

https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#platform-specific-dependencies

https://docs.rs/cargo-rustc-cfg/0.2.0/cargo_rustc_cfg/struct.Cfg.html#method.rustc_target


Similiarly to
```toml
[dependencies]
clap = "4.6.0"
lazy_static = { workspace = true }
log = { workspace = true, features = ["release_max_level_info"] }
pretty_env_logger = "0.5"
symphonia = { version = "0.6.0-alpha.1", path = "../symphonia", features = ["all"] }

[target.'cfg(target_os = "linux")'.dependencies]
libpulse-binding = "2.5.0"
libpulse-simple-binding = "2.5.0"

[target.'cfg(not(target_os = "linux"))'.dependencies]
cpal = "0.17.3"
rb = "0.4.1"
rubato = "2.0.0"
```

we should be able to say

```toml
[green.packages / system-dependencies]
clap = "4.6.0"
lazy_static = { workspace = true }
log = { workspace = true, features = ["release_max_level_info"] }
pretty_env_logger = "0.5"
symphonia = { version = "0.6.0-alpha.1", path = "../symphonia", features = ["all"] }

[green.target.'cfg(target_os = "linux")'.dependencies]
libpulse-binding = "2.5.0"
libpulse-simple-binding = "2.5.0"

[green.target.'cfg(not(target_os = "linux"))'.dependencies]
cpal = "0.17.3"
rb = "0.4.1"
rubato = "2.0.0"
```
but support more cfgs than just:

    target_arch="x86_64"
    target_endian="little"
    target_env="msvc"
    target_family="windows"
    target_feature="fxsr"
    target_feature="sse"
    target_feature="sse2"
    target_os="windows"
    target_pointer_width="64"
    target_vendor="pc"

eg.

```toml
[green.target.'cfg(package_manager = "apt")'.system-dependencies]
```

(same algebra though: sum it all. If a target has >1 pkg mgr, then it gets the sum of all matching entries)
  ((any errors to catch, statically?))

====> an approach only based on "which package manager" won't cut it!
=====> see https://github.com/alacritty/alacritty/blob/94e7c8874e526b1e67b349d9ba30ddf81669119e/INSTALL.md#dependencies

---

build cache
=> exports to containerd's registry:3
=> also could to nix's

build output
=> local xdg dir
=> r2
=> s3 likes

runner
=> buildkit native ==> drop buildx
=> none ==> no remote but can still hit outputs cache
=> podman (handled with buildkit?)
=> nix ==> populates a different build cache

both docker's and nix's build caches can then be used to craft images
=> verify if nix's can be used similarly to docker's

https://github.com/sayavc/niux

---

=> multiplatform building

https://github.com/ArcaneNibble/awawausb/commit/67e16584a059f72b744e234d10c9ef8bbc402393
docker build -o=awawausb https://github.com/ArcaneNibble/awawausb.git

---

https://crates.io/crates/zizmor

https://crates.io/crates/resonators

https://github.com/ArcaneNibble/awawausb/tree/main/native-stub

https://github.com/matrix-org/matrix-rust-sdk

https://crates.io/crates/epanet-rs

https://github.com/bnjbvr/cargo-machete

https://lib.rs/crates/cargo-sonic

https://lib.rs/crates/cargo-criterion

https://github.com/warpdotdev/warp

https://github.com/zed-industries/zed/releases/tag/v1.0.0

---

https://github.com/zizmorcore/zizmor/blob/e8a2bb8de2f4d09da9cc7bb41d5f0a12388093cc/crates/zizmor/src/main.rs#L853
    indicatif
=> TUI: split view with logs

---

https://github.com/zizmorcore/zizmor/blob/e8a2bb8de2f4d09da9cc7bb41d5f0a12388093cc/mkdocs.yml
    https://docs.zizmor.sh/usage/#cargo-style-output-plain
=> docs website

---

https://github.com/docker/github-builder
    Official Docker-maintained reusable GitHub Actions workflows to securely build container images
        https://docs.docker.com/build/ci/github-actions/github-builder/

---

https://github.com/moby/buildkit/releases/tag/dockerfile/1.23.0
    Git URLs now accept the mtime=commit query parameter to initialize checked-out file timestamps to Git commit time. Remote builds using a Git context that define SOURCE_DATE_EPOCH automatically default to mtime=commit for better reproducibility. #6600
    Dockerfile can now define SOURCE_DATE_EPOCH build-arg in the global scope with a default value. The value can still be overridden with --build-arg as before. #6601

---

cargo green fetch
=> dont check sentinel in this only case

---

prebuild:
    maximize layers
        seems to get 1 ADD to download per stage
            so 127 - 3 (debian + xx + frontend) gives max stages usables for ADDs
            ==> faster/fastest prebuild
=> OR look for buildkitd.toml param that throttles concurent ADDs

---

https://github.com/moby/buildkit/releases/tag/dockerfile/1.24.0
Dockerfile now supports special arg definitions SOURCE_DATE_EPOCH=context and SOURCE_DATE_EPOCH=<stage> which set the value of SOURCE_DATE_EPOCH to the timestamp associated with the remote context or the stage respectively. When building from a Git commit, the context timestamp is the commit timestamp, and when building from a remote URL, the timestamp is resolved from the metadata of files in the TAR archive or from the Last-Modified header of the URL #6602
==> set crate/git epochs from their ADD stage

---

https://github.com/sharkdp/fd

https://crates.io/crates/ring/reverse_dependencies?page=54
https://lib.rs/crates/lychee
https://lib.rs/crates/redbpf
https://lib.rs/crates/s3m
https://lib.rs/crates/cargo-resources
https://lib.rs/crates/rustup-mirror
https://lib.rs/crates/aati
https://crates.io/crates/voila
https://crates.io/crates/slugid
https://lib.rs/crates/rmd
https://lib.rs/crates/cratery
https://lib.rs/crates/hfile
https://lib.rs/crates/gauth
https://lib.rs/crates/muid
https://lib.rs/crates/pbcli
https://lib.rs/crates/yayo
https://lib.rs/crates/duplicate-checker
https://lib.rs/crates/mediafire_rs
https://lib.rs/crates/pw

---

impl these recommendations
  https://www.tangramvision.com/blog/building-robust-filesystem-interactions-in-rust
error wrappings
  https://docs.rs/fs-err

---

sort=downloads ./hack/find.sh rev image

---
