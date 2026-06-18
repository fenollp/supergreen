// Best-effort discovery of the *minimal* set of dependency artifacts `rustc` actually reads,
// so we can prune the over-approximated full-closure `--mount`s that otherwise blow up the LLB
// (see `md::walk_transitives`: it mounts a crate's entire transitive closure, but `rustc` only
// loads the subset reachable through the interfaces it uses — e.g. the `image` crate reads 97 of
// 403 provided rmetas).
//
// How: run `rustc -Z binary-dep-depinfo --emit metadata,dep-info` for the crate inside a one-off
// `docker run` (NOT buildx) with the host deps dir bind-mounted as a single volume. A plain
// container run has no BuildKit gRPC message cap, so this works even for crates whose normal
// (full-closure) build would exceed it. The emitted depfile lists exactly the binary deps rustc
// opened; their basenames are the read-set.
//
// Entirely fail-safe: ANY failure (image build, container run, parse, toolchain mismatch, missing
// dep, proc-macro panic, ...) returns `None` and the caller keeps the full closure — i.e. today's
// behaviour. Only a clean success prunes. Gated behind `CARGOGREEN_EXPERIMENT=binarydepinfo` and
// applied to rmeta-consuming (library) builds only — link builds (bins/tests) need the rlibs that
// a metadata-only discovery would never record.

use std::env;

