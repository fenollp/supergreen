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
