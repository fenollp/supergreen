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

```
 FROM rust-base AS dep-l-anyhow-1.0.79-95e5d8a0e52ba465

+  --mount=from=ran-467b075ea0bb0ef8,dst=/tmp/clis-ripgrep_14-1-0/release/build/anyhow-467b075ea0bb0ef8/out,source=/ \
```

rustc missing '--cfg' 'std_backtrace'


=> runnning buildrs misses cargo::outputs
z

https://github.com/dtolnay/anyhow/blob/1.0.79/build.rs

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

TODO: handle not-yet-published rust images => fallback to rustup

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

```
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯  && if   command -v apk >/dev/null 2>&1; then
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯                                      xx-apk     add     --no-cache                 '<none>'; \
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯     elif command -v apt >/dev/null 2>&1; then \
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯       DEBIAN_FRONTEND=noninteractive xx-apt     install --no-install-recommends -y 'libelf-dev'; \
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯     else \
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯       DEBIAN_FRONTEND=noninteractive xx-apt-get install --no-install-recommends -y '<none>'; \
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯     fi'''
T 25/11/22 09:38:17.432 N clang-sys 1.3.0 ecb54402a27d97cd ❯ [[stages]]
```
no
if var set then call install
> DEBIAN_FRONTEND=noninteractive xx-apt     install --no-install-recommends -y 'libelf-dev'
thatsit

add: alphasort pkgs before containerfile

also
add: should be per distro
* one cargotoml to support multiple distros
    * how to detect which distro is in use?

apt apk :
* vars for both apt + apk: {apt=libwut-dev, apk=libwut}
* another var to say which tool  to use: apt
  _____________________________s to use /in order: apt,apt-get

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

```
1    supergreen.git main 🔗 crun green +1.90 supergreen env
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.23s
     Running `target/debug/cargo-green green +1.90 supergreen env`
Overriding RUSTUP_TOOLCHAIN="stable-aarch64-apple-darwin" to "1.90" for `cargo-green +toolchain`
Using runner /usr/local/bin/docker
Calling DOCKER_BUILDKIT="1" /usr/local/bin/docker buildx ls --format=json
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/local/bin/docker buildx du --verbose --filter=type=regular --filter=description~=pulled.from
Calling BUILDX_BUILDER="supergreen" DOCKER_BUILDKIT="1" /usr/local/bin/docker inspect --format={{index .RepoDigests 0}} docker.io/docker/dockerfile:1
Calling  /Users/pierre/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo locate-project
error: no such command: `supergreen`

help: view all installed commands with `cargo --list`
help: find a package to install `supergreen` with `cargo search cargo-supergreen`
```

> RUSTUP_TOOLCHAIN=1.90 crun green supergreen env
works tho

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

