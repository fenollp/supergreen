use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::{image_uri::ImageUri, network::Network};

/// Cross platform tools from <https://github.com/tonistiigi/xx>
pub(crate) static XX: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::try_new("docker-image://docker.io/tonistiigi/xx:latest").unwrap()); //fixme: env to lock image

// TODO: drop apt-get?
// maybe actually drop apt + turn api easier for "apt-get satisfy" or repro-get like

// TODO: To install a specific version of a package using apt-get, you use the following command: sudo apt-get install = . This command allows you to specify the package and the version you want to install. Here's a simple example: sudo apt-get install curl=7.58.21 May 2024
//
// https://github.com/fenollp/docker-from-deps/blob/ee1e638de8cd30ed438611ec89252244a0e3b5f7/to_dockerfile#L45
//
// * Suggest using `=` for apt
// * auto-search the web to add `=...`

macro_rules! ENV_ADD_APK {
    () => {
        "CARGOGREEN_ADD_APK"
    };
}

macro_rules! ENV_ADD_APT {
    () => {
        "CARGOGREEN_ADD_APT"
    };
}

macro_rules! ENV_ADD_APT_GET {
    () => {
        "CARGOGREEN_ADD_APT_GET"
    };
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Add {
    #[doc = include_str!(concat!("../docs/",ENV_ADD_APK!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apk: Vec<String>,

    #[doc = include_str!(concat!("../docs/",ENV_ADD_APT!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apt: Vec<String>,

    #[doc = include_str!(concat!("../docs/",ENV_ADD_APT_GET!(),".md"))]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apt_get: Vec<String>,
}

impl Add {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        let Self { apk, apt, apt_get } = self;
        [apk, apt, apt_get].iter().all(|x| x.is_empty())
    }

    pub(crate) fn as_block(&self, xx: ImageUri, last: &str) -> (Network, String) {
        let block = format!(
            r#"
FROM --platform=$BUILDPLATFORM {xx} AS xx
{last}
ARG TARGETPLATFORM
RUN \
  --mount=from=xx,source=/usr/bin/xx-apk,dst=/usr/bin/xx-apk \
  --mount=from=xx,source=/usr/bin/xx-apt,dst=/usr/bin/xx-apt \
  --mount=from=xx,source=/usr/bin/xx-apt,dst=/usr/bin/xx-apt-get \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-c++ \
  --mount=from=xx,source=/usr/bin/xx-cargo,dst=/usr/bin/xx-cargo \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-cc \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-clang \
  --mount=from=xx,source=/usr/bin/xx-cc,dst=/usr/bin/xx-clang++ \
  --mount=from=xx,source=/usr/bin/xx-go,dst=/usr/bin/xx-go \
  --mount=from=xx,source=/usr/bin/xx-info,dst=/usr/bin/xx-info \
  --mount=from=xx,source=/usr/bin/xx-ld-shas,dst=/usr/bin/xx-ld-shas \
  --mount=from=xx,source=/usr/bin/xx-verify,dst=/usr/bin/xx-verify \
  --mount=from=xx,source=/usr/bin/xx-windres,dst=/usr/bin/xx-windres \
    set -eux \
 && if   command -v apk >/dev/null 2>&1; then \
                                     xx-apk     add     --no-cache                 '{apk}'; \
    elif command -v apt >/dev/null 2>&1; then \
      DEBIAN_FRONTEND=noninteractive xx-apt     install --no-install-recommends -y '{apt}'; \
    else \
      DEBIAN_FRONTEND=noninteractive xx-apt-get install --no-install-recommends -y '{apt_get}'; \
    fi
"#,
            xx = xx.noscheme(),
            last = last.trim(),
            apk = quote_pkgs(&self.apk),
            apt = quote_pkgs(&self.apt),
            apt_get = quote_pkgs(&self.apt_get),
        );

        (Network::Default, block)
    }
}

fn quote_pkgs(pkgs: &[String]) -> String {
    if pkgs.is_empty() {
        "<none>".to_owned()
    } else {
        pkgs.join("' '")
    }
}
