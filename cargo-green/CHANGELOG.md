# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.27.1](https://github.com/fenollp/supergreen/compare/v0.27.0...v0.27.1) - 2026-09-05

### Added

- feat build: somewhat more informative logging on disk quota errors
- only say what extra command we fork when given cargo verbosity flags
- *(wrap)* local crate stage no longer embeds host locations
- *(paths)* call paths.rewrite instead of paths.virtual_target_dir when mounting in rustc wrap + uniformize paths helpers names
- upgrade tests around shell-quote behavior
- *(wrap)* replace ./../ with ../ and save those bytes in large Containerfiles
- *(main)* suggest something when mistyping "cargo green supergreen"

### Fixed

- fix wrap: drop dead code WRT filtering out {stage}-{STDOUT,STDERR,ERRCODE}
- *(wrap)* properly drop .dwp files when writing build outputs
- fix stage: please clippy
- *(main)* serialize and pass settings only in wrap cases
- *(main)* revert bad change on "fetch" command
- *(builder)* do not error when removing non-existing builder

### Other

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
