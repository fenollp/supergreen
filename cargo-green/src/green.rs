use clap::Parser;

// use std::fmt;

// use error_stack::{report, Context, ResultExt};
// use serde::{
//     de,
//     de::{MapAccess, SeqAccess},
//     Deserialize, Deserializer,
// };

#[derive(Debug, Clone, Eq, PartialEq, Parser)]
#[command(no_binary_name = true)]
#[command(styles = clap_cargo::style::CLAP_STYLING)]
#[group(skip)]
pub(crate) struct GreenCli {
    #[command(flatten)]
    pub manifest: clap_cargo::Manifest,
}

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
