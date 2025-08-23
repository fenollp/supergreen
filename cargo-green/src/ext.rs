use std::{ffi::OsStr, future::IntoFuture, process::Output, time::Duration};

use anyhow::{anyhow, Result};
use log::info;
use tokio::time::Timeout;

const SOME_TIME: Duration = Duration::from_secs(2);

#[track_caller]
pub(crate) fn timeout<F>(fut: F) -> Timeout<F::IntoFuture>
where
    F: IntoFuture,
{
    tokio::time::timeout(SOME_TIME, fut)
}

pub(crate) trait Popped: Clone {
    #[must_use]
    fn pop(&mut self) -> bool;

    #[must_use]
    fn popped(&mut self, times: usize) -> Self
    where
        Self: Sized,
    {
        for _ in 0..times {
            assert!(self.pop());
        }
        self.to_owned()
    }
}

impl Popped for camino::Utf8PathBuf {
    fn pop(&mut self) -> bool {
        self.pop()
    }
}

impl Popped for std::path::PathBuf {
    fn pop(&mut self) -> bool {
        self.pop()
    }
}

pub(crate) trait CommandExt {
    async fn exec(&mut self) -> Result<(bool, Vec<u8>, Vec<u8>)>;

    fn envs_string(&self, except: &[&OsStr]) -> String;

    #[must_use]
    fn show_unquoted(&self) -> String;

    #[must_use]
    fn show(&self) -> String {
        format!("`{}`", self.show_unquoted())
    }
}

impl CommandExt for tokio::process::Command {
    async fn exec(&mut self) -> Result<(bool, Vec<u8>, Vec<u8>)> {
        let call = self.show_unquoted();
        let envs = self.envs_string(&[]);

        info!("Calling `{envs} {call}`");
        eprintln!("Calling `{envs} {call}`");

        let Output { status, stdout, stderr } =
            self.output().await.map_err(|e| anyhow!("Failed to spawn `{envs} {call}`: {e}"))?;

        Ok((status.success(), stdout, stderr))
    }

    fn envs_string(&self, except: &[&OsStr]) -> String {
        self.as_std()
            .get_envs()
            .filter(|(k, _)| !except.contains(k))
            .map(|(k, v)| format!("{}={:?}", k.to_string_lossy(), v.unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn show_unquoted(&self) -> String {
        let this = self.as_std();
        format!(
            "{command} {args}",
            command = this.get_program().to_string_lossy(),
            args = this
                .get_args()
                .map(|x| x.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}
