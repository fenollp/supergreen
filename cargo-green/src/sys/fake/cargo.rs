use std::sync::{Arc, Mutex, MutexGuard};

use crate::sys::Cargo;

/// Keeps what cargo would have read, in order.
pub(crate) struct FakeCargo {
    lines: Mutex<Vec<(Stream, String)>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stream {
    Stdout,
    Stderr,
}

impl FakeCargo {
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { lines: Mutex::default() })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(Stream, String)>> {
        self.lines.lock().unwrap()
    }

    #[must_use]
    pub(crate) fn read(&self, stream: Stream) -> Vec<String> {
        self.lock().iter().filter(|(s, _)| *s == stream).map(|(_, l)| l.clone()).collect()
    }
}

impl Cargo for FakeCargo {
    fn stdout(&self, line: &str) {
        self.lock().push((Stream::Stdout, line.to_owned()));
    }

    fn stderr(&self, line: &str) {
        self.lock().push((Stream::Stderr, line.to_owned()));
    }
}
