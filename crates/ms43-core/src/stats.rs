//! Request latency and rate statistics.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::transport::{Exchange, TransportError};

#[derive(Debug, Clone, Default, Serialize)]
pub struct StatsSnapshot {
    pub requests: u64,
    pub ok: u64,
    pub failed: u64,
    pub timeouts: u64,
    pub checksum_errors: u64,
    pub negative_responses: u64,
    pub other_errors: u64,
    pub discarded_bytes: u64,
    pub decoded_samples: u64,
    pub elapsed_s: f64,
    pub requests_per_s: f64,
    pub samples_per_s: f64,
    pub rtt_min_ms: Option<f64>,
    pub rtt_avg_ms: Option<f64>,
    pub rtt_max_ms: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Stats {
    started: Instant,
    snap: StatsSnapshot,
    rtt_sum: Duration,
    rtt_min: Option<Duration>,
    rtt_max: Option<Duration>,
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            snap: StatsSnapshot::default(),
            rtt_sum: Duration::ZERO,
            rtt_min: None,
            rtt_max: None,
        }
    }

    pub fn record_ok(&mut self, ex: &Exchange, samples: u64) {
        self.snap.requests += 1;
        self.snap.ok += 1;
        self.snap.decoded_samples += samples;
        self.snap.discarded_bytes += ex.discarded as u64;
        self.rtt_sum += ex.rtt;
        self.rtt_min = Some(self.rtt_min.map_or(ex.rtt, |m| m.min(ex.rtt)));
        self.rtt_max = Some(self.rtt_max.map_or(ex.rtt, |m| m.max(ex.rtt)));
    }

    pub fn record_err(&mut self, e: &TransportError) {
        self.snap.requests += 1;
        self.snap.failed += 1;
        if e.is_timeout() {
            self.snap.timeouts += 1;
        } else if e.is_checksum() {
            self.snap.checksum_errors += 1;
        } else if matches!(e, TransportError::Negative { .. }) {
            self.snap.negative_responses += 1;
        } else {
            self.snap.other_errors += 1;
        }
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let mut s = self.snap.clone();
        let el = self.started.elapsed().as_secs_f64();
        s.elapsed_s = el;
        if el > 0.0 {
            s.requests_per_s = s.requests as f64 / el;
            s.samples_per_s = s.decoded_samples as f64 / el;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        s.rtt_min_ms = self.rtt_min.map(ms);
        s.rtt_max_ms = self.rtt_max.map(ms);
        if s.ok > 0 {
            s.rtt_avg_ms = Some(ms(self.rtt_sum) / s.ok as f64);
        }
        s
    }
}
