# ROADMAP

**v1 gate** = tiers 1–3: maximum cache reuse, Rust 1.100 compatibility, and a signed results store.
Tier 4 (correctness) is ranked right after tier 1 and should land before tagging.
Everything from tier 5 on is post-v1.

## 1. Cache key: maximum reuse

The same crate, built twice on different machines or days, must hit the same cache entry.

- [ ] **Finer results key** — key results by recipe, not just by mdid; never store results of builds reading host sources; stats say why a result missed. Branch `bettertests--finerkey`.
- [ ] **Drop the rustup script** — fetch and use `rustup-init` with `ADD` + `WORKDIR` instead of running it with network. Aim for the same file tree and permissions as `/rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain <v>-<host> --default-host <host>`. Branch `rustitup` (WIP). See [toolchains](https://rust-lang.github.io/rustup/concepts/toolchains.html).
- [ ] **Toolchain for BuildKit's build platform** — install for the builder's platform, not the client's. Branch `rustup-buildplatform`.
- [ ] **Filter `--cap-lints` at the wrapper** — cargo passes `--cap-lints allow` to dependencies, so lint-only flag changes bust keys. Branch `novv`.
  Design: invoke `rustc` with `--cap-lints warn --error-format=json` (`allow` drops lints before output), then re-classify by `code.code` and `level`: pass errors through, custom-deny warnings exit non-zero, custom-warn pretty-prints, custom-allow discards. `--force-warn <lint>` bypasses the ceiling for one lint. Also see [cargo support for denying warnings](https://blog.rust-lang.org/2026/07/09/Rust-1.97.0/#cargo-support-for-denying-warnings) (1.97).
- [ ] **Allowlist non-busting rustc flags** — decide which flags may differ without changing the key (`hack/binaries.sh`). Includes `cargo -vv test` ≠ `cargo test` producing different flags, and checking `build -> test -> build` shows no `Dirty`.
- [ ] **Lock distro packages** — pin `apt`/`apk` package resolution (snapshot date = base image's); alphasort packages before they hit the Containerfile; finer per-distro package installs. See `add.rs`, `base_image.rs`.
- [ ] **Nix flakes: is the key as coarse?** — check whether all crates share one system-package cache key under nix; maybe retry compilation with and without the PRIMARY custom OS packages.
- [ ] **Install packages in separate stages, then merge with `--link`** — so one package change doesn't bust the rest.
- [ ] **Non-determinism in the generated Containerfile** — kani-verifier recipe flips between runs (two bins, `cargo-kani` and `kani`, both picked). Sort `--bins` when generating the final path; the final `scratch` stage must contain all bins. Repro: `CARGOGREEN_EXPERIMENT=repro ./hack/clis.sh kani`.
- [ ] **Property: `buildkitd.toml` is invariant to ordering.**
- [ ] **`SOURCE_DATE_EPOCH` on `COPY`** — adopt [`COPY --rewrite-timestamp`](https://github.com/moby/buildkit/issues/6348) when it lands.

## 2. Rust 1.100 compatibility

- [ ] **Support cargo/rustc 1.100** — new [build-dir layout](https://doc.rust-lang.org/nightly/cargo/reference/build-cache.html) (also see the [call for testing](https://blog.rust-lang.org/2026/03/13/call-for-testing-build-dir-layout-v2/)); `md/md_id.rs` TODO. Read the [1.100 release notes](https://blog.rust-lang.org/2026/11/12/Rust-1.100.0/) and [cargo 1.100 changelog](https://doc.rust-lang.org/nightly/cargo/CHANGELOG.html#cargo-1100-2026-11-12).
- [ ] **Unpin CI from 1.99.1** — `hack/ck.sh` `latest=1.99.1 # FIXME: set back to 'stable'`; also bump `stable=` as official images land (TODO: fall back to rustup when the image isn't there yet).
- [ ] **CI against beta** — catch the next break before release.
- [ ] **Docs: `cargo install --locked` before 1.100** — suggest it in the README install section.
  Reasoning to cite: without `--locked`, cargo ignores the packaged `Cargo.lock` and resolves fresh against `Cargo.toml`, so a bad new sub-dependency release can break the install; with `--locked` it builds the author's tested tree. It still builds yanked crates, because yanking only affects *new* resolutions, not an existing lockfile.
- [ ] **Persistent incrementality** — read [rust-glancer's write-up](https://rust-glancer.github.io/blog/persistent-incrementality-in-rust/) for what it means for `incremental` in containers (see also `-Zhint-mostly-unused`: how to cache with it).

## 3. Results store (R2) and signing

Branch `uploadresults-merged` already fetches from and publishes to a remote results store (SigV4, R2/S3, streaming, ETag verification).

- [ ] **Land fetch/publish** — merge `uploadresults-merged` after tier 1 (the results key must be final first, or published results are orphaned).
- [ ] **Signed tarballs** — *design to settle.*
  Proposed: a detached **minisign/ed25519 signature** per `out-*.tar.zst`, covering the tarball and its appended `md.toml`; clients pin the publisher's public key; unsigned or mismatching results are treated as a cache miss, never extracted.
  Verifying contents: (1) check the signature, (2) re-derive the recipe hash and compare with `md.toml`, (3) compare `tar -t` listing with the files `md.toml` declares. Alternatives to record: sigstore/cosign keyless (no keys, needs network + OIDC), reproducibility-based trust à la [trustix](https://github.com/nix-community/trustix) (needs the determinism work in tier 1 first).
- [ ] **Per-user upload/download accounting** — R2 has no native per-file counts. Options: Worker in front of the bucket + D1/KV (`$0.30/1M` Worker + `$0.36/1M` Class B reads + KV), Logpush data access logs, or GraphQL analytics (aggregates only). Keep the bucket private, authenticate before `bucket.get()`, presigned URLs for uploads, rate-limit downloads, batch counter writes through Queues.
- [ ] **Host a simple proxy in front of the R2 CDN** — doubles as the auth/counting layer above.
- [ ] **Docs** — how to configure the store, how signing works, how to verify by hand.

## 4. Correctness

Found by the corpus/property tests.

- [ ] **`.dockerignore` as a CLI arg** — ask upstream to pass dockerignore content along with `--build-context=NAME=DIRPATH` ([moby#40319](https://github.com/moby/moby/issues/40319)).
- [ ] **Checkout commit mismatch** — commit is taken from the last line of `FETCH_HEAD`, which can differ from the checked-out commit (coccinelleforrust `ADD`s `86de52a…` but mounts `50612e2`). Look into `checkouts.rs`.
- [ ] **Checkout stage naming/dedupe** — stage name comes from the crate's parent folder, so members of one repo at one commit each get their own stage with the same `ADD` (asterinas: 6×). Dedupe on stage content (BuildKit's caching already handles the duplicates). `checkouts.rs` FIXME.
- [ ] **Blank lines** — a crates.io stage dropped as a duplicate leaves an empty line (double blank line in the corpus).
- [ ] **`$PWD` for relative paths** — use `$PWD` in relative TOML; rewrite pwd to `$PWD` in the Containerfile; replace the last `WORKDIR` with `$PWD` in rustc args (`dirs/paths.rs`). `$OLDPWD` is unset after >1 `WORKDIR` ([buildkit#6698](https://github.com/moby/buildkit/issues/6698)). Branch `remappaths-moar` (WIP).
- [ ] **Handle `^C`** — kill subprocesses, delete sentinels, remove temp, abort sync work; add a property test (sub-ms SIGTERM). `build.rs:194` TODO.
- [ ] **Interleave stdio in `RUN`s** — keep stdout/stderr ordering.
- [ ] **`cargo install` of non-local code** — root crate's code isn't local yet, and config isn't readable ([cargo#9700](https://github.com/rust-lang/cargo/issues/9700#issuecomment-2748617896)); lockfile-less installs (`lockfile.rs`, `green.rs`).
- [ ] **Spurious `Dirty` in `hack/self.sh`** — `Dirty [..]: the file src/[..] has changed` (FIXME at `self.sh:277`).
- [ ] **Docker Hub digest for old tags** — `docker.io/library/rust:1.44.0-slim` fails with `missing field digest` (manifest-list shape). Replace the hand-rolled client, e.g. with `dkregistry`.

## 5. Local code and build contexts (post-v1)

- [ ] **Inline local code in the Containerfile** — embed files, fall back to a build context, pick files with `cargo package --list`. Branch `localcode-embbedfiles-contextfallback-cargopackagelist` (WIP). Reconsider for v1 if local workspace builds turn out to be the dominant path.
- [ ] **`bump-buildkit-msg-size-limits`** — raise BuildKit message limits; includes `hack/clis.sh` support for experimenting with patched buildkit/buildx, and the legacy metadata passing fix. Decide what is still wanted; the branch carries ~41k generated lines (recipes).
- [ ] **`rechrome` via a JSON library** — manipulate JSON with a library; add `cargo green un-rechrome` that edits emitted JSON in place (`rechrome.rs:590`).

## 6. Test and CI coverage

- [ ] **Test build** [plotextract v1.2.0](https://github.com/rajatmo/plotextract/blob/v1.2.0/Cargo.toml).
- [ ] **ESP32 build** and a special build for Raspberry Pi.
- [ ] **Paused time for retries** — `#[tokio::test(start_paused = true)]` or an injectable clock in `Retrier` to test counts/backoff without waiting.
- [ ] **Run tests remotely**, like [`cross`](https://github.com/cross-rs/cross/blob/49cd054de9b832dfc11a4895c72b0aef533b5c6a/README.md#supported-targets) (README).
- [ ] **Other runtimes in CI** — `podman`, runc, crun, containerd, lima, colima (`hack/clis.sh`, `hack/README.md`).
- [ ] **musl platform test**, per [trippy's Dockerfile](https://github.com/fujiapple852/trippy/blob/7fd4098ae50dbc930fd3b0503f66801a127cafff/Dockerfile).
- [ ] **Check `cache-to` works with `--output=cacheonly`** (see `TODO.md`).
- [ ] **Tests with known tags and the `docker.io/` prefix bug** (`supergreen.rs:312`).
- [ ] **More CLI corpora** — `try--*` branches (see table below).
- [ ] **GHA workflow validation.**
- [ ] **Experiment: don't clean up the env when calling `cmd()`.**
- [ ] **Hardlinks or mounts from tarballs** instead of extracting copies.
- [ ] **Capture `cargo doc` HTML generation.**

## 7. UX and CLI (post-v1)

- [ ] **TUI mode** — when stderr is a terminal, show ~10 log lines under cargo's normal progress (`wrap/rustc.rs:197`, `main.rs:218`; [prodash](https://docs.rs/prodash), [tracing-indicatif](https://crates.io/crates/tracing-indicatif)).
- [ ] **Stats** — after stdio interleaving, mention machine CPU specs (skip on remotes?) and memory used (rustc? `cc`? measure the container process).
- [ ] **Make `cargo green` show in `cargo --list`** (`main.rs:75`).
- [ ] **`supergreen` CLI** — stats (hits/misses/size/age, runners, disk), cache prune (`--keep-less-than=1month|10GB`), inspect/clear/push cache, show builder logs and Jaeger trace, verbosity flags, split `supergreen.rs` into files and group `relative`/`cratesio`/`checkouts` under a folder.
- [ ] **Config** — profiles / workspace config merging (`green.rs`), switch envs to TOML, `$CARGOGREEN_*` handling for `CARGO_BUILD_*` envs (`wrap/envs.rs`).
- [ ] **Multiplatform** — `base_image.rs` multiplatformify via auto `ARG`s; include `--target` platform in cache image tags; containerd image store.

## 8. Hermeticity and long tail (post-v1)

- [ ] Remove network needs: `rustup-init` and package installs via `ADD`s (`Network::None`).
- [ ] Private deps via `--secret` mounts; non-crates.io registries and proxies (`wrap/rustc.rs:99`).
- [ ] Proxy env vars hinder reproducibility (`runner.rs`).
- [ ] Jobserver interaction, especially for remote builders.
- [ ] `buildctl` runner (branch `buildctl`) / dropping the Docker CLI requirement (`build.rs:388`).
- [ ] Podman over `$CONTAINER_HOST`; remote `DOCKER_HOST=ssh://…` hardening.
- [ ] Export final as flatpak; `cargo green` as pushed multi-platform image with branch tags and `main` as `:latest`.
- [ ] Cache: parallel export to registry ([buildkit#6123](https://github.com/moby/buildkit/issues/6123)), exporter options (`cache/buildkit.rs:43`), GHA/local backends.
- [ ] Tidy: the ad hoc `tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f(green, fs))` — drop it (*unclear what to replace it with; see notes*).
- [ ] Optimise release profile: fewest codegen units for best LTO (`cargo-green/Cargo.toml:24`).
- [ ] Our own AI policy, perhaps based on <https://www.ruleflow.org/community/ai-policy/>.

## Stale branches: revive or delete?

No branch changes made; this is a table to decide on.

| Branch | Last commit | Note |
|---|---|---|
| `try--coccinelleforrust`, `try--miri`, `try--nanometers`, `try--privaxy`, `try--rapidraw`, `try--servo`, `try--uv`, `try--zed` | 2026-07/11 | CLI corpus attempts; revive via tier 6 |
| `try--rusty-man` | 2026-11-06 | recent; likely mergeable |
| `rustc-version-check` | 2026-11-04 | without a runner, reuse only host-rustc libraries; review for merge |
| `buildctl` | 2026-07-12 | runner experiment (tier 8) |
| `stashed`, `xx`, `pass-target`, `relyon-bindepinfo` | 2026-02/06 | likely superseded by finerkey/rustitup |
| `misc`, `nightlynight-onlygit`, `wip--cratesio-pure-crate-archive`, `wrap-linker--issue-126` | 2024–2025 | probably delete |
| `uploadresults`, `uploadresults-bis` | 2026-07-06 | superseded by `uploadresults-merged` |
