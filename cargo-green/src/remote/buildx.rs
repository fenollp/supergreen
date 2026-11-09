//! Sends users' buildx builders through the pool.
//!
//! buildx ignores `$DOCKER_HOST` for existing builders, so docker calls get a private
//! `$BUILDX_CONFIG`: a copy of the user's, repointed at the pool.

use std::process;

use anyhow::{Result, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use log::info;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::{
    builder::Driver,
    dirs::{ONLY_OURS_MODE, VAR_RUN_MODE},
    redact::Vars,
    remote::{RemoteDaemon, docker_config_dir, docker_context_named},
    sys::fs,
};

/// Where this process shadows buildx's state directory, next to the pool's `socket`:
/// a private place, removed when done.
#[must_use]
pub(crate) fn shadow_dir(socket: &Utf8Path) -> Utf8PathBuf {
    socket.with_file_name(format!("buildx-{}", process::id()))
}

/// buildx's state directory: `$BUILDX_CONFIG`, defaulting to `$DOCKER_CONFIG/buildx`.
fn buildx_config_dir(env: &Vars) -> Option<Utf8PathBuf> {
    if let Some(dir) = env.get_nonempty(BUILDX_CONFIG!()) {
        return Some(dir.into());
    }
    let dir = docker_config_dir(env)?;
    Some(dir.join("buildx"))
}

/// Shadows buildx's state directory, so builders on the `remote` docker daemon
/// go through the pool listening at `pooled`.
///
/// Gives what to use as `$BUILDX_CONFIG`. Nothing gets created when no builder needs the pool.
pub(crate) fn shadow_buildx_config(
    env: &Vars,
    remote: &RemoteDaemon,
    pooled: &Url,
) -> Result<Option<Utf8PathBuf>> {
    // Builders on a BuildKit daemon use the `remote` driver: only the managed one is supported
    let RemoteDaemon::Docker { .. } = remote else { return Ok(None) };
    let Some(real) = buildx_config_dir(env) else { return Ok(None) };
    let instances = real.join("instances");
    if !fs().is_dir(&instances) {
        return Ok(None); // No builders yet
    }

    // Decodes what a pool's address percent-encodes
    let Ok(socket) = pooled.to_file_path() else { return Ok(None) };
    let Some(socket) = Utf8Path::from_path(&socket) else { return Ok(None) };
    let shadow = shadow_dir(socket);
    let shadow_instances = shadow.join("instances");
    let addr = pooled.as_str();

    let top = fs()
        .read_dir(&real)
        .map_err(|e| anyhow!("Failed listing {real}: {e}"))?
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name().filter(|&name| name != "instances")?;
            let link = shadow.join(name);
            if name != "current" {
                return Some(Ok((entry, link, None)));
            }
            fs().read_to_string(&entry)
                .map_err(|e| anyhow!("Failed opening (RO) {entry}: {e}"))
                .map(|json| {
                    let json = rekey(env, remote, addr, &json)
                        .inspect(|_| info!("buildx: current builder goes through the pool"));
                    Some((entry, link, json))
                })
                .transpose()
        });

    let builders = fs()
        .read_dir(&instances)
        .map_err(|e| anyhow!("Failed listing {instances}: {e}"))?
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name()?.to_owned();
            fs().read_to_string(&entry)
                .map_err(|e| anyhow!("Failed opening (RO) {entry}: {e}"))
                .map(|json| {
                    let link = shadow_instances.join(&name);
                    let json = repoint(env, remote, addr, &json)
                        .inspect(|_| info!("buildx: builder {name:?} goes through the pool"));
                    Some((entry, link, json))
                })
                .transpose()
        });

    let planned = top.chain(builders).collect::<Result<Vec<_>>>()?;
    if planned.iter().all(|(_, _, json)| json.is_none()) {
        return Ok(None);
    }

    fs().create_dir_all_mode(&shadow_instances, VAR_RUN_MODE)
        .map_err(|e| anyhow!("Failed `mkdir -p {shadow_instances}`: {e}"))?;
    for (entry, link, json) in planned {
        match json {
            // Through a rename: never writes through a link to the real file
            Some(json) => fs()
                .write_atomic_bytes(&link, json.as_bytes(), ONLY_OURS_MODE)
                .map_err(|e| anyhow!("Failed atomic writing {link}: {e}"))?,
            None => fs()
                .symlink_file(&entry, &link)
                .map_err(|e| anyhow!("Failed `ln -s {entry} {link}`: {e}"))?,
        }
    }
    Ok(Some(shadow))
}

