//! Simulated K-line + DME for development without a car.
//!
//! SIMULATION ONLY. It replays the real MS43 `0B 03` capture from bmwe46oil with a synthetic RPM
//! sweep, echoes like a K+DCAN cable, models 9600 8E1 wire time, and can inject faults. The ident
//! it returns is deliberately not a real BMW part number, so it can never be mistaken for a car.

use std::collections::VecDeque;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

use crate::ds2;
use crate::transport::Link;

/// Real MS43 warm-idle response to `12 05 0B 03` (tomicooler/bmwe46oil).
const CAPTURE_0B03: [u8; 45] = [
    0x12, 0x2D, 0xA0, 0x02, 0xC3, 0x00, 0x00, 0x00, 0x00, 0x38, 0x00, 0x38, 0x99, 0xBB, 0xB5, 0xA7,
    0xC8, 0x01, 0x88, 0xFC, 0x4E, 0x64, 0x35, 0x9C, 0x79, 0x88, 0x7C, 0x2F, 0x7C, 0x55, 0xFE, 0xFE,
    0xFE, 0xFE, 0x14, 0x47, 0x09, 0x76, 0x05, 0x05, 0x0D, 0x2E, 0x90, 0x86, 0xC2,
];

/// Simulated ident: "SIM4300" is not a BMW part number.
pub const SIM_PART_NUMBER: &str = "SIM4300";

#[derive(Debug, Clone, Default)]
pub struct SimFaults {
    /// Corrupt the checksum of every Nth response.
    pub bad_checksum_every: Option<u32>,
    /// Do not answer every Nth request.
    pub no_response_every: Option<u32>,
    /// Prepend garbage bytes to every Nth response.
    pub garbage_every: Option<u32>,
    /// After N requests, behave like an unplugged USB cable.
    pub disconnect_after: Option<u32>,
    /// Ident to report instead of `SIM_PART_NUMBER` (for exercising the part-number gate).
    pub part_number: Option<String>,
}

pub struct SimLink {
    queue: VecDeque<(Instant, u8)>,
    byte_time: Duration,
    /// Simulated ECU think time before the first response byte.
    pub ecu_delay: Duration,
    faults: SimFaults,
    requests: u32,
    started: Instant,
}

impl SimLink {
    pub fn new(faults: SimFaults) -> Self {
        Self {
            queue: VecDeque::new(),
            byte_time: Duration::from_secs_f64(11.0 / 9600.0),
            ecu_delay: Duration::from_millis(12),
            faults,
            requests: 0,
            started: Instant::now(),
        }
    }

    fn every(n: Option<u32>, count: u32) -> bool {
        matches!(n, Some(n) if n > 0 && count.is_multiple_of(n))
    }

    fn respond(&self, req: &[u8]) -> Vec<u8> {
        let payload = &req[2..req.len() - 1];
        match payload {
            [0x00] => {
                let pn = self
                    .faults
                    .part_number
                    .clone()
                    .unwrap_or_else(|| SIM_PART_NUMBER.into());
                let mut ascii = format!("{pn:<7.7}").into_bytes();
                // remaining IDENT fields, synthetic
                ascii.extend_from_slice(b"0101012024260000000000000000999999990");
                let mut p = vec![0xA0];
                p.extend(ascii);
                ds2::build_frame(0x12, &p)
            }
            [0x0B, 0x03] => {
                let mut f = CAPTURE_0B03.to_vec();
                // synthetic triangle sweep 750 -> 6500 rpm over 8 s
                let t = self.started.elapsed().as_secs_f64() % 8.0;
                let phase = if t < 4.0 { t / 4.0 } else { (8.0 - t) / 4.0 };
                let rpm = (750.0 + 5750.0 * phase) as u16;
                f[3..5].copy_from_slice(&rpm.to_be_bytes());
                let n = f.len();
                f[n - 1] = ds2::checksum(&f[..n - 1]);
                f
            }
            [0x0B, 0x90] => {
                // synthetic: intake actual/target 0x9C, exhaust actual/target 0x79
                let data = [
                    0xA0, 0x9C, 0x9C, 0x00, 0x00, 0x00, 0x9C, 0x79, 0x79, 0x00, 0x00, 0x00, 0x79,
                ];
                ds2::build_frame(0x12, &data)
            }
            [0x06, 0x00, _, _, _, n] => {
                let mut p = vec![0xA0];
                p.extend(std::iter::repeat_n(0x80, *n as usize));
                ds2::build_frame(0x12, &p)
            }
            _ => ds2::build_frame(0x12, &[0xFF]),
        }
    }

    /// `about_to_write`: the next request would exceed the allowed count.
    fn check_connected(&self, about_to_write: bool) -> io::Result<()> {
        match self.faults.disconnect_after {
            Some(n) if self.requests > n || (about_to_write && self.requests >= n) => Err(
                io::Error::new(io::ErrorKind::BrokenPipe, "simulated USB disconnect"),
            ),
            _ => Ok(()),
        }
    }
}

