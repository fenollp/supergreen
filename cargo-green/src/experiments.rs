use crate::green::Green;

macro_rules! ENV_EXPERIMENT {
    () => {
        "CARGOGREEN_EXPERIMENT"
    };
}

pub(crate) const EXPERIMENTS: &[&str] = &["finalpathnonprimary", "incremental", "repro"];

impl Green {
    pub(crate) fn finalpathnonprimary(&self) -> bool {
        self.experiment.iter().any(|ex| ex == "finalpathnonprimary")
    }

    pub(crate) fn incremental(&self) -> bool {
        self.experiment.iter().any(|ex| ex == "incremental")
    }

    pub(crate) fn repro(&self) -> bool {
        self.experiment.iter().any(|ex| ex == "repro")
    }
}
