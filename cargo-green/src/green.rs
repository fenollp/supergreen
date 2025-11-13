use std::{
    collections::{HashMap, HashSet},
    env,
};

use anyhow::{anyhow, bail, Result};
use cargo_toml::Manifest;
use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::network::Network;
use crate::{
    add::Add, base_image::BaseImage, builder::Builder, cache::Cache, containerfile::Containerfile,
    image_uri::ImageUri, lockfile::find_manifest_path, r#final::Final, runner::Runner, stage::RST,
    ENV_RUNNER, PKG,
};

macro_rules! ENV_REGISTRY_MIRRORS {
    () => {
        "CARGOGREEN_REGISTRY_MIRRORS"
    };
}

macro_rules! ENV_SET_ENVS {
    () => {
        "CARGOGREEN_SET_ENVS"
    };
}

#[macro_export]
macro_rules! ENV_SYNTAX_IMAGE {
    () => {
        "CARGOGREEN_SYNTAX_IMAGE"
    };
}

// TODO? switch all envs to TOML: cargo --config 'build.rustdocflags = ["--html-in-header", "header.html"]' …

#[doc = include_str!("../docs/configuration.md")]
#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct Green {
    #[doc = include_str!(concat!("../docs/",ENV_RUNNER!(),".md"))]
    pub(crate) runner: Runner,

    /// Snapshot of runner's envs. Not user-settable.
    #[doc(hidden)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub(crate) runner_envs: HashMap<String, String>,

    #[serde(flatten)]
    pub(crate) builder: Builder,

    #[doc = include_str!(concat!("../docs/",ENV_SYNTAX_IMAGE!(),".md"))]
    pub(crate) syntax: ImageUri,

    #[doc = include_str!(concat!("../docs/",ENV_REGISTRY_MIRRORS!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) registry_mirrors: Vec<String>,

    #[serde(flatten)]
    pub(crate) cache: Cache,

    #[serde(flatten)]
    pub(crate) r#final: Final,

    #[serde(flatten)]
    pub(crate) base: BaseImage,

    #[doc = include_str!(concat!("../docs/",ENV_SET_ENVS!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) set_envs: Vec<String>,

    #[serde(skip_serializing_if = "Add::is_empty")]
    pub(crate) add: Add,

    #[doc = include_str!(concat!("../docs/",ENV_EXPERIMENT!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) experiment: Vec<String>,
}

impl Green {
    pub(crate) fn new_containerfile(&self) -> Containerfile {
        Containerfile::with_syntax(&self.syntax)
    }

    // TODO: handle worskpace cfg + merging fields
    // TODO: find a way to read cfg on `cargo install <non-local code>` cc https://github.com/rust-lang/cargo/issues/9700#issuecomment-2748617896
    pub(crate) fn new_from_env_then_manifest() -> Result<Self> {
        let manifest = if let Some(manifest_path) =
            find_manifest_path().map_err(|e| anyhow!("Can't find package manifest: {e}"))?
        {
            let manifest = Manifest::from_path(&manifest_path)
                .map_err(|e| anyhow!("Can't read package manifest {manifest_path}: {e}"))?;
            Some(manifest)
        } else {
            None
        };

        Self::try_new(manifest).map_err(|e| anyhow!("Failed reading {PKG} configuration: {e}"))
    }

