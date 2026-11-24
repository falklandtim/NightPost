//! Test helpers for the sync engine, shared with the modules beside it.

/// A pretend IMAP server: plays back `replies` and records what was sent.
pub(crate) struct Script {
    pub(crate) replies: std::io::Cursor<Vec<u8>>,
    pub(crate) sent: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl std::io::Read for Script {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.replies.read(buf)
    }
}

impl std::io::Write for Script {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sent.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Script {
    /// A server that plays back `replies`, recording what's sent in `sent`.
    pub(crate) fn new(replies: &[u8], sent: std::rc::Rc<std::cell::RefCell<Vec<u8>>>) -> Self {
        Self { replies: std::io::Cursor::new(replies.to_vec()), sent }
    }
}