use anyhow::{anyhow, bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexSet;
use log::{info, warn};

use crate::{green::Green, stage::RST};

/// Parse a `-Z binary-dep-depinfo` depfile, returning the basenames of the `.rmeta`/`.rlib`/`.so`
/// artifacts `rustc` opened. Robust to Make-style target/prereq layout: we just scan every token.
#[must_use]
pub(crate) fn parse_readset(dotd: &str) -> IndexSet<String> {
    dotd.split(|c: char| c.is_whitespace() || c == ':')
        .filter(|t| t.ends_with(".rmeta") || t.ends_with(".rlib") || t.ends_with(".so"))
        .filter_map(|t| Utf8Path::new(t).file_name())
        .map(ToOwned::to_owned)
        .collect()
}

/// Drop flags that conflict with our metadata-only discovery emit (we re-add our own).
#[must_use]
fn filter_args(flags: &[String]) -> Vec<String> {
    // Flags whose value is the *next* argument:
    const PAIRS: &[&str] = &["--out-dir", "-o", "--diagnostic-width"];
    // `-C key=val` values we must not carry into a metadata-only run:
    const C_DROP: &[&str] = &["incremental=", "linker=", "link-arg="];
    // Single-token (`--flag=val`) prefixes to drop:
    const PREFIXES: &[&str] =
        &["--emit", "--json", "--error-format", "--diagnostic-width=", "--out-dir=", "-o="];

    let mut out = Vec::with_capacity(flags.len());
    let mut it = flags.iter().peekable();
    while let Some(a) = it.next() {
        if PAIRS.contains(&a.as_str()) {
            it.next(); // drop the value too
            continue;
        }
        if a == "-C" {
            if let Some(v) = it.peek() {
                if C_DROP.iter().any(|p| v.starts_with(p)) {
                    it.next();
                    continue;
                }
            }
        }
        if PREFIXES.iter().any(|p| a.starts_with(p)) {
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn readset_cache(target_path: &Utf8Path, mdid: &str) -> Utf8PathBuf {
    target_path.join(format!(".{mdid}.readset"))
}

/// Returns the read-set (artifact basenames rustc opened) or `None` to keep the full closure.
/// `arguments` is the raw wrapper argv: `[this, real-rustc, ...flags, input]`.
pub(crate) async fn discover_readset(
    green: &Green,
    arguments: &[String],
    mdid: &str,
    pwd: &Utf8Path,
    target_path: &Utf8Path,
) -> Option<IndexSet<String>> {
    let cache = readset_cache(target_path, mdid);
    if let Ok(txt) = std::fs::read_to_string(&cache) {
        info!("binarydepinfo: read-set cache hit for {mdid}");
        return Some(txt.lines().filter(|l| !l.is_empty()).map(ToOwned::to_owned).collect());
    }

    match try_discover(green, arguments, mdid, pwd, target_path).await {
        Ok(readset) => {
            let _ = std::fs::write(&cache, readset.iter().cloned().collect::<Vec<_>>().join("\n"));
            Some(readset)
        }
        Err(e) => {
            warn!("binarydepinfo: discovery failed for {mdid}, keeping full closure: {e}");
            None
        }
    }
}

async fn try_discover(
    green: &Green,
    arguments: &[String],
    mdid: &str,
    pwd: &Utf8Path,
    target_path: &Utf8Path,
) -> Result<IndexSet<String>> {
    if arguments.len() < 3 {
        bail!("unexpected rustc argv (len {})", arguments.len());
    }
    let image = ensure_discovery_image(green).await?;

    let outdir = target_path.join(format!(".sg-disc-{mdid}"));
    let _ = std::fs::remove_dir_all(&outdir);
    std::fs::create_dir_all(&outdir).map_err(|e| anyhow!("mkdir {outdir}: {e}"))?;

    let cargo_home = green.cargo_home.as_str();
    // CARGO_TARGET_DIR root holds the deps dir the (host-path) `--extern`/`-L` flags point at.
    let target_root = env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| target_path.to_string());

    let mut cmd = green.cmd()?;
    cmd.arg("run").arg("--rm");
    // Identity bind-mounts: host paths resolve at the same paths inside the container, so the
    // crate's original (host-path) rustc flags need no rewriting.
    for src in [cargo_home, target_root.as_str()] {
        cmd.arg("-v").arg(format!("{src}:{src}:ro"));
    }
    if !pwd.as_str().starts_with(cargo_home) && !pwd.as_str().starts_with(target_root.as_str()) {
        cmd.arg("-v").arg(format!("{pwd}:{pwd}:ro"));
    }
    cmd.arg("-v").arg(format!("{outdir}:{outdir}"));
    cmd.arg("-w").arg(pwd.as_str());

    // Forward the build's CARGO_* env (incl. CARGO_MANIFEST_DIR) so compile-time proc-macros (e.g.
    // RustEmbed) resolve. Keep the image's own CARGO_HOME/RUSTUP_HOME (they locate the toolchain).
    for (k, v) in env::vars() {
        if k.starts_with("CARGO_") && k != "CARGO_HOME" && k != "CARGO_TARGET_DIR" {
            cmd.arg("-e").arg(format!("{k}={v}"));
        }
    }
    cmd.arg("-e").arg("RUSTC_BOOTSTRAP=1"); // -Z on a stable toolchain
    cmd.arg(&image);

    cmd.arg("rustc");
    cmd.args(filter_args(&arguments[2..])); // skip [this, real-rustc]
    cmd.arg("-Z").arg("binary-dep-depinfo").arg("--emit").arg("metadata,dep-info");
    cmd.arg("--out-dir").arg(outdir.as_str());

    let out = cmd.output().await.map_err(|e| anyhow!("spawning discovery run: {e}"))?;

    let dotd = read_one_depfile(&outdir)?;
    let readset = parse_readset(&dotd);
    let _ = std::fs::remove_dir_all(&outdir);

    if readset.is_empty() {
        bail!(
            "empty read-set (rc={:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(readset)
}

fn read_one_depfile(dir: &Utf8Path) -> Result<String> {
    for entry in std::fs::read_dir(dir).map_err(|e| anyhow!("readdir {dir}: {e}"))? {
        let path = entry.map_err(|e| anyhow!("dirent: {e}"))?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("d") {
            return std::fs::read_to_string(&path).map_err(|e| anyhow!("read {path:?}: {e}"));
        }
    }
    bail!("no depfile emitted in {dir}")
}

/// Build (once, then cache by content) the `rust-base` stage as a plain runnable image so the
/// discovery run uses the exact toolchain that compiled the deps.
async fn ensure_discovery_image(green: &Green) -> Result<String> {
    let tag =
        format!("supergreen-discovery:{}", &sha256::digest(green.base.image_inline.as_str())[..16]);

    let mut chk = green.cmd()?;
    chk.arg("image").arg("inspect").arg(&tag);
    if chk.output().await.map(|o| o.status.success()).unwrap_or(false) {
        return Ok(tag);
    }

    info!("binarydepinfo: materializing discovery image {tag}");
    let ctx = env::temp_dir().join(format!("sg-disc-img-{}", std::process::id()));
    let ctx = Utf8PathBuf::from_path_buf(ctx).map_err(|p| anyhow!("non-utf8 tmp {p:?}"))?;
    std::fs::create_dir_all(&ctx).map_err(|e| anyhow!("mkdir {ctx}: {e}"))?;
    let df = ctx.join("Dockerfile");
    let mut cf = green.new_containerfile();
    cf.push(&green.base.image_inline);
    cf.write_to(&df)?;

    let mut cmd = green.cmd()?;
    cmd.arg("buildx").arg("build").arg("--target").arg(RST).arg("--load").arg("-t").arg(&tag);
    cmd.arg("-f").arg(df.as_str()).arg(ctx.as_str());
    let out = cmd.output().await.map_err(|e| anyhow!("spawning image build: {e}"))?;
    let _ = std::fs::remove_dir_all(&ctx);
    if !out.status.success() {
        bail!("building discovery image: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(tag)
}

#[cfg(test)]
mod tests {
    use super::{filter_args, parse_readset};

    #[test]
    fn readset_from_binary_dep_depinfo() {
        // binary-dep-depinfo lists the crate's own outputs + every dep artifact it opened.
        let dotd = r#"
/t/release/deps/libsettings-4cada2a0109ca0f0.rmeta: /t/release/deps/libanyhow-219d1dd86ffeab69.rmeta /t/release/deps/libfs-57d1ed0fc10a713d.rmeta crates/settings/src/settings.rs
/t/release/deps/settings-4cada2a0109ca0f0.d: /t/release/deps/libanyhow-219d1dd86ffeab69.rmeta
/t/release/deps/libanyhow-219d1dd86ffeab69.rmeta:
crates/settings/src/settings.rs:
"#;
        let rs = parse_readset(dotd);
        assert!(rs.contains("libanyhow-219d1dd86ffeab69.rmeta"));
        assert!(rs.contains("libfs-57d1ed0fc10a713d.rmeta"));
        assert!(rs.contains("libsettings-4cada2a0109ca0f0.rmeta")); // own output, harmless
        assert!(!rs.iter().any(|f| f.ends_with(".rs"))); // sources excluded
        assert!(!rs.iter().any(|f| f.ends_with(".d")));
    }

    #[test]
    fn filter_drops_emit_outdir_and_incremental() {
        let flags: Vec<String> = [
            "--crate-name", "settings",
            "--emit=dep-info,metadata,link",
            "--out-dir", "/t/release/deps",
            "-C", "incremental=/t/release/incremental",
            "-C", "metadata=e68a1d07b4913b58",
            "--error-format=json",
            "--extern", "anyhow=/t/release/deps/libanyhow-219d1dd86ffeab69.rmeta",
            "crates/settings/src/settings.rs",
        ]
        .into_iter()
        .map(ToOwned::to_owned)
        .collect();

        let got = filter_args(&flags);
        assert!(!got.iter().any(|a| a.starts_with("--emit")), "{got:?}");
        assert!(!got.iter().any(|a| a == "--out-dir" || a == "/t/release/deps"), "{got:?}");
        assert!(!got.iter().any(|a| a.starts_with("incremental=")), "{got:?}");
        assert!(!got.iter().any(|a| a.starts_with("--error-format")), "{got:?}");
        // kept: crate-name, -C metadata, --extern + value, input
        assert!(got.iter().any(|a| a == "metadata=e68a1d07b4913b58"), "{got:?}");
        assert!(got.iter().any(|a| a.starts_with("anyhow=")), "{got:?}");
        assert!(got.iter().any(|a| a.ends_with("settings.rs")), "{got:?}");
    }
}
