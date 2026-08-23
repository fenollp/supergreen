use std::collections::HashSet;

use crate::wrap::Vars;

macro_rules! envdocs {
    ($name:ident) => {
        include_str!(concat!("../docs/", $name!(), ".md"))
    };
}

macro_rules! envdocs2 {
    ($name:ident) => {
        include_str!(concat!("../../docs/", $name!(), ".md"))
    };
}

macro_rules! envname {
    ($name:ident) => {
        // Eponym string literal
        macro_rules! $name {
            () => {
                stringify!($name)
            };
        }
        // Same literal, $-prefixed
        #[allow(unused)]
        pub(crate) const $name: &str = concat!("$", $name!());
    };
}

envname!(BUILDKIT_HOST);
envname!(BUILDX_BUILDER); // buildx'
envname!(CARGO); // rustup setting we read
envname!(CARGO_CRATE_NAME); // cargo setting we read
envname!(CARGO_MANIFEST_DIR); // cargo setting we read
envname!(CARGO_NET_OFFLINE); // cargo setting we read
envname!(CARGO_PKG_NAME); // cargo setting we read
envname!(CARGO_PKG_VERSION); // cargo setting we read
envname!(CARGO_PRIMARY_PACKAGE); // cargo setting we read
envname!(CARGO_TARGET_DIR); // cargo setting we read/write
envname!(CARGOGREEN); // sentinel
envname!(CARGOGREEN_ADD_APK);
envname!(CARGOGREEN_ADD_APT);
envname!(CARGOGREEN_BASE_IMAGE);
envname!(CARGOGREEN_BUILDER_IMAGE);
envname!(CARGOGREEN_CACHE_FROM_IMAGES);
envname!(CARGOGREEN_CACHE_IMAGES);
envname!(CARGOGREEN_CACHE_TO_IMAGES);
envname!(CARGOGREEN_COMPONENTS);
envname!(CARGOGREEN_EXECUTEBUILDSCRIPT); // internal
envname!(CARGOGREEN_EXPERIMENT);
envname!(CARGOGREEN_FINAL_PATH);
envname!(CARGOGREEN_LOG);
envname!(CARGOGREEN_LOG_PATH);
envname!(CARGOGREEN_LOG_STYLE);
envname!(CARGOGREEN_PLUGINSETTINGS); // Internal env used to pass config from cargo plugin to rustc wrapper
envname!(CARGOGREEN_REGISTRY_MIRRORS);
envname!(CARGOGREEN_RUNNER);
envname!(CARGOGREEN_SET_ENVS);
envname!(CARGOGREEN_SYNTAX_IMAGE);
envname!(CARGOGREEN_WITH_NETWORK);
envname!(DOCKER_BUILDKIT);
envname!(DOCKER_CONTEXT);
envname!(DOCKER_HOST);
envname!(OUT_DIR); // cargo setting we read
envname!(PATH);
envname!(RUSTC_WRAPPER); // cargo setting we read/write
envname!(RUSTUP_TOOLCHAIN); // rustup setting we read

const OURS: &[&str] = &[
    CARGOGREEN_LOG_PATH!(),
    CARGOGREEN_LOG!(),
    CARGOGREEN_LOG_STYLE!(),
    CARGOGREEN_RUNNER!(),
    CARGOGREEN_BUILDER_IMAGE!(),
    CARGOGREEN_SYNTAX_IMAGE!(),
    CARGOGREEN_REGISTRY_MIRRORS!(),
    CARGOGREEN_CACHE_IMAGES!(),
    CARGOGREEN_CACHE_FROM_IMAGES!(),
    CARGOGREEN_CACHE_TO_IMAGES!(),
    CARGOGREEN_FINAL_PATH!(),
    CARGOGREEN_BASE_IMAGE!(),
    CARGOGREEN_SET_ENVS!(),
    CARGOGREEN_WITH_NETWORK!(),
    CARGOGREEN_COMPONENTS!(),
    CARGOGREEN_ADD_APT!(),
    CARGOGREEN_ADD_APK!(),
    CARGOGREEN_EXPERIMENT!(),
];

pub(crate) const PREFIX: &str = concat!(CARGOGREEN!(), "_");

pub(crate) fn find_unknowns(vars: &Vars) -> Vec<&str> {
    let ours = OURS.iter().copied().collect::<HashSet<_>>();
    vars.keys()
        .filter(|var| var.starts_with(PREFIX))
        .filter(|var| !ours.contains(var.as_str()))
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
}
