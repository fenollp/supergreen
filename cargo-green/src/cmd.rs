use std::{
    ffi::OsStr,
    ops::{Deref, DerefMut},
    process::{Output, Stdio},
};

use anyhow::{Result, anyhow};
use log::info;
use tokio::process::Command;

use crate::green::Green;

impl Green {
    pub(crate) fn cmd(&self) -> Result<Cmd> {
        let mut cmd = Command::new(self.runner.executable()?);
        cmd.kill_on_drop(true); // Underlying OS process dies with us
        cmd.stdin(Stdio::null());
        if false {
            cmd.arg("--debug");
        }
        cmd.env_clear(); // Pass all envs explicitly only
        cmd.env(DOCKER_BUILDKIT!(), "1"); // BuildKit is used by either runner

        if let Some(ref name) = self.builder.name {
            cmd.env(BUILDX_BUILDER!(), name);
        }

        for (var, val) in &self.runner_envs {
            if [BUILDX_BUILDER!(), DOCKER_BUILDKIT!()].contains(&var.as_str()) {
                continue;
            }
            info!("passing through runner setting: ${var}={val:?}");
            cmd.env(var, val);
        }

        Ok(Cmd { actual: cmd, verbose: self.verbose })
    }
}

pub(crate) struct Cmd {
    actual: Command,
    pub(crate) verbose: bool,
}

impl Cmd {
    pub(crate) async fn exec(&mut self) -> Result<(bool, Vec<u8>, Vec<u8>)> {
        let call = self.show();
        let except_envs = &[OsStr::new(PATH!())]; // PATH regardless of Runner
        let envs = self.envs_string(except_envs);

        info!("Calling {envs} {call}");
        if self.verbose {
            eprintln!("Calling {envs} {call}");
        }

        let Output { status, stdout, stderr } =
            self.output().await.map_err(|e| anyhow!("Failed to spawn {envs} {call}: {e}"))?;

        Ok((status.success(), stdout, stderr))
    }

    pub(crate) fn envs_string(&self, except: &[&OsStr]) -> String {
        self.as_std()
            .get_envs()
            .filter(|(k, _)| !except.contains(k))
            .map(|(k, v)| format!("{}={:?}", k.to_string_lossy(), v.unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) fn show(&self) -> String {
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

impl Deref for Cmd {
    type Target = Command;

    fn deref(&self) -> &Self::Target {
        &self.actual
    }
}

impl DerefMut for Cmd {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.actual
    }
}
