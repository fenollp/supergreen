//! Pooled connections for builders `cargo green` does not manage.
//!
//! buildx records a builder's node endpoints verbatim (a docker host address, or a docker context
//! name) and dials these directly: `$DOCKER_HOST` does not apply. A builder created against the
//! pooled daemon (e.g. `docker buildx create --name mine ssh://host`) would bypass the pool.
//!
//! Instead, docker calls get `$BUILDX_CONFIG` pointing at a shadow of buildx's state directory:
//! every entry links back to the real one (so activity, refs, .. are shared), except
//! builder instances having nodes on the pooled daemon. Their copies point at the pool's socket.
//!
//! The user's builders stay untouched, as do their containers: the pool reaches the same daemon.

use std::{io, process};

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use serde_json::Value;
use url::Url;

use crate::{
    redact::Vars,
    remote::{RemoteDaemon, docker_config_dir, docker_context_named},
    sys::fs,
};

/// Drivers whose endpoint is a docker daemon
const DOCKER_DRIVERS: [&str; 2] = ["docker", "docker-container"];

/// Where this process shadows buildx's state directory: a private place, removed when done.
#[must_use]
pub(crate) fn shadow_dir(var_run: &Utf8Path) -> Utf8PathBuf {
    var_run.join(format!("buildx-{}", process::id()))
}

/// buildx's state directory: `$BUILDX_CONFIG`, defaulting to `$DOCKER_CONFIG/buildx`.
fn buildx_config_dir(env: &Vars) -> Option<Utf8PathBuf> {
    if let Some(dir) = env.get_nonempty(BUILDX_CONFIG!()) {
        return Some(dir.into());
    }
    Some(docker_config_dir(env)?.join("buildx"))
}

/// Shadows buildx's state directory at `shadow`, so builders on the `remote` docker daemon
/// go through the pool listening at `pooled`.
///
/// Gives whether `shadow` is to be used as `$BUILDX_CONFIG`: not when there is nothing to shadow.
pub(crate) fn shadow_buildx_config(
    env: &Vars,
    remote: &RemoteDaemon,
    pooled: &str,
    shadow: &Utf8Path,
) -> Result<bool> {
    // Builders on a BuildKit daemon use the `remote` driver: only the managed one is supported
    let RemoteDaemon::Docker { .. } = remote else { return Ok(false) };
    let Some(real) = buildx_config_dir(env) else { return Ok(false) };
    let instances = real.join("instances");
    if !fs().is_dir(&instances) {
        return Ok(false); // No builders yet
    }

    let shadow_instances = shadow.join("instances");
    fs().create_dir_all_mode(&shadow_instances, 0o700)
        .map_err(failed("mkdir -p", &shadow_instances))?;

    for entry in fs().read_dir(&real).map_err(failed("list", &real))? {
        let Some(name) = entry.file_name() else { continue };
        let link = shadow.join(name);
        match name {
            "instances" => {}
            _ => fs().symlink_file(&entry, &link).map_err(failed("link", &link))?,
        }
    }

    for entry in fs().read_dir(&instances).map_err(failed("list", &instances))? {
        let Some(name) = entry.file_name() else { continue };
        let link = shadow_instances.join(name);
        let json = fs().read_to_string(&entry).map_err(failed("read", &entry))?;
        match repoint(env, remote, pooled, &json) {
            Some(json) => {
                info!("buildx: builder {name:?} goes through the pool");
                // Through a rename: never writes through a link to the real file
                fs().write_atomic_bytes(&link, json.as_bytes(), 0o600)
                    .map_err(failed("write", &link))?;
            }
            None => fs().symlink_file(&entry, &link).map_err(failed("link", &link))?,
        }
    }
    Ok(true)
}

fn failed(op: &str, path: &Utf8Path) -> impl FnOnce(io::Error) -> anyhow::Error {
    move |e| anyhow!("Failed to {op} {path}: {e}")
}

