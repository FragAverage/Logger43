//! DS2 request/response transport over a half-duplex K-line.
//!
//! Sequence per request (docs/PROTOCOL.md §3, after EdiabasLib `TransDs2`):
//! regen wait → discard stale input → write → read + verify echo → read header with response
//! timeout (skipping garbage until the DME address) → read body with inter-byte timeout →
//! verify length/checksum/address → check status byte. Any error after the write flushes the
//! input until the line is quiet, which re-synchronises the next request.

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde::Serialize;
use thiserror::Error;

use crate::ds2::{self, FrameError, Response};
use crate::safety::{self, Request, RequestKind, SafetyError};

/// A byte pipe to the cable. Implemented by `serial::SerialLink` and `sim::SimLink`.
pub trait Link: Send {
    /// Writes all bytes and waits until they have been handed to the wire.
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    /// Reads what is available, blocking at most one short poll interval. `Ok(0)` = nothing yet.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn clear_input(&mut self) -> io::Result<()>;
    fn description(&self) -> String;
}

impl<L: Link + ?Sized> Link for Box<L> {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write_all(bytes)
    }
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (**self).read(buf)
    }
    fn clear_input(&mut self) -> io::Result<()> {
        (**self).clear_input()
    }
    fn description(&self) -> String {
        (**self).description()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EchoMode {
    /// K+DCAN and other K-line cables: every sent byte comes back. Verified and stripped.
    Expect,
    /// Interfaces that suppress the echo themselves.
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransportConfig {
    pub baud: u32,
    /// Max wait for the first response byte. BMW MS430DS0 `TimeoutStd` = 2000 ms.
    pub response_timeout: Duration,
    /// Max gap inside a telegram. BMW MS430DS0 `TimeoutTelEnd` = 20 ms.
    pub inter_byte_timeout: Duration,
    /// Extra allowance for USB-serial buffering. MS4X recommends a 16 ms FTDI latency timer,
    /// so a chunk may legitimately arrive up to that much later than on a real UART.
    pub usb_latency: Duration,
    /// Echo wait beyond the wire time. EdiabasLib `EchoTimeout` = 100 ms.
    pub echo_timeout: Duration,
    /// Minimum idle time after a response before the next request. BMW MS430DS0 = 100 ms;
    /// RomRaider/TunerPro poll without it (docs/PROTOCOL.md §7.3).
    pub regen_time: Duration,
    /// Retries for `transact` on recoverable errors. BMW MS430DS0 `xreps 2`.
    pub retries: u8,
    pub echo: EchoMode,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            baud: 9600,
            response_timeout: Duration::from_millis(2000),
            inter_byte_timeout: Duration::from_millis(20),
            usb_latency: Duration::from_millis(16),
            echo_timeout: Duration::from_millis(100),
            regen_time: Duration::from_millis(100),
            retries: 2,
            echo: EchoMode::Expect,
        }
    }
}

impl TransportConfig {
    /// Time for `n` bytes on the wire at 8E1 (11 bits per byte).
    pub fn wire_time(&self, n: usize) -> Duration {
        Duration::from_secs_f64(n as f64 * 11.0 / self.baud as f64)
    }

    fn gap(&self) -> Duration {
        self.inter_byte_timeout + self.usb_latency
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TraceDir {
    Tx,
    Echo,
    Rx,
    /// Bytes thrown away while re-synchronising.
    Discarded,
    Error,
}

impl TraceDir {
    pub fn tag(self) -> &'static str {
        match self {
            TraceDir::Tx => "TX",
            TraceDir::Echo => "EC",
            TraceDir::Rx => "RX",
            TraceDir::Discarded => "XX",
            TraceDir::Error => "ER",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TraceEvent {
    pub time: DateTime<Local>,
    pub dir: TraceDir,
    pub bytes: Vec<u8>,
    pub note: Option<String>,
    /// Request start → last byte, on RX events.
    pub rtt_ms: Option<f64>,
    /// XOR checksum validity, on RX events.
    pub checksum_ok: Option<bool>,
}

impl TraceEvent {
    /// `2026-10-06T19:15:23.104 TX 12 05 0B 03 1F  (note)`
    pub fn to_line(&self) -> String {
        let mut s = format!(
            "{} {} {}",
            self.time.format("%Y-%m-%dT%H:%M:%S%.3f"),
            self.dir.tag(),
            ds2::hex(&self.bytes)
        );
        if let Some(n) = &self.note {
            s.push_str("  ");
            s.push_str(n);
        }
        s
    }
}

pub type TraceSink = Box<dyn FnMut(&TraceEvent) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Stage {
    Echo,
    Header,
    Body,
}

#[derive(Debug, Clone, Error, Serialize)]
pub enum TransportError {
    #[error("{0}")]
    Safety(#[from] SafetyError),
    #[error("serial link lost: {0}")]
    Disconnected(String),
    #[error("no echo from K-line: cable not powered by the car (OBD pin 16), or not plugged in")]
    NoEcho,
    #[error("echo mismatch: sent [{}] got [{}]", ds2::hex(sent), ds2::hex(got))]
    EchoMismatch { sent: Vec<u8>, got: Vec<u8> },
    #[error("timeout waiting for {stage:?} ({} bytes: [{}])", partial.len(), ds2::hex(partial))]
    Timeout { stage: Stage, partial: Vec<u8> },
    #[error("malformed response: {error} [{}]", ds2::hex(bytes))]
    Malformed { error: FrameError, bytes: Vec<u8> },
    #[error("negative response 0x{status:02X} {text}")]
    Negative {
        status: u8,
        text: &'static str,
        bytes: Vec<u8>,
    },
}

impl TransportError {
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            TransportError::Timeout { .. } | TransportError::NoEcho
        )
    }
    pub fn is_checksum(&self) -> bool {
        matches!(
            self,
            TransportError::Malformed {
                error: FrameError::Checksum { .. },
                ..
            }
        )
    }
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            TransportError::Disconnected(_) | TransportError::Safety(_)
        )
    }
}

/// One successful request/response.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub kind: RequestKind,
    pub request: Vec<u8>,
    pub response: Response,
    pub sent_at: DateTime<Local>,
    pub received_at: DateTime<Local>,
    /// Start of write → last response byte.
    pub rtt: Duration,
    /// End of echo → first response byte (includes USB latency).
    pub ecu_delay: Duration,
    /// Garbage bytes skipped before the response header.
    pub discarded: usize,
    pub attempts: u8,
}

