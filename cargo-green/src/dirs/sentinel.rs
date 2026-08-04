use camino::Utf8PathBuf;

use crate::{PKG, VSN, dirs::tmp, green::Green};

impl Green {
    /// Includes builder container ID so its recreation retries builds
    pub(crate) fn sentinel_path(&self, name: &str, ext: &str) -> Utf8PathBuf {
        let builder = self.builder.id.as_deref().map(|id| format!("x{id:.12}")).unwrap_or_default();
        tmp().join(format!("{PKG}v{VSN}{builder}-{name}.{ext}"))
    }
}
