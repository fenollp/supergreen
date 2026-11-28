Building a crate runs code: build scripts and procedural macros, yours and your dependencies'. `cargo` runs it as you, with your files, your secrets and your network. `cargo green` wraps `rustc` calls and build scripts in BuildKit containers instead:

* **No network** (`RUN --network=none`), but for the dependencies you grant it to, by name and version requirement: see [`$CARGOGREEN_DEPS_NETWORK`](#cargogreen_deps_network).
* **A constrained filesystem:** a sandbox holds the toolchain and what its step mounts, not your home directory, your SSH keys or the Docker socket. Of your environment variables it gets Cargo's and `rustc`'s (not those of registries, such as tokens), proxy settings and [`set-envs`](#cargogreen_set_envs).
* **Read-only sources:** what a build writes in its sandbox doesn't reach your sources, your `$CARGO_HOME` nor your toolchain. Only its outputs come back, to the target directory.
* **No fallback:** a step that fails or gets refused in its container isn't retried on your machine.
* **Hermetic and reproducible builds:** the base and BuildKit frontend images are locked to digests, `rustup-init` and crate archives are checked against their SHA256, git dependencies are added at their locked commit and timestamps are fixed. Builds are written down as a Containerfile you can read and replay, see [`$CARGOGREEN_FINAL_PATH`](#cargogreen_final_path).

This takes the sting out of common software supply chain attacks, where a compromised or typosquatted package acts as soon as it gets built: reading credentials, sending them out, downloading a payload, persisting on your machine or tampering with other packages.

It does **not** protect against everything:

* What you run isn't sandboxed: `cargo green run`, `test` and `bench` execute what was built, malicious code included.
* `rustdoc` (`cargo doc`, doctests) and `clippy-driver` (`cargo clippy`) run on your machine, along with the procedural macros they expand.
* A local crate's sandbox sees all of its directory but what `.dockerignore` excludes, files you don't commit included.
* A project's own `[package.metadata.green]` is trusted: it can pick the base image, add OS packages and grant network access. Read it before building a project you didn't write.
* Base stages have network access, and OS packages (`add`) aren't locked. Neither is whatever a dependency granted network access downloads.
* Cached results are trusted: only use cache images that people you trust can push to.
* These are containers, not virtual machines: the kernel is shared.
* `$CARGOGREEN_RUNNER=none` is **not** a way to avoid running foreign code. It only reuses what earlier containerized builds stored: everything else is compiled and run on your machine, without any sandbox.


