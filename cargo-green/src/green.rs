use anyhow::{anyhow, bail, Context, Result};
use cargo_toml::Manifest;
use serde::Deserialize;

use crate::lockfile::find_package_and_workspace_tomls;

// use std::fmt;

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

//////////////////////////////////////////////////

#[derive(Debug, Deserialize)]
struct GreenMetadata {
    green: Green,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Green {
    // Sets the base Rust image for root package and all dependencies, unless themselves being configured differently.
    // See also:
    //   `additional-build-arguments`
    // In order to avoid unexpected changes, you may want to pin the image using an immutable digest.
    // Note that carefully crafting crossplatform stages can be non-trivial.
    //
    //
    // base-image-inline = """
    // FROM --platform=$BUILDPLATFORM rust:1 AS rust
    // RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
    // RUN --mount=type=secret,id=aws
    // """
    pub(crate) base_image_inline: String,
}

impl Green {
    // TODO: handle worskpace cfg + merging fields
    pub(crate) async fn try_new() -> Result<Self> {
        let (_todo, manifest_path) = find_package_and_workspace_tomls().await?;

        let manifest =
            Manifest::from_path(&manifest_path) //hmmmm this searches workspace tho
                .with_context(|| anyhow!("Reading package manifest {manifest_path}"))?;

        Self::try_from_manifest(&manifest)
    }

    pub(crate) fn try_from_manifest(manifest: &Manifest) -> Result<Self> {
        let green =
            if let Some(metadata) = manifest.package.as_ref().and_then(|x| x.metadata.as_ref()) {
                let GreenMetadata { green } = toml::from_str(&toml::to_string(metadata)?)?;
                green
            } else {
                Self::default()
            };

        // if let Some(val) =            env::var("CARGOGREEN_BASE_IMAGE").ok().and_then(|val| (!val.is_empty()).then_some(val))        {        }

        if !green.base_image_inline.is_empty() {
            if ["#syntax=", "# syntax=", "#syntax =", "# syntax ="]
                .iter()
                .any(|x| green.base_image_inline.starts_with(x))
            {
                bail!("[metadata.green.base-image-inline] must not override syntax")
            }

            // TODO: drop this requirement by allowing a `base-image-stage` override
            if !green.base_image_inline.contains(" AS rust\n")
                && !green.base_image_inline.contains(" as rust\n")
            {
                bail!("[metadata.green.base-image-inline] must provide a stage named 'rust'")
            }
        }

        Ok(green)
    }
}

#[cfg(test)]
mod tests {
    use cargo_toml::Manifest;

    use super::Green;

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
        Green::try_from_manifest(&manifest).unwrap();
    }

    #[test]
    fn metadata_green_base_image_inline_ok() {
        let manifest = Manifest::from_str(
            r#"
[package]
name = "test-package"

[package.metadata.green]
base-image-inline = """
FROM rust:1 AS rust
RUN --mount=from=some-context,target=/tmp/some-context cp -r /tmp/some-context ./
RUN --mount=type=secret,id=aws
"""
"#,
        )
        .unwrap();
        let green = Green::try_from_manifest(&manifest).unwrap();
        assert!(!green.base_image_inline.is_empty());
    }

    #[test]
    fn metadata_green_base_image_inline_bad_syntax() {
        let manifest = Manifest::from_str(
        r#"
[package]
name = "test-package"

[package.metadata.green]
base-image-inline = """
# syntax = ghcr.io/reproducible-containers/buildkit-nix:v0.1.1@sha256:7d4c42a5c6baea2b21145589afa85e0862625e6779c89488987266b85e088021
FROM xyz AS rust
RUN exit 42
"""
"#,
    )
    .unwrap();
        let err = Green::try_from_manifest(&manifest).err().unwrap().to_string();
        assert!(err.contains("override"));
        assert!(err.contains("syntax"));
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
        let err = Green::try_from_manifest(&manifest).err().unwrap().to_string();
        assert!(err.contains("must provide"));
        assert!(err.contains("stage"));
        assert!(err.contains("'rust'"));
    }
}

//////////////////////////////////////////////////

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