/// Fields we do not read: kept, for buildx to find them in our copies
type Others = Map<String, Value>;

/// A builder instance with its nodes on `remote` pointed at `pooled`, if it has any.
/// Instances buildx would fail to read are left for buildx to complain about.
fn repoint(env: &Vars, remote: &RemoteDaemon, pooled: &str, json: &str) -> Option<String> {
    /// A file of buildx's `instances` directory
    #[derive(Deserialize, Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Instance {
        driver: Driver,
        nodes: Vec<Node>,
        #[serde(flatten)]
        others: Others,
    }

    #[derive(Deserialize, Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Node {
        endpoint: String,
        #[serde(flatten)]
        others: Others,
    }

    let mut instance: Instance = serde_json::from_str(json).ok()?;
    // Other drivers' endpoints are not docker daemons
    if !matches!(instance.driver, Driver::Docker | Driver::DockerContainer) {
        return None;
    }
    let mut repointed = false;
    for node in &mut instance.nodes {
        if is_on(env, remote, &node.endpoint) {
            pooled.clone_into(&mut node.endpoint);
            repointed = true;
        }
    }
    if !repointed {
        return None;
    }
    serde_json::to_string(&instance).ok()
}

/// buildx's `current` (builder picked by `docker buildx use`), re-keyed to `pooled`
/// when it was picked for the `remote` daemon.
fn rekey(env: &Vars, remote: &RemoteDaemon, pooled: &str, json: &str) -> Option<String> {
    #[derive(Deserialize, Serialize)]
    #[serde(rename_all = "PascalCase")]
    struct Current {
        key: String,
        #[serde(flatten)]
        others: Others,
    }

    let mut current: Current = serde_json::from_str(json).ok()?;
    if !is_on(env, remote, &current.key) {
        return None;
    }
    pooled.clone_into(&mut current.key);
    serde_json::to_string(&current).ok()
}

