use std::{collections::HashSet, env::vars};

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

envname!(BUILDX_BUILDER); // buildx'
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

pub(crate) fn find_unknowns() -> Vec<String> {
    let ours = OURS.iter().collect::<HashSet<_>>();
    vars()
        .map(|(var, _)| var)
        .filter(|var| var.starts_with(PREFIX))
        .filter(|var| !ours.contains(&var.as_str()))
        .collect::<Vec<_>>()
}
