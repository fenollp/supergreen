# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.27.1](https://github.com/fenollp/supergreen/compare/v0.27.0...v0.27.1) - 2026-07-24

### Other

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
