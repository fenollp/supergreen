use crate::green::Green;

pub(crate) const EXPERIMENTS: &[&str] = &[
    //
    "bitrepro",
    "buildscriptsources",
    "cachebuildkit",
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
    experiment!(bitrepro);
    experiment!(buildscriptsources);
    experiment!(cachebuildkit);
    experiment!(finalpathcomments);
    experiment!(finalpathnonprimary);
    experiment!(incremental);
    experiment!(repro);
}
