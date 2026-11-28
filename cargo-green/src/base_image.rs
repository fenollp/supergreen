use std::{
    hash::{Hash, Hasher},
    io::ErrorKind,
    sync::LazyLock,
};

use anyhow::{Result, bail};
use camino::Utf8Path;
use serde::{Deserialize, Serialize};
use siphasher::sip::SipHasher13;

use crate::{
    REPO,
    add::{Add, timestamp_what_run_changed},
    all_our_envs::RUSTUP_TOOLCHAIN,
    dirs::{Paths, replace_tokens},
    doctor::SETUP,
    image_uri::ImageUri,
    rustup::{CHECKSUMS, Pinned, VERSION},
    stage::Stage,
    sys::fs,
};

pub(crate) const CARGO_HOME: &str = "/usr/local/cargo";
const RUSTUP_HOME: &str = "/usr/local/rustup";

/// Default base image: `docker-image://docker.io/library/debian:trixie-slim`
pub(crate) static BASE_IMAGE: LazyLock<ImageUri> =
    LazyLock::new(|| ImageUri::std("debian:trixie-slim"));

/// Default base image, pre-locked (on 2026-04-28)
pub(crate) static BASE_IMAGE_LOCKED: LazyLock<ImageUri> = LazyLock::new(|| {
    BASE_IMAGE.lock("sha256:cedb1ef40439206b673ee8b33a46a03a0c9fa90bf3732f54704f99cb061d2c5a")
});

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaseImage {
    #[doc = envdocs!(CARGOGREEN_BASE_IMAGE)]
    #[serde(rename = "base-image")]
    pub(crate) image: ImageUri,
}

impl Default for BaseImage {
    fn default() -> Self {
        Self { image: BASE_IMAGE.clone() }
    }
}

/// A computed Rust base stage, along with the stages it needs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RustBase {
    pub(crate) stage: Stage,
    pub(crate) script: String,
}

/// A value that hashes as nothing when empty.
///
/// `#[derive(Hash)]` writes something for every field, empty or not (an empty `Vec` its length,
/// `None` its discriminant), so adding a field would rename every base. Wrapped in this, a new
/// field only renames the bases that set it.
///
/// `TAG` keeps apart equal values of different fields: give each field its own.
#[derive(Debug)]
struct IfAny<const TAG: u8, T>(T);

impl<const TAG: u8, T: Hash + Default + PartialEq> Hash for IfAny<TAG, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if self.0 != T::default() {
            TAG.hash(state);
            self.0.hash(state);
        }
    }
}

/// All that a Rust base stage is made of.
///
/// Its hash names the stage: changing it, or any field's value, renames bases. Add fields as
/// [`IfAny`] so only the bases using them get renamed.
#[derive(Debug, Hash)]
pub(crate) struct RustBaseSpec {
    toolchain: String,
    host: String,
    image: ImageUri,
    components: IfAny<1, Vec<String>>,
    target: IfAny<2, Option<String>>,
    add: Add,
    bitrepro: bool,
}

impl RustBaseSpec {
    pub(crate) fn new(
        Pinned { name, host }: &Pinned,
        image: &ImageUri,
        components: &[String],
        target: Option<&str>,
        add: &Add,
        bitrepro: bool,
    ) -> Self {
        let baseline = Add {
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/alpine3.20/Dockerfile
            apk: vec!["ca-certificates".to_owned(), "gcc".to_owned()],
            // From https://github.com/rust-lang/docker-rust/blob/d14e1ad7efeb270012b1a7e88fea699b1d1082f2/nightly/bullseye/slim/Dockerfile
            apt: vec!["ca-certificates".to_owned(), "gcc".to_owned(), "libc6-dev".to_owned()],
        };
        Self {
            toolchain: name.clone(),
            host: host.clone(),
            image: image.clone(),
            components: IfAny(components.to_vec()),
            target: IfAny(target.map(ToOwned::to_owned)),
            add: baseline.union(add),
            bitrepro,
        }
    }

    /// Bases that differ never share a name: it is a hash of all they're made of.
    ///
    /// SipHash, as its output doesn't change from a release of Rust (or of ours) to the next,
    /// unlike `std`'s `DefaultHasher`'s.
    fn stage(&self) -> Result<Stage> {
        let mut hasher = SipHasher13::new();
        self.hash(&mut hasher);
        Stage::new(&format!("rust-{:016x}", hasher.finish()))
    }