pub struct Transport<L: Link> {
    link: L,
    cfg: TransportConfig,
    pending: Vec<u8>,
    last_activity: Option<Instant>,
    trace: Option<TraceSink>,
}

impl<L: Link> Transport<L> {
    pub fn new(link: L, cfg: TransportConfig) -> Self {
        Self {
            link,
            cfg,
            pending: Vec::new(),
            last_activity: None,
            trace: None,
        }
    }

    pub fn set_trace(&mut self, sink: Option<TraceSink>) {
        self.trace = sink;
    }

    pub fn config(&self) -> &TransportConfig {
        &self.cfg
    }

    pub fn config_mut(&mut self) -> &mut TransportConfig {
        &mut self.cfg
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    /// Request with up to `cfg.retries` retries on recoverable errors (timeouts, corruption).
    pub fn transact(&mut self, req: &Request) -> Result<Exchange, TransportError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.transact_once(req) {
                Ok(mut ex) => {
                    ex.attempts = attempt;
                    return Ok(ex);
                }
                Err(e) if e.is_fatal() || matches!(e, TransportError::Negative { .. }) => {
                    return Err(e)
                }
                Err(e) if attempt > self.cfg.retries => return Err(e),
                Err(_) => continue,
            }
        }
    }

    /// Exactly one request on the wire, no retries. Used by the poller so every failure counts.
    pub fn transact_once(&mut self, req: &Request) -> Result<Exchange, TransportError> {
        let frame = req.frame();
        safety::check_frame(frame)?; // defence in depth: nothing leaves without the allow-list

        if let Some(t) = self.last_activity {
            let since = t.elapsed();
            if since < self.cfg.regen_time {
                thread::sleep(self.cfg.regen_time - since);
            }
        }

        self.pending.clear();
        let stale = self.drain_available()?;
        if !stale.is_empty() {
            self.emit(
                TraceDir::Discarded,
                &stale,
                Some("stale input before request".into()),
            );
        }

        let result = self.exchange(req, frame);
        self.last_activity = Some(Instant::now());
        if let Err(e) = &result {
            if !e.is_fatal() {
                let junk = self.flush_until_quiet();
                if !junk.is_empty() {
                    self.emit(TraceDir::Discarded, &junk, Some("resync flush".into()));
                }
            }
            self.emit(TraceDir::Error, &[], Some(e.to_string()));
        }
        result
    }

    fn exchange(&mut self, req: &Request, frame: &[u8]) -> Result<Exchange, TransportError> {
        let start = Instant::now();
        let sent_at = Local::now();
        self.link.write_all(frame).map_err(disconnected)?;
        self.emit(TraceDir::Tx, frame, Some(req.describe()));

        if self.cfg.echo == EchoMode::Expect {
            let deadline = Instant::now()
                + self.cfg.wire_time(frame.len())
                + self.cfg.echo_timeout
                + self.cfg.usb_latency;
            let echo = self.read_n(frame.len(), deadline, None)?;
            if echo.is_empty() {
                return Err(TransportError::NoEcho);
            }
            if echo.len() < frame.len() {
                return Err(TransportError::Timeout {
                    stage: Stage::Echo,
                    partial: echo,
                });
            }
            if echo != frame {
                return Err(TransportError::EchoMismatch {
                    sent: frame.to_vec(),
                    got: echo,
                });
            }
            self.emit(TraceDir::Echo, &echo, None);
        }
        let echo_done = Instant::now();

        // Header: skip anything that is not the DME address (resynchronisation).
        let header_deadline = echo_done + self.cfg.response_timeout;
        let mut skipped = Vec::new();
        loop {
            let b = self.read_n(1, header_deadline, None)?;
            match b.first() {
                None => {
                    return Err(TransportError::Timeout {
                        stage: Stage::Header,
                        partial: skipped,
                    })
                }
                Some(&ds2::ADDR_DME) => break,
                Some(&other) => skipped.push(other),
            }
        }
        let first_byte = Instant::now();
        if !skipped.is_empty() {
            self.emit(
                TraceDir::Discarded,
                &skipped,
                Some("garbage before header".into()),
            );
        }

        let mut bytes = vec![ds2::ADDR_DME];
        let len = self.read_n(1, Instant::now() + self.cfg.gap(), None)?;
        let Some(&len) = len.first() else {
            return Err(TransportError::Timeout {
                stage: Stage::Header,
                partial: bytes,
            });
        };
        bytes.push(len);
        let len = len as usize;
        if len < ds2::MIN_FRAME_LEN {
            return Err(TransportError::Malformed {
                error: FrameError::TooShort(len),
                bytes,
            });
        }
        let body = self.read_n(
            len - 2,
            Instant::now() + self.cfg.gap(),
            Some(self.cfg.gap()),
        )?;
        let complete = body.len() == len - 2;
        bytes.extend_from_slice(&body);
        if !complete {
            return Err(TransportError::Timeout {
                stage: Stage::Body,
                partial: bytes,
            });
        }

        let rtt = start.elapsed();
        let checksum_ok = ds2::checksum(&bytes[..bytes.len() - 1]) == bytes[bytes.len() - 1];
        let parsed = Response::parse(&bytes, ds2::ADDR_DME);
        let status = parsed
            .as_ref()
            .map(|r| ds2::status_text(r.status()))
            .unwrap_or("-");
        self.emit_rx(&bytes, rtt, checksum_ok, format!("status {status}"));
        let response = parsed.map_err(|error| TransportError::Malformed {
            error,
            bytes: bytes.clone(),
        })?;
        if !response.is_ok() {
            let status = response.status();
            return Err(TransportError::Negative {
                status,
                text: ds2::status_text(status),
                bytes,
            });
        }
        Ok(Exchange {
            kind: req.kind().clone(),
            request: frame.to_vec(),
            response,
            sent_at,
            received_at: Local::now(),
            rtt,
            ecu_delay: first_byte.saturating_duration_since(echo_done),
            discarded: skipped.len(),
            attempts: 1,
        })
    }

    /// Reads up to `n` bytes. Returns fewer on timeout. With `inter_byte`, the deadline is
    /// extended every time data arrives (telegram-end timeout semantics).
    fn read_n(
        &mut self,
        n: usize,
        mut deadline: Instant,
        inter_byte: Option<Duration>,
    ) -> Result<Vec<u8>, TransportError> {
        let mut out = Vec::with_capacity(n);
        let mut buf = [0u8; 256];
        loop {
            let take = (n - out.len()).min(self.pending.len());
            out.extend(self.pending.drain(..take));
            if out.len() == n || Instant::now() >= deadline {
                return Ok(out);
            }
            let got = self.link.read(&mut buf).map_err(disconnected)?;
            if got > 0 {
                self.pending.extend_from_slice(&buf[..got]);
                if let Some(gap) = inter_byte {
                    deadline = deadline.max(Instant::now() + gap);
                }
            }
        }
    }

    fn drain_available(&mut self) -> Result<Vec<u8>, TransportError> {
        let mut out = Vec::new();
        let mut buf = [0u8; 256];
        loop {
            let got = self.link.read(&mut buf).map_err(disconnected)?;
            if got == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..got]);
        }
    }

    /// Reads and discards until the line has been quiet for one inter-byte gap (max 300 ms).
    fn flush_until_quiet(&mut self) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.pending);
        let hard_stop = Instant::now() + Duration::from_millis(300);
        let mut quiet_until = Instant::now() + self.cfg.gap();
        let mut buf = [0u8; 256];
        while Instant::now() < quiet_until && Instant::now() < hard_stop {
            match self.link.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    quiet_until = Instant::now() + self.cfg.gap();
                }
                Err(_) => break,
            }
        }
        let _ = self.link.clear_input();
        out
    }

    fn emit(&mut self, dir: TraceDir, bytes: &[u8], note: Option<String>) {
        if let Some(sink) = self.trace.as_mut() {
            sink(&TraceEvent {
                time: Local::now(),
                dir,
                bytes: bytes.to_vec(),
                note,
                rtt_ms: None,
                checksum_ok: None,
            });
        }
    }

    fn emit_rx(&mut self, bytes: &[u8], rtt: Duration, checksum_ok: bool, note: String) {
        if let Some(sink) = self.trace.as_mut() {
            let rtt_ms = rtt.as_secs_f64() * 1e3;
            sink(&TraceEvent {
                time: Local::now(),
                dir: TraceDir::Rx,
                bytes: bytes.to_vec(),
                note: Some(format!(
                    "RTT {rtt_ms:.1} ms, checksum {}, {note}",
                    if checksum_ok { "OK" } else { "BAD" }
                )),
                rtt_ms: Some(rtt_ms),
                checksum_ok: Some(checksum_ok),
            });
        }
    }
}

fn disconnected(e: io::Error) -> TransportError {
    TransportError::Disconnected(e.to_string())
}
