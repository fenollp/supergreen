# [`supergreen`](https://github.com/fenollp/supergreen)

Faster Rust builds!

* [`cargo-green`](./cargo-green): Cargo plugin and `$RUSTC_WRAPPER` to sandbox, cache & remote exec `cargo` builds

![A rusty crab character named Ferris, featuring a unique hairstyle resembling 'Ruby Road', a vibrant and textured hairdo often seen in flamboyant red](./hack/logo.jpg)

## Goals
* [x] seamlessly build on another machine (with more cores, more cache)
  * [x] support remote builds by setting env `DOCKER_HOST=` with e.g. `ssh://me@beaffy-machine.internal.net`
    * [x] Build cache is saved remotely, artifacts are saved locally
    * [x] Tests building happens on remote machine, execution happens on local machine
* [x] seamlessly integrate with normal `cargo` usage
  * [x] only pull sources from local filesystem
  * [x] produce the same intermediary artefacts as local `cargo` does
  * [x] fallback to normal, local `rustc` anytime
    * switching from this wrapper back to local `rustc` does necessitate a fresh build
* [ ] wrap `rustc` calls in `buildkit`-like calls (`docker`, `podman`)
  * [x] `docker`
  * [x] `podman`
  * [ ] deps compatibility
    * [x] handle Rust-only deps
    * [ ] handle all the other deps (expand this list) (use `crater`)
      * [x] `C` deps
      * [ ] ...
  * [x] runner compatibility
    * [x] set `.dockerignore`s (to be authoritative on srcs)
  * [x] trace these outputs (STDOUT/STDERR) for debugging
* [x] available as a `rustc` wrapper through `$RUSTC_WRAPPER`
* [ ] available as a `cargo` subcommand
  * [ ] configuration profiles (user, team, per-workspace, per-crate, CI, ...)
  * [x] seamlessly use current/local `rustc` version
    * [x] support overriding `rustc` base image
  * [ ] seamlessly use current/local tools (`mold`, ...)
    * [ ] config expressions on top of base image config?
    * [ ] just suggest an inline `Dockerfile` stage?
  * [ ] support CRUD-ish operations on local/remotes cache
  * [x] `[SEC]` support building a crate without it having network access
* [ ] integrate with shipping OCI images
* [ ] share cache with the World cf. [`user-wide-cache`](https://rust-lang.github.io/rust-project-goals/2024h2/user-wide-cache.html)
  * [x] never rebuild a dep (for a given version of `rustc`, ...)
    * [x] ensure finest cache granularity (crate-level)
    * [x] free users from cache key built from `Cargo.lock` (changes on every release cut!)
  * [ ] share cache with other projects on local machine
    * [ ] fix `WORKDIR`s + rewrite paths with `remap-path-prefix` 
  * [ ] share cache with CI and team
    * [ ] share cache with CI (at least for a single user)
  * [ ] `[SEC]` ensure private deps don't leak through/to cache
  * [ ] CLI gives the Dockerfile that `cargo install`'s any crate
* [ ] suggest a global cache -faciliting configuration profile
* [ ] integrate with `cross`
  * [ ] build for a non-local target
  * [ ] run/test for a non-local target (with `cross`'s same caveats ie. QEMU)


## Upstream issues & patches
* [ ] [`rust`: Compile a crate from its source archive directly](https://github.com/rust-lang/rust/issues/128884)
* [ ] [`cargo`: Tell `rustc` wrappers which envs to pass through to allow env sandboxing](https://github.com/rust-lang/cargo/issues/14444)
* [ ] [`buildkit`: `docker/dockerfile` image tags are late](https://github.com/moby/buildkit/issues/6118)
* gRPC buffers too small
  * [x] [`buildkit`: Build function: ResourceExhausted: grpc: received message larger than max (_ vs. 4194304)](https://github.com/moby/buildkit/issues/5217)
  * [x] [`buildx`: `ResourceExhausted: grpc: received message larger than max (_ vs. 4194304)`](https://github.com/docker/buildx/issues/2453)
  * [ ] [`buildkit`: remote `docker buildx build` with large dockerfile gives `trying to send message larger than max (22482550 vs. 16777216)` error](https://github.com/moby/buildkit/issues/6097)
* [x] [`buildkit`: Support extracting `ADD --checksum=.. https://.. ..`](https://github.com/moby/buildkit/issues/4907)
* TODO
  1. buildkit flag to disable dockerignore and save disk read
    * --ignore-file (closed) [Add support for specifying .dockerignore file with -i/--ignore](https://github.com/moby/moby/issues/12886)
    * --no-ignore-file
  1. docker build support multiple input files
    * --file-part
    * >=1 stage per Dockerfile part file
      * order doesn't matter: order is fixed when consolidation happens (internally)
  1. cargo + docker
    * [cargo build --dependencies-only](https://github.com/rust-lang/cargo/issues/2644#issuecomment-2304774192)


## Origins
* PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh
* Initial blog post https://fenollp.github.io/faster-rust-builds-docker_host


## Hacking

### `./hack/cli.sh ...`
```shell
Usage:           $0                                      #=> generate CI
Usage:           $0 ( <name@version> | <name> ) [clean]  #=> cargo install name@version
Usage:           $0   ok                        [clean]  #=> cargo install all working bins
Usage:           $0 ( build | test )            [clean]  #=> cargo build ./cargo-green
Usage:    jobs=1 $0 ..                                   #=> cargo --jobs=$jobs
Usage: offline=1 $0 ..                                   #=> cargo --frozen (defaults to just: --locked)
```

### `./hack/recipes.sh`
Syncs `./recipes/*.Dockerfile` files.

### `./hack/caching.sh`
Verifies properties about caching crates & granularity.
> docker buildx prune --all --force

### `./hack/hit.sh`
Estimate of amount of crates reused through compilation of `./recipes/` `--> ~5%`!
Expecting more with larger/more representative corpus + smart locking of transitive deps.
```
recipes/buildxargs@1.4.0.Dockerfile
8< 8< 8<
3: dep-l-utf8parse-0.2.1-522ff71b25340e24
5: dep-l-bitflags-1.3.2-70ce9f1f2fa253bc
5: dep-l-strsim-0.10.0-fd42a4ea370e31ec
5: dep-l-unicode-ident-1.0.12-4c1dc76c11b3deb8
6: dep-l-cfg-if-1.0.0-da34da6838abd7f1

Total recipes: 15
Total stages: 1065
Stages in common: 58
5.44%
```

### When `git-bisect`ing
```make
all:
	cargo +nightly fmt --all
	./hack/clis.sh | tee .github/workflows/clis.yml
	./hack/self.sh | tee .github/workflows/self.yml
	CARGO_TARGET_DIR=$${CARGO_TARGET_DIR:-target/clippy} cargo clippy --locked --frozen --offline --all-targets --all-features -- --no-deps -W clippy::cast_lossless -W clippy::redundant_closure_for_method_calls -W clippy::str_to_string -W clippy::unnecessary_wraps
	RUST_MIN_STACK=8000000 cargo nextest run --all-targets --all-features --locked --frozen --offline --no-fail-fast
	git --no-pager diff --exit-code
```
