# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.28.0](https://github.com/fenollp/supergreen/compare/v0.27.0...v0.28.0) - 2026-10-28

### Added

- *(logging)* log from the start of cargo green
- single COPY (via --parents) per out stage
- rename stdio/errcode files
- set tar chmod + ensure stdio_and_errcode_files_never_reach_host_disk
- RUN scripts as heredocs, not backslashed lines
- *(sys)* lift what cargo reads from us into a Cargo device
- *(redact)* only log values of environment variables known to be harmless
- *(results)* compress with zstd, through pure-Rust ruzstd
- *(final)* check $CARGOGREEN_FINAL_PATH is writable on startup
- *(sys)* add unit tests with mocked side effecting devices (IO, ...) and snapbox
- *(sys)* OCI images resolution operations
- *(sys)* git repo operations
- *(sys)* Runner builds operations
- *(sys)* lift filesystem side effects
- *(sys)* introduce injectable side effects for better testability
- *(envs)* sh: quote fewer things
- *(dirs)* introduce virtual_cwd
- *(relative)* skip serializing empty values to Md
- *(md)* add helper to render containerfile
- *(image_uri)* do not attempt locking images when Runner::None
- feat build: somewhat more informative logging on disk quota errors
- only say what extra command we fork when given cargo verbosity flags
- *(wrap)* local crate stage no longer embeds host locations
- *(paths)* call paths.rewrite instead of paths.virtual_target_dir when mounting in rustc wrap + uniformize paths helpers names
- upgrade tests around shell-quote behavior
- *(wrap)* replace ./../ with ../ and save those bytes in large Containerfiles
- *(main)* suggest something when mistyping "cargo green supergreen"

### Fixed

- *(builder)* tell local registries apart by parsing their host
- *(builder)* do not fail on cleaning up the buildkitd config
- *(checkouts)* decode the %-encoded file:// URL of git DBs
- *(md)* check stamp before strict parsing, bump it
- *(build_script)* mount the $OUT_DIRs that links metadata points into
- *(md_id)* parse MdIds of build script exes and out dirs without panicking
- *(envs)* rewrite EOR lines of multiline values as 'E'OR to protect RUNs' HEREDOCs
- *(md)* read set_envs back in the order build scripts printed them
- *(build)* keep a build's output lines as they were written
- *(rustc_arguments)* sort externs so order doesn't miss results
- *(md)* don't mount a dependency's build script's deps into its own dependents
- *(md)* mount only a library's .rlib/.rmeta into dependents, not its other crate types
- *(build_script)* mount a build script's sources where it runs from
- *(paths)* rewrite all paths of a list (e.g. $PATH), not just the first
- *(rustc)* use the container's incremental dir, as rustc's call does
- *(paths)* rewrite every $CARGO_HOME path of a text, not just the first
- *(relative)* .dockerignore target dirs that are nested under a crate's mounted dirs
- *(tests)* address clippy::significant_drop_tightening
- *(paths)* rewrite the bare target dir, as the bare project dir
- *(logging)* create log files owner-only
- *(green)* do not pass the environment snapshot to wrappers
- *(stage)* shorten stage names longer than BuildKit allows, rather than erroring
- *(relative)* extend a users .dockerignore while building, then restore it
- *(build)* read build scripts' cargo:: instructions (Rust 1.77+ syntax)
- for `cargo green test` and CARGO_RUSTC_CURRENT_DIR cf. https://github.com/assert-rs/snapbox/blob/f85fb904/crates/snapbox/src/macros.rs#L101
- *(add)* shell-quote OS packages with shell_quote::Sh
- *(dirs)* do not panic showing prebuild errors
- *(build_script)* with buildscriptsources, mount each sources dir once
- *(stage)* fold dashes to the last one
- *(logging)* use try_init instead of panic-ing on errors
- fix wrap: drop dead code WRT filtering out {stage}-{STDOUT,STDERR,ERRCODE}
- *(wrap)* properly drop .dwp files when writing build outputs
- fix stage: please clippy
- *(main)* serialize and pass settings only in wrap cases
- *(main)* revert bad change on "fetch" command
- *(builder)* do not error when removing non-existing builder

### Other