    /// Produce a recipe for a Rust base image
    ///
    /// * <https://rust-lang.github.io/rustup/environment-variables.html>
    /// * <https://rust-lang.github.io/rustup/concepts/toolchains.html#toolchain-specification>
    pub(crate) fn make_block(&self) -> Result<RustBase> {
        let Self {
            toolchain,
            host,
            image,
            components: IfAny(components),
            target: IfAny(target),
            add,
            bitrepro,
        } = self;
        let bitrepro = *bitrepro;
        let stage = self.stage()?;
        // TODO: multiplatformify (using auto ARG.s?)

        let Some(checksum) = CHECKSUMS.get(host) else {
            bail!("Unhandled rustup host {host:?} please report to {REPO}")
        };

        let components = if !components.is_empty() {
            format!(" --component {}", components.join(","))
        } else {
            "".to_owned()
        };

        // https://scribe.rip/com/better-programming/cross-compiling-rust-from-mac-to-linux-7fad5a454ab1
        // https://github.com/rust-cross/cargo-zigbuild/blob/f36f2f23c169937b680a963595e5002cf79f1cc8/src/zig.rs#L879
        // https://github.com/cross-rs/cross/pkgs/container/armv7-unknown-linux-musleabihf/68145882?tag=0.2.5
        // https://github.com/cross-rs/cross/blob/v0.2.5/docker/Dockerfile.armv7-unknown-linux-musleabihf
        // https://github.com/cross-rs/cross/wiki/Contributing#how-cross-works

        let target =
            target.as_ref().map(|target| format!(" --target {target}")).unwrap_or_default();

        // Rewrite host cargo/rustc so the base_image ones can be used
        // Also, propagate RUSTUP_TOOLCHAIN so Rustup skips looking for rust-toolchain.toml
        //   If you are trying to install a package that requires a specific nightly feature or a very new stable version,
        //   you must ensure your active toolchain meets those requirements before running the install command.
        //   Cargo won't auto-switch for you based on the dependency tree.

        let rustup_block = format!(
            r#"
FROM scratch AS rustup-{toolchain}
ADD --chmod=u+x --checksum=sha256:{checksum} \
  https://static.rust-lang.org/rustup/archive/{VERSION}/{host}/rustup-init /rustup-init
FROM --platform=$BUILDPLATFORM {base} AS {stage}
SHELL {shell:?}
ENV       CARGO_HOME={CARGO_HOME} \
         RUSTUP_HOME={RUSTUP_HOME} \
    RUSTUP_TOOLCHAIN={toolchain}
ENV CARGO=$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/cargo \
    RUSTC=$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustc \
     PATH=$CARGO_HOME/bin:$PATH
RUN \
  --mount=from=rustup-{toolchain},source=/rustup-init,dst=/rustup-init \
  <<'EOR'
/rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain {toolchain} --default-host {host}{target}{components}
chmod -R a+w $RUSTUP_HOME $CARGO_HOME
{sorted_components}{timestamp}EOR
"#,
            sorted_components = sort_components(bitrepro),
            timestamp = timestamp_what_run_changed(bitrepro),
            shell = ["/bin/sh", "-eux", "-c"],
            base = image.noscheme(),
        );
        // NOTE: rustup-init requires network TODO: turn rustup-init calls into ADDs

        // have buildkit call rustc with `--target $(adapted $TARGETPLATFORM)`, if not given `--target`
        // `adapted` translates buildkit platform format to rustc's
        //
        // maybe that's too naive
        //   do more research with `cargo cross`
        //
        // Use https://github.com/search?q=repo%3Across-rs/cross%20path%3Adockerfile&type=code images as auto base image?
        //
        // osx https://github.com/tonistiigi/xx?tab=readme-ov-file#external-sdk-support
        //
        // https://github.com/tonistiigi/xx?tab=readme-ov-file#rust
        // xx-cargo
        //
        // RUN apk add clang lld
        // ARG TARGETPLATFORM
        // RUN cargo build --target=$(xx-cargo --print-target-triple) --release --target-dir ./build && \
        //     xx-verify ./build/$(xx-cargo --print-target-triple)/release/hello_cargo

        // TODO: find a way to install packages without requiring Network (ie using only ADDs)
        // TODO: lock distro packages we install, somehow.
        //   https://github.com/reproducible-containers/repro-sources-list.sh
        //   https://github.com/reproducible-containers/repro-pkg-cache
        //   https://github.com/reproducible-containers/repro-get

        let script = add.as_block(&rustup_block, bitrepro);

        Ok(RustBase { stage, script })
    }
}

