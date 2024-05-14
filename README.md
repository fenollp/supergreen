# [`supergreen`](https://github.com/fenollp/supergreen)

Faster Rust builds!

* [`cargo-green`](./cargo-green): Cargo plugin to sandbox, cache & remote exec `cargo` builds
* [`rustcbuildx`](./rustcbuildx): `$RUSTC_WRAPPER` tool to sandbox `cargo` builds and execute jobs remotely

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
* [ ] share cache with the World
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


## Origins
PoC originally written in Bash: https://github.com/fenollp/buildxargs/blob/buildx/tryin.sh


## Hacking
See `./hack/`
