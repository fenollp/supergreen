use std::{collections::HashSet, env};

use anyhow::{anyhow, bail, Result};
use camino::Utf8PathBuf;
use cargo_toml::Manifest;
use serde::{Deserialize, Serialize};

use crate::{
    base::{BaseImage, RUST},
    lockfile::find_manifest_path,
};

// Envs that override Cargo.toml settings
pub(crate) const ENV_ADD_APK: &str = "CARGOGREEN_ADD_APK";
pub(crate) const ENV_ADD_APT: &str = "CARGOGREEN_ADD_APT";
pub(crate) const ENV_ADD_APT_GET: &str = "CARGOGREEN_ADD_APT_GET";
pub(crate) const ENV_BASE_IMAGE: &str = "CARGOGREEN_BASE_IMAGE";
pub(crate) const ENV_BASE_IMAGE_INLINE: &str = "CARGOGREEN_BASE_IMAGE_INLINE";
pub(crate) const ENV_SET_ENVS: &str = "CARGOGREEN_SET_ENVS";

/// Settings for building this package with `cargo-green`
#[derive(Debug, Deserialize)]
struct GreenMetadata {
    green: Green,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Add {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    apk: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    apt: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    apt_get: Vec<String>,
}

impl Add {
    #[must_use]
    pub(crate) fn with_apt(pkgs: &[&str]) -> Self {
        Self { apt: pkgs.iter().map(|&x| x.to_owned()).collect(), ..Default::default() }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        let Self { apk, apt, apt_get } = self;
        [apk, apt, apt_get].iter().all(|x| x.is_empty())
    }

