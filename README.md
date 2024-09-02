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
  * [ ] share cache with other projects on local machine
    * [ ] fix `WORKDIR`s + rewrite paths with `remap-path-prefix` 
  * [ ] share cache with CI and team
    * [ ] share cache with CI (at least for a single user)
  * [ ] `[SEC]` ensure private deps don't leak through/to cache
  * [ ] CLI gives the Dockerfile that `cargo install`'s any crate
* [ ] suggest a global cache -faciliting configuration profile
* [ ] integrate with `cross`
  * [ ] build for a non-local target


## Upstream issues & patches
* [ ] [`rust`: Compile a crate from its source archive directly](https://github.com/rust-lang/rust/issues/128884)
* [ ] [`cargo`: Tell `rustc` wrappers which envs to pass through to allow env sandboxing](https://github.com/rust-lang/cargo/issues/14444)
* TODO
  1. buildkit flags to prefix stdout and stderr progress
  1. buildkit flag to disable dockerignore and save disk read
    * --ignore-file (closed) https://github.com/moby/moby/issues/12886
    * --no-ignore-file
  1. docker build support multiple input files
    * --file-part
    * >=1 stage per Dockerfile part file
      * order doesn't matter: order is fixed when consolidation happens (internally)
  1. cargo + docker
    * https://github.com/rust-lang/cargo/issues/2644#issuecomment-2304774192


## Origins
* PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh
* Initial blog post https://fenollp.github.io/faster-rust-builds-docker_host


## Hacking
See `./hack/`