    fn try_new(manifest: Option<Manifest>) -> Result<Self> {
        let mut green = Self::default();

        if let Some(Manifest {
            package: Some(cargo_toml::Package { metadata: Some(metadata), .. }),
            ..
        }) = manifest
        {
            #[derive(Deserialize, Default)]
            struct GreenMetadata {
                green: Option<Green>,
            }
            if let GreenMetadata { green: Some(from_manifest) } = metadata.try_into()? {
                green = from_manifest;
            }
        }

        let var = ENV_REGISTRY_MIRRORS!();
        let mut origin = setting(var);
        let mut was_reset = false;
        if let Ok(val) = env::var(var) {
            origin = format!("${var}");
            if val.is_empty() {
                was_reset = true;
                green.registry_mirrors = vec![];
            } else {
                green.registry_mirrors = parse_csv(&val);
            }
        }
        if green.registry_mirrors.len()
            != green.registry_mirrors.iter().collect::<HashSet<_>>().len()
        {
            bail!("{origin} contains duplicates")
        }
        if green.registry_mirrors.is_empty() && !was_reset {
            // Hit me if you have more!
            green.registry_mirrors =
                vec!["mirror.gcr.io".to_owned(), "public.ecr.aws/docker".to_owned()];
        }

        for (field, var) in [
            (&mut green.cache.from_images, ENV_CACHE_FROM_IMAGES!()),
            (&mut green.cache.to_images, ENV_CACHE_TO_IMAGES!()),
            (&mut green.cache.images, ENV_CACHE_IMAGES!()),
        ] {
            let mut origin = setting(var);
            if let Ok(val) = env::var(var) {
                origin = format!("${var}");
                *field = val
                    .split(',')
                    .map(|x| ImageUri::try_new(x).map_err(|e| anyhow!("{origin} {e}")))
                    .collect::<Result<_>>()?;
            }
            if field.len() != field.iter().collect::<HashSet<_>>().len() {
                bail!("{origin} contains duplicates")
            }
            for item in field {
                if !item.noscheme().contains('/') {
                    bail!("{origin} must contain a registry and namespace: {item:?}")
                }
                if item.tagged() || item.locked() {
                    bail!("{origin} must not contain a tag nor digest: {item:?}")
                }
            }
        }

        let var = ENV_BASE_IMAGE!();
        if let Ok(val) = env::var(var) {
            let val = val.try_into().map_err(|e| anyhow!("${var} {e}"))?;
            green.base = BaseImage::from_image(val);
        }

        let var = ENV_BASE_IMAGE_INLINE!();
        let mut origin = setting(var);
        if let Ok(val) = env::var(var) {
            origin = format!("${var}");
            green.base.image_inline = Some(val);
        }
        if let Some(ref base_image_inline) = green.base.image_inline {
            if base_image_inline.is_empty() {
                bail!("{origin} is empty")
            }
            // TODO: drop this requirement by allowing a `base-image-stage` override
            //FIXME: have to repeat base stage per stage actually => no naming constraint then anyway
            if !base_image_inline.to_lowercase().contains(&format!(" AS {RST}\n").to_lowercase()) {
                bail!("{origin} does not provide a stage named '{RST}'")
            }
        }

        if let Some(ref base_image_inline) = green.base.image_inline {
            let base = green.base.image.noscheme();
            if base.is_empty() || !base_image_inline.contains(&format!(" {base} ")) {
                bail!(
                    "Make sure to match {} ({base}) with the image URL used in {}",
                    setting(ENV_BASE_IMAGE!()),
                    setting(ENV_BASE_IMAGE_INLINE!())
                )
            }
        }
        if green.base.is_unset() {
            //CARGOGREEN_USE=<a rustup toolchain>
            //CARGOGREEN_TOOLCHAIN=<a rustup toolchain> MOUCH BETTA
            // #TODO: CARGOGREEN_COMPONENT=toolchain=,target=,add=llvm-tools-preview;remove=
            // https://rust-lang.github.io/rustup/concepts/toolchains.html#toolchain-specification
            // if set use it, else:
            green.base = BaseImage::from_local_rustc();
        }

        validate_csv(&mut green.set_envs, ENV_SET_ENVS!())?;
        if green.set_envs.iter().any(|var| var.starts_with("CARGOGREEN_")) {
            bail!("{origin} contains CARGOGREEN_* names")
        }

        for (field, var) in [
            (&mut green.add.apk, ENV_ADD_APK!()),
            (&mut green.add.apt, ENV_ADD_APT!()),
            (&mut green.add.apt_get, ENV_ADD_APT_GET!()),
        ] {
            validate_csv(field, var)?;
        }

        Ok(green)
    }
}

fn env_as_toml(var: &str) -> String {
    var.replace("CARGOGREEN_", "").replace('_', "-").to_lowercase()
}

fn setting(var: &str) -> String {
    format!("[metadata.green.{}]", env_as_toml(var))
}

fn parse_csv(val: &str) -> Vec<String> {
    val.split(',').map(ToOwned::to_owned).collect()
}

pub(crate) fn validate_csv(field: &mut Vec<String>, var: &'static str) -> Result<()> {
    let mut origin = setting(var);
    if let Ok(val) = env::var(var) {
        origin = format!("${var}");
        if val.is_empty() {
            bail!("{origin} is empty")
        }

        *field = parse_csv(&val);
    }
    if !field.is_empty() {
        let bad_chars = [' ', '\'', '"', ';'];
        if field.iter().any(|x| x.is_empty() || x.contains(bad_chars) || x.trim() != x) {
            bail!("{origin} contains empty names, quotes, semicolons or whitespace")
        }

        if field.len() != field.iter().collect::<HashSet<_>>().len() {
            bail!("{origin} contains duplicates")
        }
    }
    Ok(())
}

