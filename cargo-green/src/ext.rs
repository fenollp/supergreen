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

pub(crate) trait ShowCmd {
    #[must_use]
    fn show_unquoted(&self) -> String;

    #[must_use]
    fn show(&self) -> String {
        format!("`{}`", self.show_unquoted())
    }
}

impl ShowCmd for std::process::Command {
    fn show_unquoted(&self) -> String {
        format!(
            "{command} {args}",
            command = self.get_program().to_string_lossy(),
            args = self
                .get_args()
                .map(|x| x.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

impl ShowCmd for tokio::process::Command {
    fn show_unquoted(&self) -> String {
        self.as_std().show_unquoted()
    }
}
