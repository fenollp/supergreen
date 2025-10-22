use serde::{Deserialize, Serialize};

use crate::network::Network;

// Envs that override Cargo.toml settings
pub(crate) const ENV_ADD_APK: &str = "CARGOGREEN_ADD_APK";
pub(crate) const ENV_ADD_APT: &str = "CARGOGREEN_ADD_APT";
pub(crate) const ENV_ADD_APT_GET: &str = "CARGOGREEN_ADD_APT_GET";

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Add {
    /// Adds OS packages to the base image with `apk add`, serialized as CSV.
    ///
    /// ```toml
    /// add.apk = [ "libpq-dev", "pkgconf" ]
    /// ```
    ///
    /// *This environment variable takes precedence over any `Cargo.toml` settings:*
    /// ```shell
    /// # Note: values here are comma-separated.
    /// CARGOGREEN_ADD_APK="libpq-dev,pkg-conf"
    /// ```
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apk: Vec<String>,

    /// Adds OS packages to the base image with `apt install`, serialized as CSV.
    ///
    /// ```toml
    /// add.apt = [ "libpq-dev", "pkg-config" ]
    /// ```
    ///
    /// *This environment variable takes precedence over any `Cargo.toml` settings:*
    /// ```shell
    /// # Note: values here are comma-separated.
    /// CARGOGREEN_ADD_APT="libpq-dev,pkg-config"
    /// ```
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apt: Vec<String>,

    /// Adds OS packages to the base image with `apt-get install`, serialized as CSV.
    ///
    /// ```toml
    /// add.apt-get = [ "libpq-dev", "pkg-config" ]
    /// ```
    ///
    /// *This environment variable takes precedence over any `Cargo.toml` settings:*
    /// ```shell
    /// # Note: values here are comma-separated.
    /// CARGOGREEN_ADD_APT_GET="libpq-dev,pkg-config"
    /// ```
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) apt_get: Vec<String>,
}

impl Add {
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        let Self { apk, apt, apt_get } = self;
        [apk, apt, apt_get].iter().all(|x| x.is_empty())
    }

    // TODO: pin major + lock by pulling
    // TODO: more architectures
    pub(crate) fn as_block(&self, last: &str) -> (Network, String) {
        const XX: &str = "docker.io/tonistiigi/xx:1.6.1@sha256:923441d7c25f1e2eb5789f82d987693c47b8ed987c4ab3b075d6ed2b5d6779a3";

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
    elif command -v apt >/dev/null 2&>1; then \
      DEBIAN_FRONTEND=noninteractive xx-apt     install --no-install-recommends -y '{apt}'; \
    else \
      DEBIAN_FRONTEND=noninteractive xx-apt-get install --no-install-recommends -y '{apt_get}'; \
    fi
"#,
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
