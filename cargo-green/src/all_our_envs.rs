use std::{collections::HashSet, env::vars};

const OURS: &[&str] = &[
    ENV_LOG_PATH!(),
    ENV_LOG!(),
    ENV_LOG_STYLE!(),
    ENV_RUNNER!(),
    ENV_BUILDER_IMAGE!(),
    ENV_SYNTAX_IMAGE!(),
    ENV_REGISTRY_MIRRORS!(),
    ENV_CACHE_IMAGES!(),
    ENV_CACHE_FROM_IMAGES!(),
    ENV_CACHE_TO_IMAGES!(),
    ENV_FINAL_PATH!(),
    ENV_BASE_IMAGE!(),
    ENV_SET_ENVS!(),
    ENV_WITH_NETWORK!(),
    ENV_COMPONENTS!(),
    ENV_ADD_APT!(),
    ENV_ADD_APK!(),
    ENV_EXPERIMENT!(),
];

pub(crate) fn find_unknowns() -> Vec<String> {
    let ours = OURS.iter().collect::<HashSet<_>>();
    vars()
        .map(|(var, _)| var)
        .filter(|var| var.starts_with(concat!(ENV!(), "_")))
        .filter(|var| !ours.contains(&var.as_str()))
        .collect::<Vec<_>>()
}