/// A builder instance with its nodes on `remote` pointed at `pooled`, if it has any.
/// Instances buildx would fail to read are left for buildx to complain about.
fn repoint(env: &Vars, remote: &RemoteDaemon, pooled: &str, json: &str) -> Option<String> {
    let mut group: Value = serde_json::from_str(json).ok()?;
    if !DOCKER_DRIVERS.contains(&group.get("Driver")?.as_str()?) {
        return None;
    }
    let mut repointed = false;
    for node in group.get_mut("Nodes")?.as_array_mut()? {
        let Some(endpoint) = node.get_mut("Endpoint") else { continue };
        if endpoint.as_str().is_some_and(|ep| is_on(env, remote, ep)) {
            *endpoint = pooled.into();
            repointed = true;
        }
    }
    repointed.then(|| group.to_string())
}

/// Whether a node's endpoint (an address or a docker context name) reaches the `remote` daemon.
fn is_on(env: &Vars, remote: &RemoteDaemon, endpoint: &str) -> bool {
    let host = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        // Contexts that fail to load are not on our daemon
        let Some(config) = docker_config_dir(env) else { return false };
        let Ok(Some(context)) = docker_context_named(&config, endpoint.to_owned()) else {
            return false;
        };
        context.host
    };
    Url::parse(&host).is_ok_and(|url| RemoteDaemon::Docker { url, tls: None }.key() == remote.key())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::sys::{Fs, Sys, fake::FakeFs};

    const CONFIG: &str = "/home/me/.docker";
    const BUILDX: &str = "/home/me/.docker/buildx";
    const SHADOW: &str = "/run/cg/buildx-42";
    const POOLED: &str = "unix:///run/cg/pool-1234.sock";

    fn with_fs<T>(files: impl FnOnce(&FakeFs), f: impl FnOnce(&FakeFs) -> T) -> T {
        let fs = FakeFs::new();
        files(&fs);
        let _guard = Sys::install(Sys { fs: fs.clone() as Arc<_>, ..Sys::fake() });
        f(&fs)
    }

    fn instance(driver: &str, endpoints: &[&str]) -> String {
        let nodes: Vec<_> = endpoints
            .iter()
            .enumerate()
            .map(|(i, ep)| format!(r#"{{"Name":"n{i}","Endpoint":"{ep}","Platforms":null}}"#))
            .collect();
        let nodes = nodes.join(",");
        format!(r#"{{"Name":"mine","Driver":"{driver}","Nodes":[{nodes}],"Dynamic":false}}"#)
    }

    fn endpoints(json: &str) -> Vec<String> {
        let group: Value = serde_json::from_str(json).unwrap();
        let nodes = group["Nodes"].as_array().unwrap();
        nodes.iter().map(|n| n["Endpoint"].as_str().unwrap().to_owned()).collect()
    }

    fn env() -> Vars {
        Vars::from([(DOCKER_CONFIG!(), CONFIG)])
    }

    fn remote(addr: &str) -> RemoteDaemon {
        RemoteDaemon::Docker { url: Url::parse(addr).unwrap(), tls: None }
    }

    fn repointed(driver: &str, eps: &[&str]) -> Option<Vec<String>> {
        let json = instance(driver, eps);
        with_fs(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json))
            .map(|json| endpoints(&json))
    }

    #[test]
    fn repoints_the_remote_address() {
        let got = repointed("docker-container", &["ssh://me@far"]);
        assert_eq!(got, Some(vec![POOLED.to_owned()]));
    }

    #[test]
    fn repoints_the_remote_address_with_trailing_slash() {
        let got = repointed("docker-container", &["ssh://me@far/"]);
        assert_eq!(got, Some(vec![POOLED.to_owned()]));
    }

    #[test]
    fn repoints_only_nodes_on_the_remote() {
        let got = repointed("docker-container", &["ssh://me@far", "ssh://me@near"]);
        assert_eq!(got, Some(vec![POOLED.to_owned(), "ssh://me@near".to_owned()]));
    }

    #[test]
    fn leaves_other_daemons() {
        assert_eq!(repointed("docker-container", &["ssh://me@near"]), None);
        assert_eq!(repointed("docker-container", &["ssh://far"]), None);
        assert_eq!(repointed("docker", &["unix:///var/run/docker.sock"]), None);
    }

    #[test]
    fn leaves_remote_drivers() {
        assert_eq!(repointed("remote", &["ssh://me@far"]), None);
        assert_eq!(repointed("kubernetes", &["ssh://me@far"]), None);
    }

    #[test]
    fn leaves_unreadable_instances() {
        let got = with_fs(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, "{"));
        assert_eq!(got, None);
    }

    #[test]
    fn keeps_other_fields() {
        let json = instance("docker-container", &["ssh://me@far"]);
        let json = json.replace(r#""Dynamic":false"#, r#""Dynamic":false,"Unknown":[1]"#);
        let got = with_fs(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
        let got: Value = serde_json::from_str(&got.unwrap()).unwrap();
        assert_eq!(got["Unknown"], serde_json::json!([1]));
        assert_eq!(got["Name"], "mine");
    }

    fn far_context(fs: &FakeFs) {
        let id = sha256::digest("far");
        let json = r#"{"Name":"far","Metadata":{},"Endpoints":{"docker":{"Host":"ssh://me@far"}}}"#;
        fs.file(format!("{CONFIG}/contexts/meta/{id}/meta.json"), json);
    }

    #[test]
    fn repoints_a_context_on_the_remote() {
        let json = instance("docker-container", &["far"]);
        let got = with_fs(far_context, |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
        assert_eq!(endpoints(&got.unwrap()), [POOLED]);
    }

    #[test]
    fn leaves_unknown_and_default_contexts() {
        for ep in ["nope", "default"] {
            let json = instance("docker", &[ep]);
            let got =
                with_fs(far_context, |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
            assert_eq!(got, None, "{ep}");
        }
    }

    fn store(fs: &FakeFs) {
        fs.file(format!("{BUILDX}/current"), r#"{"Key":"","Name":"","Global":false}"#);
        fs.file(format!("{BUILDX}/activity/mine"), "");
        fs.mkdir(format!("{BUILDX}/refs"));
        fs.file(
            format!("{BUILDX}/instances/mine"),
            instance("docker-container", &["ssh://me@far"]),
        );
        fs.file(
            format!("{BUILDX}/instances/other"),
            instance("docker-container", &["tcp://h:2375"]),
        );
    }

    fn shadow(env: &Vars, remote: &RemoteDaemon) -> Result<bool> {
        shadow_buildx_config(env, remote, POOLED, SHADOW.into())
    }

    #[test]
    fn shadows_the_store() {
        with_fs(store, |fs| {
            assert!(shadow(&env(), &remote("ssh://me@far")).unwrap());
            for entry in ["current", "activity", "refs", "instances/other"] {
                let link = Utf8PathBuf::from(format!("{SHADOW}/{entry}"));
                assert!(fs.is_symlink(&link), "{link}");
            }
            let mine = fs.read(format!("{SHADOW}/instances/mine")).unwrap();
            assert_eq!(endpoints(&mine), [POOLED]);
            assert_eq!(fs.mode(format!("{SHADOW}/instances/mine").as_str().into()).unwrap(), 0o600);
        });
    }

    #[test]
    fn leaves_the_real_store_alone() {
        with_fs(store, |fs| {
            let before = fs.read(format!("{BUILDX}/instances/mine"));
            shadow(&env(), &remote("ssh://me@far")).unwrap();
            assert_eq!(fs.read(format!("{BUILDX}/instances/mine")), before);
        });
    }

    #[test]
    fn shadows_buildx_config() {
        let files =
            |fs: &FakeFs| fs.file("/bx/instances/mine", instance("docker", &["ssh://me@far"]));
        with_fs(files, |fs| {
            let env = Vars::from([(BUILDX_CONFIG!(), "/bx")]);
            assert!(shadow(&env, &remote("ssh://me@far")).unwrap());
            assert_eq!(endpoints(&fs.read(format!("{SHADOW}/instances/mine")).unwrap()), [POOLED]);
        });
    }

    #[test]
    fn nothing_to_shadow_without_builders() {
        assert!(!with_fs(|_| (), |_| shadow(&env(), &remote("ssh://me@far")).unwrap()));
    }

    #[test]
    fn nothing_to_shadow_for_buildkit() {
        let buildkit =
            RemoteDaemon::BuildKit { url: Url::parse("ssh://me@far").unwrap(), tls: None };
        assert!(!with_fs(store, |_| shadow(&env(), &buildkit).unwrap()));
    }

    #[test]
    fn shadow_dir_is_per_process() {
        let dir = shadow_dir("/run/cg".into());
        assert_eq!(dir, format!("/run/cg/buildx-{}", process::id()));
    }
}