#[cfg(test)]
#[test_case::test_matrix(["", "[package.metadata.green]", "[package.metadata.other]"])]
fn metadata_green_ok(conf: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

{conf}
"#
    ))
    .unwrap();
    let mut green = Green::try_new(Some(manifest)).unwrap();

    assert!(!green.base.image.is_empty());
    green.base.image = ImageUri::default();
    assert!(green.base.image.is_empty());

    assert!(!green.registry_mirrors.is_empty());
    green.registry_mirrors = vec![];
    assert!(green.registry_mirrors.is_empty());

    assert_eq!(green, Green::default());
}

//

#[test]
fn metadata_green_add_ok() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
add.apt = [ "libpq-dev", "pkg-config" ]
add.apt-get = [ "libpq-dev", "pkg-config" ]
add.apk = [ "libpq-dev", "pkgconf" ]
"#,
    )
    .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(green.add.apt, vec!["libpq-dev".to_owned(), "pkg-config".to_owned()]);
    assert_eq!(green.add.apt_get, vec!["libpq-dev".to_owned(), "pkg-config".to_owned()]);
    assert_eq!(green.add.apk, vec!["libpq-dev".to_owned(), "pkgconf".to_owned()]);
}

#[cfg(test)]
#[test_case::test_matrix(["apt", "apt-get", "apk"])]
fn metadata_green_add_empty_name(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "" ]
"#
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("empty"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["apt", "apt-get", "apk"])]
fn metadata_green_add_quotes(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "'a'" ]
"#
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("quotes"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["apt", "apt-get", "apk"])]
fn metadata_green_add_whitespace(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "a b" ]
"#
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("space"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["apt", "apt-get", "apk"])]
fn metadata_green_add_duplicates(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
add.{setting} = [ "a", "b", "a" ]
            "#
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("duplicates"), "In: {err}");
}

//

#[test]
fn metadata_green_set_envs_ok() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
"#,
    )
    .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(
        green.set_envs,
        vec![
            "GIT_AUTH_TOKEN".to_owned(),
            "TYPENUM_BUILD_CONSTS".to_owned(),
            "TYPENUM_BUILD_OP".to_owned()
        ]
    );
}

#[test]
fn metadata_green_set_envs_empty_var() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "" ]
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("empty name"), "In: {err}");
}

#[test]
fn metadata_green_set_envs_quotes() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "'a'" ]
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("quotes"), "In: {err}");
}

#[test]
fn metadata_green_set_envs_whitespace() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "A B" ]
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("space"), "In: {err}");
}

#[test]
fn metadata_green_set_envs_our_vars() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "CARGOGREEN_LOG" ]
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("CARGOGREEN"), "In: {err}");
}

#[test]
fn metadata_green_set_envs_duplicates() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
set-envs = [ "A", "B", "A" ]
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("duplicates"), "In: {err}");
}

//

#[test]
fn metadata_green_base_image_ok() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker-image://docker.io/library/rust:1"
"#,
    )
    .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(
        green.base,
        BaseImage {
            image: ImageUri::std("rust:1"),
            image_inline: None,
            with_network: Network::None,
        }
    );
}

#[test]
fn metadata_green_base_image_bad_scheme() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker.io/library/rust:1"
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("scheme"), "In: {err}");
}

#[test]
fn metadata_green_base_image_whitespace() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = " docker-image://docker.io/library/rust:1  "
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("space"), "In: {err}");
}

#[test]
fn metadata_green_base_image_and_inline() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker-image://docker.io/library/rust:1"
base-image-inline = """
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("to match"), "In: {err}");
}

//

#[test]
fn metadata_green_base_image_inline_ok() {
    let manifest = Manifest::from_str(
            r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker-image://rust:1"
base-image-inline = """
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021 <-- gets ignored
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
        )
        .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(green.base, BaseImage {
        image: ImageUri::try_new("docker-image://rust:1").unwrap(),
        image_inline:
            Some(
                r#"
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021 <-- gets ignored
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"#[1..]
                    .to_owned()
            ),
            with_network: Network::None,
        });
}

