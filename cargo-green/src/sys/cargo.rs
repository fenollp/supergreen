/// What cargo reads from us, as `$RUSTC_WRAPPER`: build scripts' `cargo::` instructions on
/// stdout, rustc's diagnostics on stderr.
pub(crate) trait Cargo: Send + Sync {
    fn stdout(&self, line: &str);
    fn stderr(&self, line: &str);
}