    // TODO: pin major + lock by pulling
    // TODO: more architectures
    pub(crate) fn as_block(&self, last: &str) -> String {
        const XX: &str = "docker.io/tonistiigi/xx:1.6.1@sha256:923441d7c25f1e2eb5789f82d987693c47b8ed987c4ab3b075d6ed2b5d6779a3";
        format!(
            r#"
FROM --platform=$BUILDPLATFORM {XX} AS xx
{last}
ARG TARGETPLATFORM
RUN \
  --mount=from=xx,source=/usr/bin/xx-apk,target=/usr/bin/xx-apk \
  --mount=from=xx,source=/usr/bin/xx-apt,target=/usr/bin/xx-apt \
  --mount=from=xx,source=/usr/bin/xx-apt,target=/usr/bin/xx-apt-get \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-c++ \
  --mount=from=xx,source=/usr/bin/xx-cargo,target=/usr/bin/xx-cargo \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-cc \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-clang \
  --mount=from=xx,source=/usr/bin/xx-cc,target=/usr/bin/xx-clang++ \
  --mount=from=xx,source=/usr/bin/xx-go,target=/usr/bin/xx-go \
  --mount=from=xx,source=/usr/bin/xx-info,target=/usr/bin/xx-info \
  --mount=from=xx,source=/usr/bin/xx-ld-shas,target=/usr/bin/xx-ld-shas \
  --mount=from=xx,source=/usr/bin/xx-verify,target=/usr/bin/xx-verify \
  --mount=from=xx,source=/usr/bin/xx-windres,target=/usr/bin/xx-windres \
    set -eux \
 && if   command -v apk >/dev/null 2>&1; then \
                                     xx-apk     add     --no-cache                 {apk}; \
    elif command -v apt >/dev/null 2&>1; then \
      DEBIAN_FRONTEND=noninteractive xx-apt     install --no-install-recommends -y {apt}; \
    else \
      DEBIAN_FRONTEND=noninteractive xx-apt-get install --no-install-recommends -y {apt_get}; \
    fi
"#,
            last = last.trim(),
            apk = self.apk.join(" "),
            apt = self.apt.join(" "),
            apt_get = self.apt_get.join(" "),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Green {
    // Pick which executor to use: "docker" (default), "podman" or "none".
    //
    // # Use by setting this environment variable (no Cargo.toml setting):
    // CARGOGREEN_RUNNER="docker"
    pub(crate) runner: String, //TODO: type(enum)

    // Sets which BuildKit frontend syntax to use.
    //
    // See https://docs.docker.com/build/buildkit/frontend/#stable-channel
    //
    // # Use by setting this environment variable (no Cargo.toml setting):
    // CARGOGREEN_SYNTAX="docker-image://docker.io/docker/dockerfile:1"
    pub(crate) syntax: String, //TODO? type(uri?)

    // Sets which BuildKit builder to use.
    //
    // See https://docs.docker.com/build/builders/
    //
    // # Use by setting this environment variable (no Cargo.toml setting):
    // CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:latest"
    // CARGOGREEN_BUILDER_IMAGE="docker-image://docker.io/moby/buildkit:buildx-stable-1"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) builder_image: Option<String>, //TODO? type(uri?)

    // Write final Dockerfile to given path.
    //
    // Helps e.g. create a Dockerfile with caching for dependencies.
    //
    // # Use by setting this environment variable (no Cargo.toml setting):
    // CARGOGREEN_FINAL_PATH="$PWD/my-bin@1.0.0.Dockerfile"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) final_path: Option<Utf8PathBuf>,

    #[serde(flatten)]
    pub(crate) image: BaseImage,

    // Pass environment variables through to build runner.
    // $CARGOGREEN_SET_ENVS overrides this setting.
    // See also:
    //   `packages`
    // May be useful if a build script exported some vars that a package then reads.
    // About $GIT_AUTH_TOKEN: https://docs.docker.com/build/building/secrets/#git-authentication-for-remote-contexts
    //
    // set-envs = [ "GIT_AUTH_TOKEN", "TYPENUM_BUILD_CONSTS", "TYPENUM_BUILD_OP" ]
    //
    // # This environment variable takes precedence over any Cargo.toml settings:
    // CARGOGREEN_SET_ENVS="[\"GIT_AUTH_TOKEN\", \"TYPENUM_BUILD_CONSTS\", \"TYPENUM_BUILD_OP\"]"
    //
    // NOTE: this doesn't (yet) accumulate dependencies' set-envs values!
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) set_envs: Vec<String>,

    // add.apt = [ "libpq-dev", "pkg-config" ]
    // add.apk = [ "libpq-dev", "pkgconf" ]
    //
    // # These environment variables take precedence over any Cargo.toml settings:
    // CARGOGREEN_ADD_APT='[ "libpq-dev", "pkg-config" ]'
    // FIXME: ===> use CSV instead of JSON
    #[serde(skip_serializing_if = "Add::is_empty")]
    pub(crate) add: Add,
}

impl Green {
    // TODO: handle worskpace cfg + merging fields
    // TODO: find a way to read cfg on `cargo install <non-local code>` cc https://github.com/rust-lang/cargo/issues/9700#issuecomment-2748617896
    pub(crate) fn new_from_env_then_manifest() -> Result<Self> {
        let manifest_path = find_manifest_path()?;

        let manifest = Manifest::from_path(&manifest_path)
            .map_err(|e| anyhow!("Can't read package manifest {manifest_path}: {e}"))?;

        Self::try_new(manifest)
    }

    fn try_new(manifest: Manifest) -> Result<Self> {
        let mut green = Self::default();
        if let Some(metadata) = manifest.package.and_then(|x| x.metadata) {
            let GreenMetadata { green: from_manifest } = metadata.try_into()?;
            green = from_manifest;
        }

        let mut origin = "[metadata.green.base-image]".to_owned();
        if let Ok(val) = env::var(ENV_BASE_IMAGE) {
            origin = format!("${ENV_BASE_IMAGE}");
            green.image = BaseImage::from_image(val);
        }
        if !green.image.base_image.is_empty() {
            if green.image.base_image != green.image.base_image.trim() {
                bail!("{origin} has leading or trainling whitespace: {:?}", green.image.base_image)
            }
            if !green.image.base_image.starts_with("docker-image://") {
                bail!("{origin} unsupported scheme: {:?}", green.image.base_image)
            }
        }

        let mut origin = "[metadata.green.base-image-inline]".to_owned();
        if let Ok(val) = env::var(ENV_BASE_IMAGE_INLINE) {
            origin = format!("${ENV_BASE_IMAGE_INLINE}");
            green.image.base_image_inline = Some(val);
        }
        if let Some(ref base_image_inline) = green.image.base_image_inline {
            if base_image_inline.is_empty() {
                bail!("{origin} is empty")
            }
            // TODO: drop this requirement by allowing a `base-image-stage` override
            //FIXME: have to repeat base stage per stage actually => no naming constraint then anyway
            if !base_image_inline.contains(&format!(" AS {RUST}\n"))
                && !base_image_inline.contains(&format!(" as {RUST}\n"))
            {
                bail!("{origin} does not provide a stage named '{RUST}'")
            }
        }

        if let Some(ref base_image_inline) = green.image.base_image_inline {
            let base = green.image.base_image.trim_start_matches("docker-image://");
            if base.is_empty() || !base_image_inline.contains(&format!(" {base} ")) {
                bail!("Make sure to match [metadata.green.base-image] with the image URL used in [metadata.green.base-image-inline]")
            }
        }
        if green.image.is_unset() {
            green.image = BaseImage::from_local_rustc();
        }

        let mut origin = "[metadata.green.set-envs]".to_owned();
        if let Ok(val) = env::var(ENV_SET_ENVS) {
            origin = format!("${ENV_SET_ENVS}");
            if val.is_empty() {
                bail!("{origin} is empty")
            }
            green.set_envs =
                serde_json::from_str(&val).map_err(|e| anyhow!("Failed parsing {origin}: {e}"))?;
        }
        if !green.set_envs.is_empty() {
            if green.set_envs.iter().any(String::is_empty) {
                bail!("{origin} contains empty names")
            }
            if green.set_envs.iter().any(|var| var.starts_with("CARGOGREEN_")) {
                bail!("{origin} contains CARGOGREEN_* names")
            }
            if green.set_envs.iter().any(|var| var.starts_with("RUSTCBUILDX_")) {
                bail!("{origin} contains RUSTCBUILDX_* names")
            }
            if green.set_envs.len() != green.set_envs.iter().collect::<HashSet<_>>().len() {
                bail!("{origin} contains duplicates")
            }
        }

        for (field, (var, setting)) in [
            (&mut green.add.apk, (ENV_ADD_APK, "apk")),
            (&mut green.add.apt, (ENV_ADD_APT, "apt")),
            (&mut green.add.apt_get, (ENV_ADD_APT_GET, "apt-get")),
        ] {
            let mut origin = format!("[metadata.green.add.{setting}]");
            if let Ok(val) = env::var(var) {
                origin = format!("${var}");
                if val.is_empty() {
                    bail!("{origin} is empty")
                }
                *field = serde_json::from_str(&val)
                    .map_err(|e| anyhow!("Failed parsing {origin}: {e}"))?;
            }
            if !field.is_empty() {
                if field.iter().any(String::is_empty) {
                    bail!("{origin} contains empty names")
                }
                if field.len() != field.iter().collect::<HashSet<_>>().len() {
                    bail!("{origin} contains duplicates")
                }
            }
        }

        Ok(green)
    }
}

#[test]
fn metadata_green_ok() {
    let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
"#,
    )
    .unwrap();
    Green::try_new(manifest).unwrap();
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
    let green = Green::try_new(manifest).unwrap();
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
    assert!(err.contains("empty"), "In: {err}");
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
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
    let green = Green::try_new(manifest).unwrap();
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
    assert!(err.contains("empty name"), "In: {err}");
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
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
    let green = Green::try_new(manifest).unwrap();
    assert_eq!(green.image.base_image, "docker-image://docker.io/library/rust:1");
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
    assert!(err.contains("scheme"), "In: {err}");
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
RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
    )
    .unwrap();
    let err = Green::try_new(manifest).err().unwrap().to_string();
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
RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
        )
        .unwrap();
    let green = Green::try_new(manifest).unwrap();
    assert_eq!(
            green.image.base_image_inline,
            Some(
                r#"
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021 <-- gets ignored
FROM rust:1 AS rust-base
RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"#[1..]
                    .to_owned()
            )
        );
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
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
    let err = Green::try_new(manifest).err().unwrap().to_string();
    assert!(err.contains("provide"), "In: {err}");
    assert!(err.contains("stage"), "In: {err}");
    assert!(err.contains("'rust-base'"), "In: {err}");
}

//////////////////////////////////////////////////
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

//////////////////////////////////////////////////

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

// /// Group of fields which members of the workspace can inherit
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