#[test]
fn metadata_green_base_image_inline_with_network_ok() {
    let manifest = Manifest::from_str(
            r#"
[package]
name = "test-package"

[package.metadata.green]
base-image = "docker-image://rust:1"
with-network = "default"
base-image-inline = """
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021 <-- gets ignored
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
        )
        .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(green.base, BaseImage {
        image: ImageUri::try_new("docker-image://rust:1").unwrap(),
        image_inline:
            Some(
                r#"
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021 <-- gets ignored
FROM rust:1 AS rust-base
RUN --mount=from=some-context,dst=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"#[1..]
                    .to_owned()
            ),
            with_network: Network::Default,
        });
}

#[test]
fn metadata_green_base_image_inline_empty() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image-inline = ""
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("empty"), "In: {err}");
}

#[test]
fn metadata_green_base_image_inline_bad_stage() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image-inline = """
FROM xyz AS not-rust
RUN exit 42
"""
"#,
    )
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("provide"), "In: {err}");
    assert!(err.contains("stage"), "In: {err}");
    assert!(err.contains("'rust-base'"), "In: {err}");
}

//

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_ok(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = [
  "docker-image://some-registry.com/dir/image",
  "docker-image://other.registry/dir2/image3",
]
"#,
    ))
    .unwrap();
    let green = Green::try_new(Some(manifest)).unwrap();
    assert_eq!(
        match setting {
            "cache-images" => green.cache.images,
            "cache-from-images" => green.cache.from_images,
            "cache-to-images" => green.cache.to_images,
            _ => unreachable!(),
        },
        vec![
            ImageUri::try_new("docker-image://some-registry.com/dir/image").unwrap(),
            ImageUri::try_new("docker-image://other.registry/dir2/image3").unwrap(),
        ]
    );
}

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_dupes(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = [
  "docker-image://some-registry.com/dir/image",
  "docker-image://other.registry/dir2/image3",
  "docker-image://some-registry.com/dir/image",
]
"#,
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("duplicates"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_bad_names(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://some-registry.com/dir/image 'docker-image://other.registry/dir2/image3'", ""]
"#,
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("names"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_bad_scheme(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["some-registry.com/dir/image"]
"#,
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("scheme"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_bad_registry(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://image"]
"#,
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("registry"), "In: {err}");
}

#[cfg(test)]
#[test_case::test_matrix(["cache-images", "cache-from-images", "cache-to-images"])]
fn metadata_green_cache_images_bad_image(setting: &str) {
    let manifest = Manifest::from_str(&format!(
        r#"
[package]
name = "test-package"

[package.metadata.green]
{setting} = ["docker-image://some-registry.com/dir/image:sometag"]
"#,
    ))
    .unwrap();
    let err = Green::try_new(Some(manifest)).err().unwrap().to_string();
    assert!(err.contains("tag"), "In: {err}");
}

//

// from https://github.com/PRQL/prql/pull/3773/files
// [profile.release.package.prql-compiler]
// strip = "debuginfo"
//=> look into how `[profile.release.package.PACKAGE]` settings are propagated

// TODO: cli config / profiles https://github.com/rust-lang/cargo/wiki/Third-party-cargo-subcommands
//   * https://docs.rs/figment/latest/figment/
//   * https://lib.rs/crates/toml_edit
//   * https://github.com/jdrouet/serde-toml-merge
//   * https://crates.io/crates/toml-merge
// https://github.com/cbourjau/cargo-with
// https://github.com/RazrFalcon/cargo-bloat
// https://lib.rs/crates/cargo_metadata
// https://github.com/stormshield/cargo-ft/blob/d4ba5b048345ab4b21f7992cc6ed12afff7cc863/src/package/metadata.rs

//

// use error_stack::{report, Context, ResultExt};
// use serde::{
//     de,
//     de::{MapAccess, SeqAccess},
//     Deserialize, Deserializer,
// };

// #[derive(Debug, Clone, Eq, PartialEq, Parser)]
// #[command(no_binary_name = true)]
// #[command(styles = clap_cargo::style::CLAP_STYLING)]
// #[group(skip)]
// pub(crate) struct GreenCli {
//     #[command(flatten)]
//     pub manifest: clap_cargo::Manifest,
// }

// #[cfg(test)]
// mod tests;

// type MetadataResult<T> = error_stack::Result<T, ParseMetadataError>;

// #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
// pub enum ParseMetadataError {
//     Missing,
//     Invalid,
// }

// impl fmt::Display for ParseMetadataError {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         f.pad(match self {
//             Self::Missing => "missing metadata",
//             Self::Invalid => "invalid metadata",
//         })
//     }
// }

// impl Context for ParseMetadataError {}

// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
// pub struct FtWorkspaceMetadata {
//     fields: InheritableFields,
// }

// impl FtWorkspaceMetadata {
//     pub fn parse(metadata: serde_json::Value) -> MetadataResult<Self> {
//         let metadata = serde_json::from_value::<Option<JsonWorkspaceMetadata>>(metadata)
//             .change_context(ParseMetadataError::Invalid)?;

//         let fields = metadata.unwrap_or_default().ft.unwrap_or_default();

//         Ok(Self { fields })
//     }
// }

// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
// pub struct FtMetadata {
//     targets: Vec<String>,
// }

// impl FtMetadata {
//     pub fn parse(
//         workspace_metadata: &FtWorkspaceMetadata,
//         package_metadata: serde_json::Value,
//     ) -> MetadataResult<Self> {
//         let package_metadata =
//             serde_json::from_value::<Option<JsonPackageMetadata>>(package_metadata)
//                 .change_context(ParseMetadataError::Invalid)?
//                 .ok_or(ParseMetadataError::Missing)
//                 .attach_printable("no `package.metadata` table")?;

//         let ft = package_metadata
//             .ft
//             .ok_or(ParseMetadataError::Missing)
//             .attach_printable("no `package.metadata.ft` table")?;

//         let targets = ft
//             .targets
//             .ok_or(ParseMetadataError::Missing)
//             .attach_printable("no `package.metadata.ft.targets` array")?
//             .resolve("targets", || workspace_metadata.fields.targets())?;

//         Ok(Self { targets })
//     }

//     pub fn targets(&self) -> impl ExactSizeIterator<Item = &str> {
//         self.targets.iter().map(AsRef::as_ref)
//     }
// }

// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// struct JsonWorkspaceMetadata {
//     ft: Option<InheritableFields>,
// }

// // Group of fields which members of the workspace can inherit
// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// struct InheritableFields {
//     targets: Option<Vec<String>>,
// }

// impl InheritableFields {
//     fn targets(&self) -> MetadataResult<Vec<String>> {
//         self.targets
//             .as_ref()
//             .cloned()
//             .ok_or(report!(ParseMetadataError::Invalid))
//             .attach_printable("`workspace.metadata.ft.targets` was not defined")
//     }
// }

// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// struct JsonPackageMetadata {
//     ft: Option<JsonPackageMetadataFt>,
// }

// #[derive(Debug, Clone, Eq, PartialEq, Hash, Default, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// struct JsonPackageMetadataFt {
//     targets: Option<MaybeWorkspace<Vec<String>>>,
// }

// #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
// enum MaybeWorkspace<T> {
//     Defined(T),
//     Workspace,
// }

// impl<T: Default> Default for MaybeWorkspace<T> {
//     fn default() -> Self {
//         Self::Defined(T::default())
//     }
// }

// impl<'de> Deserialize<'de> for MaybeWorkspace<Vec<String>> {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: Deserializer<'de>,
//     {
//         struct Visitor;

//         impl<'de> de::Visitor<'de> for Visitor {
//             type Value = MaybeWorkspace<Vec<String>>;

//             fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
//                 formatter.pad("a sequence of strings or workspace")
//             }

//             fn visit_seq<A>(self, seq: A) -> Result<Self::Value, A::Error>
//             where
//                 A: SeqAccess<'de>,
//             {
//                 let deserializer = de::value::SeqAccessDeserializer::new(seq);
//                 Vec::deserialize(deserializer).map(MaybeWorkspace::Defined)
//             }

//             fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
//             where
//                 A: MapAccess<'de>,
//             {
//                 let deserializer = de::value::MapAccessDeserializer::new(map);
//                 JsonWorkspaceField::deserialize(deserializer)?;

//                 Ok(MaybeWorkspace::Workspace)
//             }
//         }

//         deserializer.deserialize_any(Visitor)
//     }
// }

// impl<T> MaybeWorkspace<T> {
//     fn resolve(
//         self,
//         label: &str,
//         get_workspace_inheritable: impl FnOnce() -> MetadataResult<T>,
//     ) -> MetadataResult<T> {
//         match self {
//             Self::Defined(value) => Ok(value),
//             Self::Workspace => get_workspace_inheritable().attach_printable_lazy(|| format!("error inheriting `{label}` from workspace root manifest's `workspace.metadata.ft.{label}`")),
//         }
//     }
// }

// #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default, Deserialize)]
// #[serde(rename_all = "kebab-case")]
// struct JsonWorkspaceField {
//     #[serde(deserialize_with = "bool_true")]
//     workspace: bool,
// }

// fn bool_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
// where
//     D: Deserializer<'de>,
// {
//     if bool::deserialize(deserializer)? {
//         Ok(true)
//     } else {
//         Err(de::Error::custom("`workspace` cannot be false"))
//     }
// }
