//! Bounded, nonblocking forwarding. Pending bytes survive partial writes and backpressure.
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream};

pub(super) trait HalfClose: Read + Write {
    fn close_write(&mut self) -> io::Result<()>;
}
impl HalfClose for TcpStream {
    fn close_write(&mut self) -> io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}
impl HalfClose for ssh2::Channel {
    fn close_write(&mut self) -> io::Result<()> {
        self.send_eof().map_err(Into::into)
    }
}

#[derive(Default)]
struct Pending {
    bytes: Vec<u8>,
    offset: usize,
    eof: bool,
    closed: bool,
}
impl Pending {
    fn pump(
        &mut self,
        source: &mut impl HalfClose,
        target: &mut impl HalfClose,
    ) -> io::Result<bool> {
        let mut progressed = false;
        if self.bytes.is_empty() && !self.eof {
            let mut buffer = [0u8; 16 * 1024];
            match source.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    progressed = true;
                }
                Ok(size) => {
                    self.bytes.extend_from_slice(&buffer[..size]);
                    progressed = true;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        if !self.bytes.is_empty() {
            match target.write(&self.bytes[self.offset..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "隧道写入已关闭")),
                Ok(size) => {
                    self.offset += size;
                    progressed = true;
                    if self.offset == self.bytes.len() {
                        self.bytes.clear();
                        self.offset = 0;
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        if self.eof && self.bytes.is_empty() && !self.closed {
            match target.close_write() {
                Ok(()) => {
                    self.closed = true;
                    progressed = true;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(progressed)
    }
}

pub(super) struct Relay<L, R> {
    left: L,
    right: R,
    outgoing: Pending,
    incoming: Pending,
}
impl<L: HalfClose, R: HalfClose> Relay<L, R> {
    pub(super) fn new(left: L, right: R) -> Self {
        Self {
            left,
            right,
            outgoing: Pending::default(),
            incoming: Pending::default(),
        }
    }
    pub(super) fn into_parts(self) -> (L, R) {
        (self.left, self.right)
    }
    /// Returns (still active, made progress). Both directions drain before closing.
    pub(super) fn tick(&mut self) -> io::Result<(bool, bool)> {
        let outgoing = self.outgoing.pump(&mut self.left, &mut self.right)?;
        let incoming = self.incoming.pump(&mut self.right, &mut self.left)?;
        Ok((
            !(self.outgoing.closed && self.incoming.closed),
            outgoing || incoming,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct Throttled {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        stalled: bool,
        half_closed: bool,
    }
    impl Read for Throttled {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.input.read(bytes)
        }
    }
    impl Write for Throttled {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.stalled = !self.stalled;
            if self.stalled {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let size = bytes.len().min(3);
            self.output.extend_from_slice(&bytes[..size]);
            Ok(size)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl HalfClose for Throttled {
        fn close_write(&mut self) -> io::Result<()> {
            self.half_closed = true;
            Ok(())
        }
    }
    #[test]
    fn partial_writes_and_would_block_preserve_both_directions_and_half_close() {
        let make = |data: Vec<u8>| Throttled {
            input: Cursor::new(data),
            output: Vec::new(),
            stalled: false,
            half_closed: false,
        };
        let request = (0..40000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let response = b"response after request EOF".to_vec();
        let mut relay = Relay::new(make(request.clone()), make(response.clone()));
        for _ in 0..40000 {
            if !relay.tick().unwrap().0 {
                break;
            }
        }
        assert_eq!(relay.left.output, response);
        assert_eq!(relay.right.output, request);
        assert!(relay.left.half_closed && relay.right.half_closed);
    }
}
