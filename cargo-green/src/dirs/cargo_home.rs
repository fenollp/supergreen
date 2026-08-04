use anyhow::{Result, anyhow};
use camino::Utf8PathBuf;

pub(crate) fn cargo_home() -> Result<Utf8PathBuf> {
    home::cargo_home()
        .map_err(|e| anyhow!("Bad $CARGO_HOME or something: {e}"))?
        .try_into()
        .map_err(|e| anyhow!("Corrupted $CARGO_HOME path: {e}"))
}
