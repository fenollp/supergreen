use anyhow::{bail, Result};
use chrono::{DateTime, FixedOffset};
use log::warn;

use crate::{ext::CommandExt, green::Green};

#[derive(Debug, Default)]
pub(crate) struct Du {
    id: String,
    parent: Option<String>,
    created_at: DateTime<FixedOffset>,
    mutable: bool,
    reclaimable: bool,
    shared: bool,
    size: String,
    description: String,
    usage_count: String,
    last_used: String,
    r#type: String,
}

// ID:     zh2p7ef3asdf4tcv3ut0nvdle
// Parent:     ywvobht0suz3p3axzia2l1llq
// Created at: 2025-08-23 12:01:27.811028341 +0000 UTC
// Mutable:    false
// Reclaimable:    true
// Shared:     false
// Size:       1.103GB
// Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:6c828d9865870a3bc8c02919d73803df22cac59b583d8f2cb30a296abe64748f
// Usage count:    6
// Last used:  46 minutes ago
// Type:       regular

fn parse_buildx_du_kvs(stdout: &[u8]) -> Vec<Du> {
    let mut dus = vec![];
    for block in String::from_utf8_lossy(stdout).split_inclusive("\n\n") {
        let mut du = Du::default();
        for line in block.lines() {
            let Some((lhs, rhs)) = line.split_once(":") else { continue };
            let rhs = rhs.trim();
            match lhs {
                "ID" => du.id = rhs.to_owned(),
                "Parent" => du.parent = Some(rhs.to_owned()),
                "Created at" => {
                    let v = DateTime::parse_and_remainder(rhs, "%Y-%m-%d %H:%M:%S%.9f %z").ok();
                    let Some((v, _)) = v else { continue };
                    du.created_at = v;
                }
                "Mutable" => du.mutable = rhs == "true",
                "Reclaimable" => du.reclaimable = rhs == "true",
                "Shared" => du.shared = rhs == "true",
                "Size" => du.size = rhs.to_owned(),
                "Description" => du.description = rhs.to_owned(),
                "Usage count" => du.usage_count = rhs.to_owned(),
                "Last used" => du.last_used = rhs.to_owned(),
                "Type" => du.r#type = rhs.to_owned(),
                wut => warn!("unexpected builder cache entry field: {wut}"),
            }
        }
        dus.push(du);
    }
    dus
}

impl Green {
    pub(crate) async fn images_in_builder_cache(&self) -> Result<Vec<Du>> {
        let mut cmd = self.cmd();
        cmd.args(["buildx", "du", "--verbose"]);
        cmd.arg("--filter=type=regular");
        cmd.arg("--filter=description~=pulled.from");
        let (succeeded, stdout, stderr) = cmd.exec().await?;
        if !succeeded {
            let stderr = String::from_utf8_lossy(&stderr);
            bail!("Failed to query builder cache: {stderr}")
        }
        Ok(parse_images(&stdout))
    }
}

fn parse_images(stdout: &[u8]) -> Vec<Du> {
    let mut dus = parse_buildx_du_kvs(stdout);
    dus.sort_by(|Du { created_at: a, .. }, Du { created_at: b, .. }| b.cmp(a));
    dus.dedup_by_key(|Du { description, .. }| description.to_owned());
    dus
}

#[inline]
#[must_use]
pub(crate) fn lock_from_builder_cache<'a>(img: &str, cached: &'a [Du]) -> Option<&'a str> {
    cached
        .iter()
        .filter(|&Du { description, .. }| description.contains(img))
        .map(|Du { description, .. }| {
            description.trim_start_matches(|c| c != '@').trim_start_matches('@')
        })
        .next()
}

#[test]
fn lock_from_builder_cache_multiple_identical() {
    let stdout = r#"
Usage count:    1
Last used:  6 days ago
Type:       regular

ID:     dyoo0ez6aq47esc1lu7gij20a
Created at: 2025-08-12 13:04:40.696682772 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       113.5MB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa
Usage count:    1
Last used:  23 hours ago
Type:       regular

ID:     u5k6dutexg57ajnuatyj805re
Created at: 2025-08-23 12:05:44.238653655 +0000 UTC
Mutable:    true
Reclaimable:    true
Shared:     false
Size:       11.51MB
Description:    [out-19ffbea695cb4980 1/1] COPY --from=dep-l-syn-2.0.104-19ffbea695cb4980 /tmp/clis-cargo-config2_0-1-34_/release/deps/*-19ffbea695cb4980* /
Usage count:    3
Last used:  About an hour ago
Type:       regular

ID:     ohxhekyoshxip5l5hnd3th9jb
Parent:     dyoo0ez6aq47esc1lu7gij20a
Created at: 2025-08-12 13:04:40.701102099 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       1.09GB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa
Usage count:    11
Last used:  23 hours ago
Type:       regular

Reclaimable:    3.69GB
Total:      3.69GB
"#;
    let cached = parse_images(stdout.as_bytes());
    assert_eq!(
        lock_from_builder_cache("rust:1.89.0-slim", &cached),
        Some("sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa")
    );
    assert_eq!(
        lock_from_builder_cache("docker.io/library/rust:1.89.0-slim", &cached),
        Some("sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa")
    );
    assert_eq!(lock_from_builder_cache("blaaaa", &cached), None);
}

#[test]
fn lock_from_builder_cache_multiple_sortme() {
    let multiple = r#"ID:     dyoo0ez6aq47esc1lu7gij20a
Created at: 2025-08-12 13:04:40.696682772 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       113.5MB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa
Usage count:    1
Last used:  42 hours ago
Type:       regular

ID:     re241lo0ymzrzzhdpam8nlrlh
Created at: 2025-08-13 12:56:45.856142994 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       113.5MB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:2ff54dd21007d5ee97026fadad80598e66136a43adc5687078d796d958bd58fb
Usage count:    1
Last used:  18 hours ago
Type:       regular

ID:     u5k6dutexg57ajnuatyj805re
Created at: 2025-08-23 12:05:44.238653655 +0000 UTC
Mutable:    true
Reclaimable:    true
Shared:     false
Size:       11.51MB
Description:    [out-19ffbea695cb4980 1/1] COPY --from=dep-l-syn-2.0.104-19ffbea695cb4980 /tmp/clis-cargo-config2_0-1-34_/release/deps/*-19ffbea695cb4980* /
Usage count:    3
Last used:  About an hour ago
Type:       regular

ID:     zx37wzeg6qh755h9vitile8b2
Parent:     re241lo0ymzrzzhdpam8nlrlh
Created at: 2025-08-13 12:56:45.859601066 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       1.09GB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:2ff54dd21007d5ee97026fadad80598e66136a43adc5687078d796d958bd58fb
Usage count:    5
Last used:  17 hours ago
Type:       regular

ID:     ohxhekyoshxip5l5hnd3th9jb
Parent:     dyoo0ez6aq47esc1lu7gij20a
Created at: 2025-08-12 13:04:40.701102099 +0000 UTC
Mutable:    false
Reclaimable:    true
Shared:     false
Size:       1.09GB
Description:    pulled from docker.io/library/rust:1.89.0-slim@sha256:33219ca58c0dd38571fd3f87172b5bce2d9f3eb6f27e6e75efe12381836f71fa
Usage count:    11
Last used:  41 hours ago
Type:       regular

"#;
    let cached = parse_images(multiple.as_bytes());
    assert_eq!(
        lock_from_builder_cache("rust:1.89.0-slim", &cached),
        Some("sha256:2ff54dd21007d5ee97026fadad80598e66136a43adc5687078d796d958bd58fb")
    );
}
