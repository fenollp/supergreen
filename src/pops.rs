use camino::Utf8PathBuf;

pub(crate) trait Popped: Clone {
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
    fn pop(&mut self) -> bool {
        self.pop()
    }
}