- *(builder)* pass buildkitd's insecure entitlement as --flag=value
- *(build_script)* assert the order of $DEP_* out dirs found (IndexSet equality ignores order)
- *(build_script)* share splitting $DEP_* values into words
- *(cargo_green)* read the runner and dirs before the settings
- *(builder)* decide builder setup apart from running it
- *(builder)* a typed Driver, named as buildx names drivers
- *(stage)* AsStage::mounts() returns impl IntoIterator
- *(md)* add Md::{mount_out_dir_of,mount_output}
- *(stage)* Stage::{local,incremental,output} can't fail
- *(green)* check computed and env-only settings in Green::try_new
- build Vars and Green envs from string pairs
- *(md)* replace Md.buildrs with Md::is_buildrs_run (ie. writes_to.is_some())
- *(md)* rename some Md struct fields
- log wrappers' errors once, from main
- move some fixtures into files
- *(cratesio)* drop an unwrap for an unlikely case
- *(builder)* match BUILDX_BUILDER docs to code, explain the managed builder
- introduce a dotd module
- *(md)* skip mount paths that have no file name instead of unwrapping
- document ImageUri methods and Effects
- *(dirs)* make rewrite_cwd #[cfg(test)]
- *(dirs)* swap is_named_same_as_virtual_target_dir for virtual_target_dir()
- *(build)* return a Built struct instead of a 5-tuple
- *(final)* split and simplify
- *(final)* read envs once only
- *(supergreen)* read envs once only
- *(wrap)* no need to own that pwd
- *(logging)* setup and drop some more direct env reads
- *(main)* read args from cmd instead of re-skipping again
- *(cargo_arguments)* ask for OsStr instead of String args
- refacto hashing: more testable code again
- restrict pwd() invocations to only from within main.rs
- tidy up args passed to wrappers
- refacto vars: extract env-reading for testability
- *(deps)* Bump astral-tokio-tar from 0.6.4 to 0.7.0
- refacto chmod: mention it only where needed
- further type mount_flag
- turn replace_carefully into replace_tokens
- *(target_dir)* move under dirs mod
- ./hack/latest_buildkit.sh | tee cargo-green/latest_buildkit.txt
- split dirs module into files
- move dirs module to a directory
- *(main)* replace custom EEXIT str matching with typing
- *(target_dir)* rename TARGET_DIR to HOST_TARGET_DIR
- release v0.27.0

## [0.27.0](https://github.com/fenollp/supergreen/compare/v0.26.0...v0.27.0) - 2026-07-24

### Added

- use strip-ansi-escapes crate instead of custom simplistic code
- *(dirs)* add context to create_current_target_dir errors
- *(main)* skip some unsafe envs-setting we set via cmd
- discard errored results
- log some file reading operations with more context
- *(md)* write Md TOMLs atomically
- *(wrap)* no more stdios/errcode files in out_dir mounts
- *(md)* let's stamp Md.s for compatibility
- *(main)* warn on usage when calling plugin without cargo, again
- forbid unhandled CARGOGREEN_* env vars
- *(image_uri)* allow overriding syntax image
- *(main)* warn on usage when calling plugin without cargo
- read given --target <TARGET> & handle cross-compilation paths
- *(wrap)* do not pass down more cargo-only envs
- *(build)* support another buildkit_interrupted transient reason
- *(wrap)* do not pass down cargo-only CARGO_BUILD_WARNINGS env

### Fixed

- *(main)* properly guess cargo command by actually parsing its optional arguments
- actually use unsafe env::set_var in a safe way, in single-threaded code
- do not ignore errors where it matters when streaming bytes
- *(build)* correctly handle legacy way of passing metadata to cargo
- make errors while writing results non-fatal
- fwd to cargo /after/ writing toml to avoid racey Md reads of files written
- appease clippy on some minor thing
- *(build)* avoid silently skipping build failures
- *(build_script)* PWD is actually CARGO_MANIFEST_DIR as git workspace members show

### Other

- *(build)* avoid repeating substring pattern
- *(runner)* use a wider net when catching 'em runner envs pokemons
- move more strings to all_our_envs
- *(main)* thats a match
- macro-define envs from/for rustup/cargo/rustc
- introduce envdocs and envdocs2
- move CARGOGREEN_ defining macros to all_our_envs
- *(buildscriptsources)* keep buildrs deps srcs in some scenarios
- *(dirs)* easier-to-read target_dir computation
- ./hack/latest_buildkit.sh
- *(build)* make Effects::try_to_help somewhat easier on the eyes
