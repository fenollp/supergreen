use camino::{Utf8Path, Utf8PathBuf};

use crate::dirs::Paths;

impl Paths {
    /// Set to `$CARGO_TARGET_DIR/$PROFILE` when cross-compiling, `None` otherwise.
    /// Never to `$CARGO_TARGET_DIR/<target triple>/$PROFILE`: that's `target_path`.
    #[must_use]
    pub(crate) fn host_profile_dir(&self, target_path: &Utf8Path) -> Option<Utf8PathBuf> {
        let profile = target_path.file_name()?; // "release" | "debug" | $PROFILE
        let host = Utf8Path::new(self.target_dir().as_str()).join(profile);
        (host != target_path).then_some(host)
    }
}

/// Cross-compilation -safe way of making target paths.
#[must_use]
pub(crate) fn locate_path(
    f: impl Fn(&Utf8Path) -> Utf8PathBuf,
    target_path: &Utf8Path,
    host_path: Option<&Utf8Path>,
) -> Utf8PathBuf {
    if let Some(host_path) = host_path {
        let host = f(host_path);
        if host.exists() {
            return host;
        }
    }
    f(target_path) // `Md::from_file` can emit its helpful not-found message
}