/// `rustup` installs components concurrently, listing them in download-order.
///
/// See (closed) [fix(dist): write lib/rustlib/components in sorted order](https://github.com/rust-lang/rustup/pull/5199)
fn sort_components(bitrepro: bool) -> &'static str {
    if bitrepro {
        "for f in $RUSTUP_HOME/toolchains/*/lib/rustlib/components; do sort -o \"$f\" \"$f\"; done\n"
    } else {
        ""
    }
}

impl Paths {
    pub(crate) fn rewrite_cargo_home(&self, path: &str) -> String {
        path.replace(CARGO_HOME, "$CARGO_HOME").replace(self.cargo_home.as_str(), "$CARGO_HOME")
    }

    pub(crate) fn un_rewrite_cargo_home(&self, txt: &str) -> String {
        replace_tokens(txt, CARGO_HOME, self.cargo_home.as_str(), false)
    }
}

pub(crate) fn rewrite_rustup_home(val: &str) -> String {
    let val = val.replacen(RUSTUP_HOME, "$RUSTUP_HOME", 1);
    const DIR: &str = ".rustup";
    if let Some(pos) = val.find(DIR) {
        return "$RUSTUP_HOME".to_owned() + &val[(pos + DIR.len())..];
    }
    val
}

impl Paths {
    /// Within the guest runner, [CARGO_HOME] is always `/usr/local/cargo`. The files the runner
    /// produces contains this guest path that can't all be rewritten. Text gets rewritten (STDIO
    /// and `.d` files). Binary artifacts (rlib, rmeta, executables) can't be edited.
    /// The link makes those paths lead to real files on the host.
    pub(crate) fn setup(&self) -> Result<()> {
        let _ = fs().create_dir_all(&self.cargo_home);

        let (guest, host) = (Utf8Path::new(CARGO_HOME), &self.cargo_home);
        if !fs().exists(guest) {
            eprintln!("Execute the following commands, or pipe them with: `{SETUP}`");
            eprintln!();
            let cmd = format!("ln -s {host} {guest}");
            println!("{cmd}");
            eprintln!();
            if let Err(e) = fs().symlink_dir(host, guest)
                && e.kind() != ErrorKind::AlreadyExists
            {
                bail!(
                    "Trying to ensure guest $CARGO_HOME is followable from host, but:
Could not `{cmd}`:
    {e}

Please try:
    {SETUP}
"
                )
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod guest_link {
    use anyhow::Result;

    use super::Paths;
    use crate::sys::{Fs, Sys, fake::FakeFs};

    fn paths() -> Paths {
        Paths { cargo_home: "/home/u/.cargo".into(), ..Default::default() }
    }

    #[test]
    fn cannot_be_made_without_root_and_we_say_how() {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });
        fs.readonly("/usr/local");

        let err = paths().setup().unwrap_err().to_string();
        assert!(err.contains("Could not `ln -s /home/u/.cargo /usr/local/cargo`"), "{err}");
        assert!(err.contains("supergreen setup"), "{err}");
        assert!(!fs.exists("/usr/local/cargo".into()));
    }

    #[test]
    fn gets_made_when_allowed() -> Result<()> {
        let fs = FakeFs::new();
        let _guard = Sys::install(Sys { fs: fs.clone(), ..Sys::fake() });

        paths().setup()?;
        assert_eq!(fs.read_link("/usr/local/cargo".into())?, "/home/u/.cargo");
        Ok(())
    }
}

#[cfg(test)]
mod duplo {
    use super::{Add, BASE_IMAGE_LOCKED, RustBase, RustBaseSpec};
    use crate::{md::merge_blocks, rustup::Pinned};

    const STABLE: &str = "1.94.0-x86_64-unknown-linux-gnu";

    fn block(components: &[&str], target: Option<&str>, add: Add) -> RustBase {
        block_with(components, target, add, false)
    }

    fn block_with(components: &[&str], target: Option<&str>, add: Add, bitrepro: bool) -> RustBase {
        let components: Vec<_> = components.iter().map(ToString::to_string).collect();
        let toolchain = Pinned::parse(STABLE).unwrap();
        let image = &BASE_IMAGE_LOCKED;
        RustBaseSpec::new(&toolchain, image, &components, target, &add, bitrepro)
            .make_block()
            .unwrap()
    }

    #[test]
    fn the_default_toolchain_stage() {
        assert_snapshots_eq!(
            block(&[], None, Add::default()).script,
            snapbox::str![[r#"
...
  https://static.rust-lang.org/rustup/archive/1.29.0/x86_64-unknown-linux-gnu/rustup-init /rustup-init
...
ENV       CARGO_HOME=/usr/local/cargo \
         RUSTUP_HOME=/usr/local/rustup \
    RUSTUP_TOOLCHAIN=1.94.0-x86_64-unknown-linux-gnu
ENV CARGO=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/cargo \
    RUSTC=$RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN/bin/rustc \
     PATH=$CARGO_HOME/bin:$PATH
RUN \
  --mount=from=rustup-1.94.0-x86_64-unknown-linux-gnu,source=/rustup-init,dst=/rustup-init \
  <<'EOR'
/rustup-init --verbose -y --no-modify-path --profile minimal --default-toolchain 1.94.0-x86_64-unknown-linux-gnu --default-host x86_64-unknown-linux-gnu
chmod -R a+w $RUSTUP_HOME $CARGO_HOME
EOR
ARG TARGETPLATFORM
RUN \
  --mount=from=xx,source=/usr/bin/xx-apk,dst=/usr/bin/xx-apk \
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
  <<'EOR'
if command -v apk >/dev/null 2>&1; then
                                                      xx-apk     add     --no-cache                 ca-certificates gcc
else
  xx-apt-get update && DEBIAN_FRONTEND=noninteractive xx-apt-get satisfy --no-install-recommends -y ca-certificates gcc libc6-dev
fi
EOR

"#]]
        );
    }

    #[test]
    fn components_and_a_cross_target() {
        let target = "armv7-unknown-linux-musleabihf";
        let block = block(&["clippy", "rustfmt"], Some(target), Add::default()).script;
        let patt = format!(
            " --default-host x86_64-unknown-linux-gnu --target {target} --component clippy,rustfmt\n"
        );
        assert!(block.contains(&patt), "no {patt} in {block}");
    }

    #[test]
    fn bases_that_differ_are_named_differently() {
        let host = block(&[], None, Add::default()).stage;
        assert_eq!(host, block(&[], None, Add::default()).stage);
        for other in [
            block(&[], Some("armv7-unknown-linux-musleabihf"), Add::default()),
            block(&["clippy"], None, Add::default()),
            block(&[], None, Add { apt: vec!["libssl-dev".into()], apk: vec![] }),
            block_with(&[], None, Add::default(), true),
        ] {
            assert_ne!(other.stage, host, "{}", other.script);
        }
    }

    #[test]
    fn merged_bases_share_their_common_stages() {
        let host = block(&[], None, Add::default());
        let arm = block(&[], Some("armv7-unknown-linux-musleabihf"), Add::default());
        assert_eq!(merge_blocks([host.script.as_str()]), host.script.trim());

        let merged = merge_blocks([host.script.as_str(), arm.script.as_str()]);
        for stage in [" AS xx\n", " AS rustup-1.94.0-x86_64-unknown-linux-gnu\n"] {
            assert_eq!(merged.matches(stage).count(), 1, "{stage} in {merged}");
        }
        for base in [&host.stage, &arm.stage] {
            assert!(merged.contains(&format!(" AS {base}\n")), "{base} in {merged}");
        }
    }

    #[test]
    fn extra_packages_extend_the_baseline() {
        let add = Add { apt: vec!["libssl-dev".into()], apk: vec!["openssl-dev".into()] };
        let made = block(&[], None, add);

        let block = made.script;
        assert!(block.contains("ca-certificates gcc openssl-dev"), "in {block}");
        assert!(block.contains("ca-certificates gcc libc6-dev libssl-dev"), "in {block}");
    }

    #[test]
    fn baseline_packages_are_never_optional() {
        let made = block(&[], None, Add::default());

        let block = made.script;
        assert!(block.contains("AS xx\n"), "in {}", block);
        assert!(block.contains("ca-certificates gcc libc6-dev"), "in {block}");
    }
}

#[cfg(test)]
mod a_rewrite_and_a_block_walk_into_a_bar {
    use super::{BASE_IMAGE_LOCKED, Paths, RustBaseSpec, rewrite_rustup_home};
    use crate::{add::Add, rustup::Pinned};

    #[test]
    fn test_rewrite_rustup_home() {
        use crate::all_our_envs::RUSTUP_TOOLCHAIN;
        assert_eq!(
            format!("$RUSTUP_HOME/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustdoc"),
            rewrite_rustup_home(&format!(
                "/home/runner/.rustup/toolchains/{RUSTUP_TOOLCHAIN}/bin/rustdoc"
            ))
        );
    }

    #[test]
    fn every_cargo_home_path_is_rewritten() {
        let paths = Paths { cargo_home: "/a/.cargo".into(), ..Default::default() };
        assert_eq!(
            paths.rewrite_cargo_home("/a/.cargo/x /a/.cargo/y"),
            "$CARGO_HOME/x $CARGO_HOME/y"
        );
    }

    #[test_case::test_matrix(["1.80.0-x86_64-unknown-linux-gnu", "nightly-2025-09-14-aarch64-apple-darwin"])]
    fn base_make_block(toolchain: &str) {
        let base_image = BASE_IMAGE_LOCKED.clone();
        let toolchain = Pinned::parse(toolchain).unwrap();
        let res = RustBaseSpec::new(&toolchain, &base_image, &[], None, &Add::default(), false)
            .make_block()
            .unwrap();
        assert!(
            res.script.contains(&format!(" {} AS {}\n", base_image.noscheme(), res.stage)),
            "In {}",
            res.script
        );
    }
}

#[cfg(test)]
mod same_ingredients_same_name {
    use std::hash::{Hash, Hasher};

    use siphasher::sip::SipHasher13;
    use test_case::test_case;

    use super::{Add, IfAny, RustBaseSpec};
    use crate::{image_uri::ImageUri, rustup::Pinned};

    const ARM: &str = "aarch64-unknown-linux-gnu";

    /// The values these assert on change whenever the way bases get named does: be it the hash
    /// function, how `std` or our types feed it, or what every base is made of.
    #[test_case(&[], None, &[], false => "rust-d0fb7386d9d97588"; "bare")]
    #[test_case(&["clippy"], None, &[], false => "rust-a2818368c1067067"; "a component")]
    #[test_case(&[], Some(ARM), &[], false => "rust-93241c74d1733f8a"; "a target")]
    #[test_case(&[], None, &["libssl-dev"], false => "rust-f1eac1f11ac570d7"; "a package")]
    #[test_case(&[], None, &[], true => "rust-1cfe615536ece031"; "bitrepro")]
    fn stage(components: &[&str], target: Option<&str>, apt: &[&str], bitrepro: bool) -> String {
        let toolchain = Pinned::parse("1.99.0-x86_64-unknown-linux-gnu").unwrap();
        let image = ImageUri::std("debian:trixie-slim");
        let owned = |xs: &[&str]| xs.iter().map(ToString::to_string).collect::<Vec<_>>();
        let add = Add { apt: owned(apt), apk: vec![] };
        RustBaseSpec::new(&toolchain, &image, &owned(components), target, &add, bitrepro)
            .stage()
            .unwrap()
            .to_string()
    }

    fn hash(x: impl Hash) -> u64 {
        let mut hasher = SipHasher13::new();
        x.hash(&mut hasher);
        hasher.finish()
    }

    #[derive(Hash, Default)]
    struct Before {
        old: String,
    }

    #[derive(Hash, Default)]
    struct DerivedAfter {
        old: String,
        new: Vec<String>,
    }

    #[derive(Hash)]
    struct After {
        old: String,
        new: IfAny<3, Vec<String>>,
        newer: IfAny<4, Option<String>>,
    }

    fn after(new: &[&str], newer: Option<&str>) -> u64 {
        let new = IfAny(new.iter().map(ToString::to_string).collect());
        hash(After { old: "x".into(), new, newer: IfAny(newer.map(ToOwned::to_owned)) })
    }

    #[test]
    fn an_empty_new_field_changes_nothing() {
        let before = hash(Before { old: "x".into() });
        assert_eq!(after(&[], None), before);
        assert_ne!(after(&["y"], None), before);
        assert_ne!(after(&[], Some("y")), before);
        assert_ne!(after(&["y"], None), after(&[], Some("y")));
    }

    #[test]
    fn which_deriving_alone_does_not_give() {
        let derived = DerivedAfter { old: "x".into(), new: vec![] };
        assert_ne!(hash(derived), hash(Before { old: "x".into() }));
    }
}
