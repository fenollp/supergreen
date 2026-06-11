use serde::{Deserialize, Serialize};

use crate::network::Network;

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
    /// True when no OS packages are to be installed through apt/apk
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        let Self { apk, apt, apt_get } = self;
        [apk, apt, apt_get].iter().all(|x| x.is_empty())
    }

    /// Sums, sorts and dedupes multiple instances
    #[must_use]
    pub(crate) fn union(self, lhs: &Self) -> Self {
        let Self { mut apk, mut apt, mut apt_get } = self;
        apk.extend(lhs.apk.iter().cloned());
        apt.extend(lhs.apt.iter().cloned());
        apt_get.extend(lhs.apt_get.iter().cloned());
        apk.sort();
        apt.sort();
        apt_get.sort();
        apk.dedup();
        apt.dedup();
        apt_get.dedup();
        Self { apk, apt, apt_get }
    }

    // TODO: finer package installs per os/distro
    pub(crate) fn as_block(&self, last: &str) -> (Network, String) {
        if self.is_empty() {
            let block = format!("\n{last}\n", last = last.trim());
            return (Network::None, block);
        }

        // TODO: pin major + lock by pulling
        const XX: &str = "docker.io/tonistiigi/xx:1.6.1@sha256:923441d7c25f1e2eb5789f82d987693c47b8ed987c4ab3b075d6ed2b5d6779a3";

        // NOTE: `ARG TARGETPLATFORM` is needed by xx
        let block = format!(
            r#"
FROM --platform=$BUILDPLATFORM {XX} AS xx
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
      DEBIAN_FRONTEND=noninteractive xx-apt     satisfy --no-install-recommends -y '{apt}'; \
    else \
      DEBIAN_FRONTEND=noninteractive xx-apt-get satisfy --no-install-recommends -y '{apt_get}'; \
    fi
"#,
            last = last.trim(),
            apk = quote_pkgs(&self.apk),
            apt = quote_pkgs(&self.apt),
            apt_get = quote_pkgs(&self.apt_get),
        );

        // TODO: lock package resolving indexes to snaphots (date = base image's?)
        // https://github.com/reproducible-containers/repro-sources-list.sh/blob/39fbf150e3a5062d4c6b9a241f25af133e7cb6f0/repro-sources-list.sh
        // https://github.com/reproducible-containers/repro-get/blob/fcc0f1b7907fc0543d10b6934f1ef3a963bcd9c7/examples/gcc/Dockerfile
        (Network::Default, block) // TODO: pull Network::None using ADDs
    }
}

fn quote_pkgs(pkgs: &[String]) -> String {
    if pkgs.is_empty() {
        "<none>".to_owned()
    } else {
        pkgs.join("' '")
    }
}