/// Whether a node's endpoint (an address or a docker context name) reaches the `remote` daemon.
fn is_on(env: &Vars, remote: &RemoteDaemon, endpoint: &str) -> bool {
    let host = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        // Contexts that fail to load are not on our daemon
        let Some(config) = docker_config_dir(env) else { return false };
        let Ok(Some(context)) = docker_context_named(&config, endpoint) else {
            return false;
        };
        context.host
    };
    let Ok(url) = Url::parse(&host) else { return false };
    remote.key() == RemoteDaemon::Docker { url, tls: None }.key()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dirs::ONLY_OURS_MODE,
        sys::{Fs, Sys, fake::FakeFs},
    };

    const CONFIG: &str = "/home/me/.docker";
    const BUILDX: &str = "/home/me/.docker/buildx";
    const POOLED: &str = "unix:///run/cg/pool-1234.sock";

    fn at(entry: &str) -> String {
        format!("/run/cg/buildx-{}/{entry}", process::id())
    }

    fn with<T>(files: impl FnOnce(&FakeFs), f: impl FnOnce(&FakeFs) -> T) -> T {
        let fs = FakeFs::new();
        files(&fs);
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        f(&fs)
    }

    fn instance(driver: Driver, endpoints: &[&str]) -> String {
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

    fn repointed(driver: Driver, eps: &[&str]) -> Option<Vec<String>> {
        let json = instance(driver, eps);
        with(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json))
            .map(|json| endpoints(&json))
    }

    #[test]
    fn repoints_the_remote_address() {
        let got = repointed(Driver::DockerContainer, &["ssh://me@far"]);
        assert_eq!(got, Some(vec![POOLED.to_owned()]));
    }

    #[test]
    fn repoints_the_remote_address_with_trailing_slash() {
        let got = repointed(Driver::DockerContainer, &["ssh://me@far/"]);
        assert_eq!(got, Some(vec![POOLED.to_owned()]));
    }

    #[test]
    fn repoints_only_nodes_on_the_remote() {
        let got = repointed(Driver::DockerContainer, &["ssh://me@far", "ssh://me@near"]);
        assert_eq!(got, Some(vec![POOLED.to_owned(), "ssh://me@near".to_owned()]));
    }

    #[test]
    fn leaves_other_daemons() {
        assert_eq!(repointed(Driver::DockerContainer, &["ssh://me@near"]), None);
        assert_eq!(repointed(Driver::DockerContainer, &["ssh://far"]), None);
        assert_eq!(repointed(Driver::Docker, &["unix:///var/run/docker.sock"]), None);
    }

    #[test]
    fn leaves_remote_drivers() {
        assert_eq!(repointed(Driver::Remote, &["ssh://me@far"]), None);
        assert_eq!(repointed(Driver::Other("kubernetes".to_owned()), &["ssh://me@far"]), None);
    }

    #[test]
    fn leaves_unreadable_instances() {
        let got = with(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, "{"));
        assert_eq!(got, None);
    }

    #[test]
    fn keeps_other_fields() {
        let json = instance(Driver::DockerContainer, &["ssh://me@far"]);
        let json = json.replace(r#""Dynamic":false"#, r#""Dynamic":false,"Unknown":[1]"#);
        let got = with(|_| (), |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
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
        let json = instance(Driver::DockerContainer, &["far"]);
        let got = with(far_context, |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
        assert_eq!(endpoints(&got.unwrap()), [POOLED]);
    }

    #[test]
    fn leaves_unknown_and_default_contexts() {
        for ep in ["nope", "default"] {
            let json = instance(Driver::Docker, &[ep]);
            let got =
                with(far_context, |_| repoint(&env(), &remote("ssh://me@far"), POOLED, &json));
            assert_eq!(got, None, "{ep}");
        }
    }

    fn store(fs: &FakeFs) {
        fs.file(format!("{BUILDX}/current"), r#"{"Key":"","Name":"","Global":false}"#);
        fs.file(format!("{BUILDX}/activity/mine"), "");
        fs.mkdir(format!("{BUILDX}/refs"));
        fs.file(
            format!("{BUILDX}/instances/mine"),
            instance(Driver::DockerContainer, &["ssh://me@far"]),
        );
        fs.file(
            format!("{BUILDX}/instances/other"),
            instance(Driver::DockerContainer, &["tcp://h:2375"]),
        );
    }

    fn shadow(env: &Vars, remote: &RemoteDaemon) -> Result<bool> {
        shadow_buildx_config(env, remote, &Url::parse(POOLED).unwrap()).map(|dir| dir.is_some())
    }

    #[test]
    fn rekeys_current_on_the_remote() {
        let rekeyed = |json: &str| {
            with(far_context, |_| rekey(&env(), &remote("ssh://me@far"), POOLED, json))
        };
        let got = rekeyed(r#"{"Key":"ssh://me@far","Name":"mine","Global":false}"#).unwrap();
        let got: Value = serde_json::from_str(&got).unwrap();
        assert_eq!((got["Key"].as_str(), got["Name"].as_str()), (Some(POOLED), Some("mine")));
        assert!(rekeyed(r#"{"Key":"far","Name":"mine","Global":false}"#).is_some());

        assert_eq!(rekeyed(r#"{"Key":"ssh://me@near","Name":"mine","Global":false}"#), None);
        assert_eq!(rekeyed(r#"{"Key":"unix:///var/run/docker.sock","Name":"mine"}"#), None);
        assert_eq!(rekeyed("{"), None);
    }

    #[test]
    fn shadows_current_on_the_remote() {
        let files = |fs: &FakeFs| {
            store(fs);
            fs.file(format!("{BUILDX}/current"), r#"{"Key":"ssh://me@far","Name":"mine"}"#);
        };
        with(files, |fs| {
            assert!(shadow(&env(), &remote("ssh://me@far")).unwrap());
            let link = Utf8PathBuf::from(at("current"));
            assert!(!fs.is_symlink(&link));
            let got: Value = serde_json::from_str(&fs.read(link.as_str()).unwrap()).unwrap();
            assert_eq!(got["Key"], POOLED);
            let real = fs.read(format!("{BUILDX}/current")).unwrap();
            assert!(real.contains("ssh://me@far"));
        });
    }

    #[test]
    fn shadows_the_store() {
        with(store, |fs| {
            assert!(shadow(&env(), &remote("ssh://me@far")).unwrap());
            for entry in ["current", "activity", "refs", "instances/other"] {
                let link = Utf8PathBuf::from(at(entry));
                assert!(fs.is_symlink(&link), "{link}");
            }
            let mine = fs.read(at("instances/mine")).unwrap();
            assert_eq!(endpoints(&mine), [POOLED]);
            assert_eq!(fs.mode(at("instances/mine").as_str().into()).unwrap(), ONLY_OURS_MODE);
        });
    }

    #[test]
    fn leaves_the_real_store_alone() {
        with(store, |fs| {
            let before = fs.read(format!("{BUILDX}/instances/mine"));
            shadow(&env(), &remote("ssh://me@far")).unwrap();
            assert_eq!(fs.read(format!("{BUILDX}/instances/mine")), before);
        });
    }

    #[test]
    fn shadows_buildx_config() {
        let files = |fs: &FakeFs| {
            fs.file("/bx/instances/mine", instance(Driver::Docker, &["ssh://me@far"]))
        };
        with(files, |fs| {
            let env = Vars::from([(BUILDX_CONFIG!(), "/bx")]);
            assert!(shadow(&env, &remote("ssh://me@far")).unwrap());
            assert_eq!(endpoints(&fs.read(at("instances/mine")).unwrap()), [POOLED]);
        });
    }

    #[test]
    fn nothing_to_shadow_without_builders() {
        assert!(!with(|_| (), |_| shadow(&env(), &remote("ssh://me@far")).unwrap()));
    }

    #[test]
    fn nothing_to_shadow_for_buildkit() {
        let buildkit =
            RemoteDaemon::BuildKit { url: Url::parse("ssh://me@far").unwrap(), tls: None };
        assert!(!with(store, |_| shadow(&env(), &buildkit).unwrap()));
    }

    #[test]
    fn nothing_created_when_no_builder_needs_the_pool() {
        with(store, |fs| {
            assert!(!shadow(&env(), &remote("ssh://me@elsewhere")).unwrap());
            assert!(!fs.is_dir(Utf8Path::new(&at(""))));
        });
    }

    #[test]
    fn shadows_for_the_current_builder_alone() {
        let files = |fs: &FakeFs| {
            store(fs);
            fs.file(
                format!("{BUILDX}/instances/mine"),
                instance(Driver::DockerContainer, &["ssh://me@near"]),
            );
            fs.file(format!("{BUILDX}/current"), r#"{"Key":"ssh://me@far","Name":"mine"}"#);
        };
        with(files, |fs| {
            assert!(shadow(&env(), &remote("ssh://me@far")).unwrap());
            assert!(!fs.is_symlink(Utf8Path::new(&at("current"))));
            assert!(fs.is_symlink(Utf8Path::new(&at("instances/mine"))));
        });
    }

    #[test]
    fn shadow_is_next_to_the_decoded_socket() {
        let pooled = Url::parse("unix:///run/c%20g/pool-1.sock").unwrap();
        with(store, |_| {
            let got = shadow_buildx_config(&env(), &remote("ssh://me@far"), &pooled).unwrap();
            assert_eq!(got.unwrap(), format!("/run/c g/buildx-{}", process::id()));
        });
    }

    #[test]
    fn shadow_dir_is_per_process() {
        let dir = shadow_dir("/run/cg/pool-1.sock".into());
        assert_eq!(dir, format!("/run/cg/buildx-{}", process::id()));
    }
}