impl Link for SimLink {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.check_connected(true)?;
        self.requests += 1;
        let start = self
            .queue
            .back()
            .map_or(Instant::now(), |(t, _)| (*t).max(Instant::now()));
        let mut t = start;
        for &b in bytes {
            t += self.byte_time;
            self.queue.push_back((t, b)); // K-line echo
        }
        // the write itself takes wire time
        thread::sleep(t.saturating_duration_since(Instant::now()));

        let n = self.requests;
        if Self::every(self.faults.no_response_every, n) {
            return Ok(());
        }
        let mut resp = self.respond(bytes);
        if Self::every(self.faults.bad_checksum_every, n) {
            let last = resp.len() - 1;
            resp[last] ^= 0x5A;
        }
        if Self::every(self.faults.garbage_every, n) {
            resp.splice(0..0, [0x00, 0xFF, 0x3C]);
        }
        t += self.ecu_delay;
        for b in resp {
            t += self.byte_time;
            self.queue.push_back((t, b));
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check_connected(false)?;
        let now = Instant::now();
        let mut n = 0;
        while n < buf.len() {
            match self.queue.front() {
                Some((t, b)) if *t <= now => {
                    buf[n] = *b;
                    n += 1;
                    self.queue.pop_front();
                }
                _ => break,
            }
        }
        if n == 0 {
            thread::sleep(Duration::from_millis(1));
        }
        Ok(n)
    }

    fn clear_input(&mut self) -> io::Result<()> {
        let now = Instant::now();
        self.queue.retain(|(t, _)| *t > now);
        Ok(())
    }

    fn description(&self) -> String {
        "SIMULATED K-line (no hardware)".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels;
    use crate::ms43::{EcuIdent, LayoutTrust};
    use crate::safety::{Request, StatusBlock};
    use crate::transport::{Transport, TransportConfig, TransportError};

    fn transport(faults: SimFaults) -> Transport<SimLink> {
        let cfg = TransportConfig {
            regen_time: Duration::ZERO,
            ..Default::default()
        };
        Transport::new(SimLink::new(faults), cfg)
    }

    #[test]
    fn end_to_end_ident_and_rpm() {
        let mut t = transport(SimFaults::default());
        let ex = t.transact(&Request::ident()).unwrap();
        let id = EcuIdent::parse(&ex.response);
        assert_eq!(id.bmw_part_number.as_deref(), Some(SIM_PART_NUMBER));
        assert_eq!(id.layout_trust(), LayoutTrust::Unknown);

        let ex = t
            .transact_once(&Request::status_block(StatusBlock::Measurements))
            .unwrap();
        let rpm = channels::find("rpm")
            .unwrap()
            .decode_block(&ex.response)
            .unwrap();
        assert!((750.0..=6500.0).contains(&rpm.value));
        // 5-byte request + echo + 45-byte response at 9600 8E1 is at least ~57 ms on the wire
        assert!(ex.rtt >= Duration::from_millis(55), "rtt {:?}", ex.rtt);
    }

    #[test]
    fn checksum_errors_are_detected_and_line_resyncs() {
        let mut t = transport(SimFaults {
            bad_checksum_every: Some(2),
            ..Default::default()
        });
        let req = Request::status_block(StatusBlock::Measurements);
        assert!(t.transact_once(&req).is_ok());
        let e = t.transact_once(&req).unwrap_err();
        assert!(e.is_checksum(), "{e}");
        assert!(t.transact_once(&req).is_ok());
    }

    #[test]
    fn garbage_before_header_is_skipped() {
        let mut t = transport(SimFaults {
            garbage_every: Some(1),
            ..Default::default()
        });
        let ex = t
            .transact_once(&Request::status_block(StatusBlock::Measurements))
            .unwrap();
        assert_eq!(ex.discarded, 3);
    }

    #[test]
    fn missing_response_times_out_then_recovers() {
        let mut t = transport(SimFaults {
            no_response_every: Some(2),
            ..Default::default()
        });
        t.config_mut().response_timeout = Duration::from_millis(150);
        let req = Request::status_block(StatusBlock::Measurements);
        assert!(t.transact_once(&req).is_ok());
        assert!(t.transact_once(&req).unwrap_err().is_timeout());
        assert!(t.transact_once(&req).is_ok());
        // transact() retries through a single dropped response
        assert!(t.transact(&req).is_ok());
    }

    #[test]
    fn disconnect_is_fatal() {
        let mut t = transport(SimFaults {
            disconnect_after: Some(1),
            ..Default::default()
        });
        let req = Request::status_block(StatusBlock::Measurements);
        assert!(t.transact(&req).is_ok());
        assert!(matches!(
            t.transact(&req),
            Err(TransportError::Disconnected(_))
        ));
    }
}
