use crate::green::Green;

macro_rules! ENV_EXPERIMENT {
    () => {
        "CARGOGREEN_EXPERIMENT"
    };
}

pub(crate) const EXPERIMENTS: &[&str] = &[
    //
    "finalpathcomments",
    "finalpathnonprimary",
    "incremental",
    "repro",
];

macro_rules! experiment {
    ($name:tt) => {
        pub(crate) fn $name(&self) -> bool {
            self.experiment.iter().any(|ex| ex == stringify!($name))
        }
    };
}

impl Green {
    experiment!(finalpathcomments);
    experiment!(finalpathnonprimary);
    experiment!(incremental);
    experiment!(repro);
}
