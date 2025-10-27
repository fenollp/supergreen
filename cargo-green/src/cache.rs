use serde::{Deserialize, Serialize};

use crate::image_uri::ImageUri;

macro_rules! ENV_CACHE_FROM_IMAGES {
    () => {
        "CARGOGREEN_CACHE_FROM_IMAGES"
    };
}

macro_rules! ENV_CACHE_IMAGES {
    () => {
        "CARGOGREEN_CACHE_IMAGES"
    };
}

macro_rules! ENV_CACHE_TO_IMAGES {
    () => {
        "CARGOGREEN_CACHE_TO_IMAGES"
    };
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cache {
    #[doc = include_str!(concat!("../docs/",ENV_CACHE_FROM_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-from-images")]
    pub(crate) from_images: Vec<ImageUri>,

    #[doc = include_str!(concat!("../docs/",ENV_CACHE_TO_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-to-images")]
    pub(crate) to_images: Vec<ImageUri>,

    #[doc = include_str!(concat!("../docs/",ENV_CACHE_IMAGES!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "cache-images")]
    pub(crate) images: Vec<ImageUri>,
}
