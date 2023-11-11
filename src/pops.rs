use camino::Utf8PathBuf;

pub(crate) trait Popped: Clone {
    #[must_use]
    fn pop(&mut self) -> bool;
    fn popped(&mut self, times: usize) -> Self
    where
        Self: Sized,
    {
        for _ in 0..times {
            assert!(self.pop());
        }
        self.clone()
    }
}
impl Popped for Utf8PathBuf {
    #[must_use]
    fn pop(&mut self) -> bool {
        self.pop()
    }
}
