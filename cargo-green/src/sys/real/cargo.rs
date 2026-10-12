use crate::sys::Cargo;

pub(crate) struct RealCargo;

impl Cargo for RealCargo {
    fn stdout(&self, line: &str) {
        println!("{line}")
    }

    fn stderr(&self, line: &str) {
        eprintln!("{line}")
    }
}
