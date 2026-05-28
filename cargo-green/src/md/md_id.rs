use std::fmt;

use anyhow::{bail, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

/// An ID unique to crate+version+crate-type+.. extracted from the rustc arg "extrafn"
#[derive(Debug, Copy, Clone, Deserialize, Serialize, Eq, PartialEq, Hash)]
#[serde(from = "String", into = "String")]
pub(crate) struct MdId(u64);

impl fmt::Display for MdId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:0>16}", format!("{:#x}", self.0).trim_start_matches("0x"))
    }
}

/// Used by serde
impl From<MdId> for String {
    fn from(metadata: MdId) -> Self {
        format!("{metadata}")
    }
}

impl From<u64> for MdId {
    fn from(raw: u64) -> Self {
        Self(raw)
    }
}

/// Used by serde
/// = help: the trait `From<std::string::String>` is not implemented for `md::MdId`
///         but trait `From<&str>` is implemented for it
// TODO? prefer &str impl
impl From<String> for MdId {
    fn from(hex: String) -> Self {
        hex.as_str().into()
    }
}
impl From<&str> for MdId {
    fn from(hex: &str) -> Self {
        assert_eq!(hex.len(), 16, "Unexpected MdId {hex:?}");
        Self(u64::from_str_radix(hex, 16).expect("16-digit hex str"))
    }
}

impl MdId {
    #[must_use]
    pub(crate) fn new(extrafn: &str) -> Self {
        assert!(extrafn.starts_with('-'), "Unexpected extrafn {extrafn:?}");
        extrafn[1..].to_owned().into()
    }

    /// E.g. libunicode_xid-c443c88a44e24bc6.rlib
    pub(crate) fn from_extern_filename(xtern: &str) -> Result<Self> {
        let Some(xtern) = xtern.split(['-', '.']).nth(1) else {
            bail!("BUG: expected extern to match ^lib[^.-]+-<mdid>.[^.]+$: {xtern}")
        };
        Ok(xtern.into())
    }

    /// E.g. OUT_DIR="/tmp/clis-vixargs_0-1-0/release/build/proc-macro-error-attr-de2f43c37de3bfce/out"
    #[must_use]
    pub(crate) fn from_out_dir_var(out_dir: &Utf8Path) -> Self {
        assert_eq!(out_dir.file_name(), Some("out"), "BUG: unexpected $OUT_DIR={out_dir} format");
        out_dir
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            //   => "proc-macro-error-attr-de2f43c37de3bfce"
            .rsplit('-')
            .next()
            .unwrap()
            //   => "de2f43c37de3bfce"
            .into()
    }

    #[must_use]
    pub(crate) fn path(&self, target_path: &Utf8Path) -> Utf8PathBuf {
        target_path.join(format!("{self}.toml"))
    }
}

#[test]
fn mdid_path() {
    assert_eq!(
        MdId(0xfb7fae2e3366cafc).path("some/path".into()),
        "some/path/fb7fae2e3366cafc.toml"
    );
}

#[test]
fn mdid_from_out_dir_var() {
    let out_dir_var = "$CARGO_TARGET_DIR/release/build/proc-macro-error-attr-de2f43c37de3bfce/out";
    assert_eq!(MdId::from_out_dir_var(out_dir_var.into()), "de2f43c37de3bfce".into());
}

#[test]
fn mdid_roundrobin() {
    let extrafn = "-dab737da4696ee62";
    let mdid = MdId::new(extrafn);
    assert_eq!(format!("-{mdid}"), extrafn);
}

#[test]
fn mdid_pads() {
    let mdid = MdId(0x572f583993dd3d9).to_string();
    assert_eq!(mdid, "0572f583993dd3d9");
    assert_eq!(mdid.len(), 16);
}

#[test]
fn mdid_ser() {
    let mdid = MdId(0x78d0c09fd98410d3);
    assert_eq!(mdid.to_string(), "78d0c09fd98410d3".to_owned());
    assert_eq!(&serde_json::to_string(&mdid).unwrap(), "\"78d0c09fd98410d3\"");
}

#[test]
fn mdid_de() {
    let hex = r#""78d0c09fd98410d3""#;
    assert_eq!(serde_json::from_str::<MdId>(hex).unwrap(), MdId(0x78d0c09fd98410d3));
}
