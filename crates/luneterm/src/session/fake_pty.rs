//! A fake `Pty` for the session tests, built on the public `PtyWriter::new` (D20).

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lune_pty::{Pty, PtySize, PtyWriter, RawMode};

use super::threads::lock;

/// What the test can see of the fake.
#[derive(Debug, Default)]
pub(crate) struct Probe {
    pub(crate) resizes: Mutex<Vec<PtySize>>,
    pub(crate) exit_code: Mutex<Option<i32>>,
    pub(crate) dropped: AtomicBool,
    /// Every byte the writer accepted, in order.
    pub(crate) written: Mutex<Vec<u8>>,
    /// The length of every `write` call.
    pub(crate) write_sizes: Mutex<Vec<usize>>,
    /// Output bytes handed to the session's reader.
    pub(crate) output_read: AtomicU64,
    /// Bytes a `Gate::Discard` writer accepted.
    pub(crate) discarded: AtomicU64,
    /// A `Gate::Forever` writer is blocked.
    pub(crate) blocked: AtomicBool,
    /// What `process_id` answers (the shell).
    pub(crate) pid: Mutex<Option<u32>>,
    /// What `foreground_process` answers.
    pub(crate) foreground: Mutex<Option<u32>>,
    /// What `raw_mode` answers.
    pub(crate) raw: Mutex<RawMode>,
    /// Calls of `foreground_process` (one per probe).
    pub(crate) foreground_probes: AtomicU64,
    /// Calls of `raw_mode`.
    pub(crate) raw_probes: AtomicU64,
    /// Releases a `Gate::Forever` writer.
    release: (Mutex<bool>, Condvar),
}

impl Probe {
    pub(crate) fn written_len(&self) -> usize {
        lock(&self.written).len()
    }

    pub(crate) fn release(&self) {
        *lock(&self.release.0) = true;
        self.release.1.notify_all();
    }
}

/// When the fake child accepts input.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Gate {
    Open,
    /// Not until this many output bytes have been read (the child blocks writing output).
    AfterOutput(u64),
    /// The first write blocks until `Probe::release`.
    Forever,
    /// Accepts everything and records only the byte count.
    Discard,
}

struct FakeWriter {
    probe: Arc<Probe>,
    gate: Gate,
}

impl Write for FakeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.gate {
            Gate::Open => {}
            Gate::Discard => {
                self.probe
                    .discarded
                    .fetch_add(buf.len() as u64, Ordering::SeqCst);
                return Ok(buf.len());
            }
            Gate::AfterOutput(n) => {
                while self.probe.output_read.load(Ordering::SeqCst) < n {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            Gate::Forever => {
                self.probe.blocked.store(true, Ordering::SeqCst);
                let mut released = lock(&self.probe.release.0);
                while !*released {
                    released = self
                        .probe
                        .release
                        .1
                        .wait(released)
                        .unwrap_or_else(|e| e.into_inner());
                }
                return Err(io::Error::other("released"));
            }
        }
        lock(&self.probe.write_sizes).push(buf.len());
        lock(&self.probe.written).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// One step of a scripted output stream.
#[derive(Debug)]
pub(crate) enum Script {
    Data(Vec<u8>),
    /// A read error with this raw OS error (5 = EIO).
    Error(i32),
}

/// Output from a channel; the sender dropped = EOF.
pub(crate) struct ScriptReader {
    rx: Receiver<Script>,
    left: Vec<u8>,
    probe: Arc<Probe>,
}

impl Read for ScriptReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.left.is_empty() {
            match self.rx.recv() {
                Ok(Script::Data(d)) => self.left = d,
                Ok(Script::Error(code)) => return Err(io::Error::from_raw_os_error(code)),
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.left.len());
        buf[..n].copy_from_slice(&self.left[..n]);
        self.left.drain(..n);
        self.probe.output_read.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

/// `total` bytes of `pattern` repeated, as fast as the reader asks, then EOF.
pub(crate) struct FloodReader {
    pattern: Vec<u8>,
    pos: usize,
    remaining: u64,
    probe: Arc<Probe>,
}

impl Read for FloodReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf
            .len()
            .min(usize::try_from(self.remaining).unwrap_or(usize::MAX));
        for b in buf.iter_mut().take(n) {
            *b = self.pattern[self.pos];
            self.pos = (self.pos + 1) % self.pattern.len();
        }
        self.remaining -= n as u64;
        self.probe.output_read.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

pub(crate) struct FakePty {
    reader: Option<Box<dyn Read + Send>>,
    writer: PtyWriter,
    probe: Arc<Probe>,
}

impl FakePty {
    fn with_reader(
        make: impl FnOnce(Arc<Probe>) -> Box<dyn Read + Send>,
        gate: Gate,
    ) -> (Box<dyn Pty>, Arc<Probe>) {
        let probe = Arc::new(Probe::default());
        let writer = PtyWriter::new(Box::new(FakeWriter {
            probe: Arc::clone(&probe),
            gate,
        }));
        let pty = FakePty {
            reader: Some(make(Arc::clone(&probe))),
            writer,
            probe: Arc::clone(&probe),
        };
        (Box::new(pty), probe)
    }

    /// A child whose output the test sends through the returned channel.
    pub(crate) fn scripted(gate: Gate) -> (Box<dyn Pty>, Arc<Probe>, Sender<Script>) {
        let (tx, rx) = channel();
        let (pty, probe) = Self::with_reader(
            |probe| {
                Box::new(ScriptReader {
                    rx,
                    left: Vec::new(),
                    probe,
                })
            },
            gate,
        );
        (pty, probe, tx)
    }

    /// A child that writes `total` bytes of `pattern` as fast as it can, then exits.
    pub(crate) fn flood(pattern: &[u8], total: u64, gate: Gate) -> (Box<dyn Pty>, Arc<Probe>) {
        let pattern = pattern.to_vec();
        Self::with_reader(
            move |probe| {
                Box::new(FloodReader {
                    pattern,
                    pos: 0,
                    remaining: total,
                    probe,
                })
            },
            gate,
        )
    }
}

impl Drop for FakePty {
    fn drop(&mut self) {
        self.probe.dropped.store(true, Ordering::SeqCst);
    }
}

impl Pty for FakePty {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("reader taken"))
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)
    }

    fn resize(&mut self, size: PtySize) -> io::Result<()> {
        lock(&self.probe.resizes).push(size);
        Ok(())
    }

    fn try_exit_code(&mut self) -> io::Result<Option<i32>> {
        Ok(*lock(&self.probe.exit_code))
    }

    fn kill(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn take_reader(&mut self) -> io::Result<Box<dyn Read + Send>> {
        self.reader
            .take()
            .ok_or_else(|| io::Error::other("reader already taken"))
    }

    fn writer(&self) -> PtyWriter {
        self.writer.clone()
    }

    fn process_id(&self) -> Option<u32> {
        *lock(&self.probe.pid)
    }

    fn raw_mode(&self) -> RawMode {
        self.probe.raw_probes.fetch_add(1, Ordering::SeqCst);
        *lock(&self.probe.raw)
    }

    fn foreground_process(&self) -> Option<u32> {
        self.probe.foreground_probes.fetch_add(1, Ordering::SeqCst);
        *lock(&self.probe.foreground)
    }
}
