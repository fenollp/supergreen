//! Test-only assertion helpers.

/// [`snapbox::assert_data_eq!`] with path normalization off.
///
/// Containerfiles are full of `\` line continuations, which snapbox would otherwise
/// rewrite to `/` as if they were Windows path separators, so every `RUN` line would
/// compare equal no matter which one it was.
///
/// Like the macro it wraps, `SNAPSHOTS=overwrite` updates the inline `str![[…]]`.
macro_rules! assert_containerfile_eq {
    ($actual:expr, $expected:expr $(,)?) => {{
        let actual = ::snapbox::IntoData::into_data($actual);
        let expected = ::snapbox::IntoData::into_data($expected);
        ::snapbox::Assert::new()
            .action_env(::snapbox::assert::DEFAULT_ACTION_ENV)
            .normalize_paths(false)
            .eq(actual, expected);
    }};
}

pub(crate) use assert_containerfile_eq;
