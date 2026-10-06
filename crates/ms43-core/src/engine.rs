//! Runtime: connection state machine, polling thread, logging and batched UI events.
//!
//! ```text
//!  poller thread ── transport ── K-line
//!      │ decode → Row
//!      ├─► ring buffer (recent history)
//!      ├─► Logger::push (bounded channel → CSV writer thread)
//!      └─► pending rows ─┐
//!  trace sink ─► debug ring + pending debug ─┤
//!                                            ▼
//!                         emitter thread (every 100 ms) ─► EventSink (Tauri events / CLI)
//! ```
//! The poller never waits on the UI or the disk.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::channels::ChannelDef;
use crate::ds2;
use crate::logger::{LogRequest, Logger, LoggerStatus, Row, RunMeta};
use crate::ms43::{EcuIdent, LayoutTrust};
use crate::plan::{self, Plan, PlanSummary};
use crate::safety::Request;
use crate::serial::{SerialConfig, SerialLink};
use crate::sim::{SimFaults, SimLink};
use crate::stats::{Stats, StatsSnapshot};
use crate::transport::{EchoMode, Link, TraceDir, TraceEvent, Transport, TransportConfig};

const RING_ROWS: usize = 20_000;
const PENDING_ROWS_MAX: usize = 5_000;
const DEBUG_RING: usize = 5_000;
const DEBUG_BATCH_MAX: usize = 1_000;
const EMIT_INTERVAL: Duration = Duration::from_millis(100);
const STATS_INTERVAL: Duration = Duration::from_millis(500);
/// Consecutive failed requests before the state goes to Error ("ECU not responding").
const FAILURES_BEFORE_ERROR: u32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectOptions {
    pub port: String,
    pub simulate: bool,
    pub regen_ms: u64,
    pub timeout_ms: u64,
    pub inter_byte_ms: u64,
    pub usb_latency_ms: u64,
    pub echo: bool,
    pub dtr: bool,
    pub rts: bool,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            port: String::new(),
            simulate: false,
            regen_ms: 100,
            timeout_ms: 2000,
            inter_byte_ms: 20,
            usb_latency_ms: 16,
            echo: true,
            dtr: false,
            rts: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConnState {
    Disconnected,
    OpeningSerial,
    InitialisingKLine,
    StartingDiagnosticSession,
    ReadingEcuId,
    Ready,
    Logging,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionStatus {
    pub state: ConnState,
    pub message: Option<String>,
    pub port: Option<String>,
    pub simulated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EcuInfo {
    pub ident: EcuIdent,
    pub layout_trust: LayoutTrust,
    pub description: Option<&'static str>,
    /// "430056" / "430069" when the part number is known.
    pub software_version: Option<&'static str>,
    pub raw_frame: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SampleBatch {
    pub columns: Vec<&'static str>,
    /// Seconds since connect.
    pub t: Vec<f64>,
    /// One array per column, aligned with `t`.
    pub values: Vec<Vec<Option<f64>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveStats {
    pub poll: StatsSnapshot,
    /// Rates over the last stats interval.
    pub requests_per_s: f64,
    pub samples_per_s: f64,
    pub rows_per_s: f64,
    pub rows_total: u64,
    /// Values of due jobs that failed (left empty in the row).
    pub dropped_samples: u64,
    /// Rows the UI buffer had to discard.
    pub ui_dropped_rows: u64,
    pub logger: Option<LoggerStatus>,
    pub plan: Option<PlanSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugValue {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub raw_hex: String,
    pub raw: i64,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DebugRecord {
    pub seq: u64,
    pub time: String,
    /// TX, EC (echo), RX, XX (discarded), ER (error), DEC (decoded values)
    pub dir: &'static str,
    pub bytes: String,
    pub note: Option<String>,
    pub rtt_ms: Option<f64>,
    pub checksum_ok: Option<bool>,
    pub decoded: Vec<DebugValue>,
}

impl DebugRecord {
    pub fn to_line(&self) -> String {
        let mut s = format!("{} {} {}", self.time, self.dir, self.bytes);
        if let Some(n) = &self.note {
            s.push_str("  ");
            s.push_str(n);
        }
        for d in &self.decoded {
            s.push_str(&format!(
                "\n    {} raw=0x{} ({}) decoded={:.4} {}",
                d.id, d.raw_hex, d.raw, d.value, d.unit
            ));
        }
        s
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolErrorEvent {
    pub time: String,
    pub message: String,
    pub fatal: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "payload", rename_all = "snake_case")]
pub enum EngineEvent {
    ConnectionStatus(ConnectionStatus),
    EcuInfo(EcuInfo),
    SampleBatch(SampleBatch),
    LoggerStats(LiveStats),
    ProtocolDebug(Vec<DebugRecord>),
    ProtocolError(ProtocolErrorEvent),
}

impl EngineEvent {
    /// Event name used by the Tauri layer.
    pub fn name(&self) -> &'static str {
        match self {
            EngineEvent::ConnectionStatus(_) => "connection_status",
            EngineEvent::EcuInfo(_) => "ecu_info",
            EngineEvent::SampleBatch(_) => "sample_batch",
            EngineEvent::LoggerStats(_) => "logger_stats",
            EngineEvent::ProtocolDebug(_) => "protocol_debug",
            EngineEvent::ProtocolError(_) => "protocol_error",
        }
    }
}

pub trait EventSink: Send + Sync {
    fn emit(&self, event: EngineEvent);
}

struct Shared {
    sink: Arc<dyn EventSink>,
    opts: ConnectOptions,
    session_start: Instant,
    stop: AtomicBool,
    status: Mutex<ConnectionStatus>,
    ecu: Mutex<Option<EcuInfo>>,
    selected: Mutex<Vec<String>>,
    replan: AtomicBool,
    plan: Mutex<Option<PlanSummary>>,
    columns: Mutex<Arc<Vec<&'static ChannelDef>>>,
    stats: Mutex<Stats>,
    rows_total: AtomicU64,
    dropped_samples: AtomicU64,
    ring: Mutex<VecDeque<Row>>,
    pending_rows: Mutex<Vec<Row>>,
    ui_dropped: AtomicU64,
    debug_enabled: AtomicBool,
    debug_seq: AtomicU64,
    debug_ring: Mutex<VecDeque<DebugRecord>>,
    debug_pending: Mutex<Vec<DebugRecord>>,
    errors_pending: Mutex<Vec<ProtocolErrorEvent>>,
    raw_file: Mutex<Option<(PathBuf, BufWriter<File>)>>,
    logger: Mutex<Option<Logger>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn t_now(&self) -> f64 {
        self.session_start.elapsed().as_secs_f64()
    }

    fn set_state(&self, state: ConnState, message: Option<String>) {
        let st = {
            let mut s = lock(&self.status);
            s.state = state;
            s.message = message;
            s.clone()
        };
        self.sink.emit(EngineEvent::ConnectionStatus(st));
    }

    fn state(&self) -> ConnState {
        lock(&self.status).state
    }

    fn running_state(&self) -> ConnState {
        if lock(&self.logger).is_some() {
            ConnState::Logging
        } else {
            ConnState::Ready
        }
    }

    fn push_debug(&self, mut rec: DebugRecord) {
        rec.seq = self.debug_seq.fetch_add(1, Ordering::Relaxed);
        if self.debug_enabled.load(Ordering::Relaxed) || lock(&self.raw_file).is_some() {
            lock(&self.debug_pending).push(rec.clone());
        }
        let mut ring = lock(&self.debug_ring);
        if ring.len() >= DEBUG_RING {
            ring.pop_front();
        }
        ring.push_back(rec);
    }

    fn push_error(&self, message: String, fatal: bool) {
        let mut e = lock(&self.errors_pending);
        if e.len() < 200 {
            e.push(ProtocolErrorEvent {
                time: now_iso(),
                message,
                fatal,
            });
        }
    }
}

fn now_iso() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
}

struct Running {
    shared: Arc<Shared>,
    poller: JoinHandle<()>,
    emitter: JoinHandle<()>,
}

pub struct Engine {
    sink: Arc<dyn EventSink>,
    running: Option<Running>,
    selected: Vec<String>,
    debug_enabled: bool,
}

impl Engine {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self {
            sink,
            running: None,
            selected: Vec::new(),
            debug_enabled: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|r| !r.poller.is_finished())
    }

    /// Starts the connection in the background; progress arrives as `connection_status` events.
    pub fn connect(&mut self, opts: ConnectOptions) -> Result<(), String> {
        if self.is_connected() {
            return Err("already connected".into());
        }
        self.teardown("reconnect");
        if !opts.simulate && opts.port.trim().is_empty() {
            return Err("no serial port selected".into());
        }
        let shared = Arc::new(Shared {
            sink: self.sink.clone(),
            session_start: Instant::now(),
            stop: AtomicBool::new(false),
            status: Mutex::new(ConnectionStatus {
                state: ConnState::Disconnected,
                message: None,
                port: Some(if opts.simulate {
                    "SIMULATOR".into()
                } else {
                    opts.port.clone()
                }),
                simulated: opts.simulate,
            }),
            opts,
            ecu: Mutex::new(None),
            selected: Mutex::new(self.selected.clone()),
            replan: AtomicBool::new(true),
            plan: Mutex::new(None),
            columns: Mutex::new(Arc::new(Vec::new())),
            stats: Mutex::new(Stats::new()),
            rows_total: AtomicU64::new(0),
            dropped_samples: AtomicU64::new(0),
            ring: Mutex::new(VecDeque::new()),
            pending_rows: Mutex::new(Vec::new()),
            ui_dropped: AtomicU64::new(0),
            debug_enabled: AtomicBool::new(self.debug_enabled),
            debug_seq: AtomicU64::new(0),
            debug_ring: Mutex::new(VecDeque::new()),
            debug_pending: Mutex::new(Vec::new()),
            errors_pending: Mutex::new(Vec::new()),
            raw_file: Mutex::new(None),
            logger: Mutex::new(None),
        });
        let s1 = shared.clone();
        let poller = thread::Builder::new()
            .name("ms43-poller".into())
            .spawn(move || poller_main(s1))
            .map_err(|e| e.to_string())?;
        let s2 = shared.clone();
        let emitter = thread::Builder::new()
            .name("ms43-emitter".into())
            .spawn(move || emitter_main(s2))
            .map_err(|e| e.to_string())?;
        self.running = Some(Running {
            shared,
            poller,
            emitter,
        });
        Ok(())
    }

    pub fn disconnect(&mut self) {
        self.teardown("disconnect");
        self.sink
            .emit(EngineEvent::ConnectionStatus(ConnectionStatus {
                state: ConnState::Disconnected,
                message: None,
                port: None,
                simulated: false,
            }));
    }

    fn teardown(&mut self, reason: &str) {
        if let Some(r) = self.running.take() {
            r.shared.stop.store(true, Ordering::SeqCst);
            let _ = r.poller.join();
            let _ = finish_logger(&r.shared, reason);
            let _ = r.emitter.join();
            lock(&r.shared.raw_file).take();
        }
    }

    pub fn status(&self) -> ConnectionStatus {
        match &self.running {
            Some(r) => lock(&r.shared.status).clone(),
            None => ConnectionStatus {
                state: ConnState::Disconnected,
                message: None,
                port: None,
                simulated: false,
            },
        }
    }

    pub fn ecu_info(&self) -> Option<EcuInfo> {
        self.running
            .as_ref()
            .and_then(|r| lock(&r.shared.ecu).clone())
    }

    /// Selects channels. Returns the plan that results for the connected DME (or for no DME).
    pub fn set_channels(&mut self, ids: Vec<String>) -> Result<PlanSummary, String> {
        if let Some(r) = &self.running {
            if lock(&r.shared.logger).is_some() {
                return Err("stop logging before changing channels".into());
            }
        }
        let part = self.ecu_info().and_then(|e| e.ident.bmw_part_number);
        let summary = plan::build(&ids, part.as_deref()).summary();
        self.selected = ids.clone();
        if let Some(r) = &self.running {
            *lock(&r.shared.selected) = ids;
            r.shared.replan.store(true, Ordering::SeqCst);
        }
        Ok(summary)
    }

    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    pub fn start_logging(&self, mut req: LogRequest) -> Result<LoggerStatus, String> {
        let r = self.running.as_ref().ok_or("not connected")?;
        let state = r.shared.state();
        if !matches!(state, ConnState::Ready | ConnState::Error) {
            return Err(format!("cannot start logging in state {state:?}"));
        }
        let mut slot = lock(&r.shared.logger);
        if slot.is_some() {
            return Err("already logging".into());
        }
        let columns = lock(&r.shared.columns).clone();
        if columns.is_empty() {
            return Err("no channels are being polled".into());
        }
        req.port = lock(&r.shared.status).port.clone().unwrap_or_default();
        let ident = lock(&r.shared.ecu).as_ref().map(|e| e.ident.clone());
        let settings = serde_json::json!({
            "connection": r.shared.opts,
            "plan": *lock(&r.shared.plan),
            "selected": *lock(&r.shared.selected),
        });
        *lock(&r.shared.stats) = Stats::new(); // run statistics start with the run
        let logger = Logger::start(
            &req,
            &columns,
            r.shared.t_now(),
            ident,
            r.shared.opts.simulate,
            settings,
        )
        .map_err(|e| format!("cannot start log: {e}"))?;
        let status = logger.status();
        *slot = Some(logger);
        drop(slot);
        if r.shared.state() == ConnState::Ready {
            r.shared.set_state(ConnState::Logging, None);
        }
        Ok(status)
    }

    pub fn stop_logging(&self) -> Result<RunMeta, String> {
        let r = self.running.as_ref().ok_or("not connected")?;
        let meta = finish_logger(&r.shared, "user stop")?.ok_or("not logging")?;
        if r.shared.state() == ConnState::Logging {
            r.shared.set_state(ConnState::Ready, None);
        }
        Ok(meta)
    }

    pub fn live_stats(&self) -> Option<LiveStats> {
        self.running
            .as_ref()
            .map(|r| snapshot_stats(&r.shared, 0.0, 0.0, 0.0))
    }

    pub fn set_debug(&mut self, enabled: bool) {
        self.debug_enabled = enabled;
        if let Some(r) = &self.running {
            r.shared.debug_enabled.store(enabled, Ordering::SeqCst);
        }
    }

    /// Streams every protocol record to a file until stopped (or disconnect).
    pub fn record_raw(&self, path: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
        let r = self.running.as_ref().ok_or("not connected")?;
        let mut slot = lock(&r.shared.raw_file);
        if let Some((p, mut f)) = slot.take() {
            let _ = f.flush();
            if path.is_none() {
                return Ok(Some(p));
            }
        }
        if let Some(p) = path {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let f = File::create(&p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
            *slot = Some((p.clone(), BufWriter::new(f)));
            return Ok(Some(p));
        }
        Ok(None)
    }

    /// Writes the in-memory debug history (last 5000 records) to a file.
    pub fn save_debug_history(&self, path: &Path) -> Result<usize, String> {
        let r = self.running.as_ref().ok_or("not connected")?;
        let ring = lock(&r.shared.debug_ring);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let mut f = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
        for rec in ring.iter() {
            writeln!(f, "{}", rec.to_line()).map_err(|e| e.to_string())?;
        }
        f.flush().map_err(|e| e.to_string())?;
        Ok(ring.len())
    }

    /// Recent rows (for a UI reload).
    pub fn recent(&self, seconds: f64) -> Option<SampleBatch> {
        let r = self.running.as_ref()?;
        let ring = lock(&r.shared.ring);
        let cutoff = r.shared.t_now() - seconds;
        let rows: Vec<Row> = ring.iter().filter(|row| row.t >= cutoff).cloned().collect();
        drop(ring);
        to_batches(rows).pop()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.teardown("application exit");
    }
}

fn finish_logger(shared: &Shared, reason: &str) -> Result<Option<RunMeta>, String> {
    let Some(logger) = lock(&shared.logger).take() else {
        return Ok(None);
    };
    let snap = lock(&shared.stats).snapshot();
    logger
        .stop(snap, reason)
        .map(Some)
        .map_err(|e| format!("error finishing log: {e}"))
}

// ------------------------------------------------------------------------------------------

fn poller_main(shared: Arc<Shared>) {
    let opts = shared.opts.clone();
    shared.set_state(ConnState::OpeningSerial, None);
    let link: Box<dyn Link> = if opts.simulate {
        Box::new(SimLink::new(SimFaults::default()))
    } else {
        let cfg = SerialConfig {
            dtr: opts.dtr,
            rts: opts.rts,
            ..Default::default()
        };
        match SerialLink::open(&opts.port, &cfg) {
            Ok(l) => Box::new(l),
            Err(e) => {
                let msg = format!("cannot open {}: {e}", opts.port);
                shared.push_error(msg.clone(), true);
                shared.set_state(ConnState::Error, Some(msg));
                return;
            }
        }
    };

    shared.set_state(
        ConnState::InitialisingKLine,
        Some("9600 baud 8E1, DS2 timing parameters (MS43 needs no wake-up)".into()),
    );
    let cfg = TransportConfig {
        response_timeout: Duration::from_millis(opts.timeout_ms),
        inter_byte_timeout: Duration::from_millis(opts.inter_byte_ms),
        usb_latency: Duration::from_millis(opts.usb_latency_ms),
        regen_time: Duration::from_millis(opts.regen_ms),
        echo: if opts.echo {
            EchoMode::Expect
        } else {
            EchoMode::None
        },
        ..Default::default()
    };
    let mut t = Transport::new(link, cfg);
    let trace_shared = shared.clone();
    t.set_trace(Some(Box::new(move |ev: &TraceEvent| {
        trace_shared.push_debug(DebugRecord {
            seq: 0,
            time: ev.time.format("%Y-%m-%dT%H:%M:%S%.3f").to_string(),
            dir: ev.dir.tag(),
            bytes: ds2::hex(&ev.bytes),
            note: ev.note.clone(),
            rtt_ms: ev.rtt_ms,
            checksum_ok: ev.checksum_ok,
            decoded: Vec::new(),
        });
        if ev.dir == TraceDir::Error {
            trace_shared.push_error(ev.note.clone().unwrap_or_default(), false);
        }
    })));

    shared.set_state(
        ConnState::StartingDiagnosticSession,
        Some("DS2 has no session: verifying K-line echo and a DME answer on address 0x12".into()),
    );
    let ex = match t.transact(&Request::ident()) {
        Ok(ex) => ex,
        Err(e) => {
            let msg = format!("no answer from MS43: {e}");
            shared.push_error(msg.clone(), true);
            shared.set_state(ConnState::Error, Some(msg));
            return;
        }
    };

    shared.set_state(ConnState::ReadingEcuId, None);
    let ident = EcuIdent::parse(&ex.response);
    let trust = ident.layout_trust();
    let info = EcuInfo {
        description: ident.description(),
        software_version: ident
            .bmw_part_number
            .as_deref()
            .and_then(crate::ms43::software_version),
        layout_trust: trust,
        raw_frame: ds2::hex(ex.response.bytes()),
        ident,
    };
    *lock(&shared.ecu) = Some(info.clone());
    shared.sink.emit(EngineEvent::EcuInfo(info.clone()));
    if trust == LayoutTrust::OtherDme {
        let msg = format!(
            "DME {:?} is a known non-MS43 Siemens DME; MS43 layouts would decode garbage. Not polling.",
            info.ident.bmw_part_number
        );
        shared.push_error(msg.clone(), true);
        shared.set_state(ConnState::Error, Some(msg));
        return;
    }
    let warn = (trust == LayoutTrust::Unknown).then(|| {
        format!(
            "DME part number {} is not on the known MS43 list; decoding with the MS43 layout",
            info.ident.bmw_part_number.as_deref().unwrap_or("?")
        )
    });
    shared.set_state(ConnState::Ready, warn.clone());
    let part = info.ident.bmw_part_number.clone();

    let mut plan = Plan::empty();
    let mut latest: Vec<Option<f64>> = Vec::new();
    let mut cycle: u64 = 0;
    let mut consecutive_failures = 0u32;

    while !shared.stop.load(Ordering::SeqCst) {
        if shared.replan.swap(false, Ordering::SeqCst) {
            let ids = lock(&shared.selected).clone();
            plan = plan::build(&ids, part.as_deref());
            latest = vec![None; plan.columns.len()];
            *lock(&shared.columns) = plan.columns.clone();
            *lock(&shared.plan) = Some(plan.summary());
            for u in &plan.unavailable {
                shared.push_error(format!("channel {} unavailable: {}", u.id, u.reason), false);
            }
            cycle = 0;
        }
        if plan.jobs.is_empty() {
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        let mut row_t: Option<f64> = None;
        let mut values = latest.clone();
        for job in plan.jobs.iter().filter(|j| j.due(cycle)) {
            match t.transact_once(&job.request) {
                Ok(ex) => {
                    consecutive_failures = 0;
                    row_t.get_or_insert_with(|| shared.t_now());
                    let bytes = ex.response.bytes();
                    let mut decoded = Vec::with_capacity(job.fields.len());
                    for f in &job.fields {
                        // never read the checksum byte as data
                        let v = (f.offset + f.dtype.size() < bytes.len())
                            .then(|| f.dtype.read(bytes, f.offset))
                            .flatten()
                            .map(|raw| (raw, f.def.convert(raw)));
                        match v {
                            Some((raw, d)) => {
                                values[f.column] = Some(d.value);
                                latest[f.column] = Some(d.value);
                                decoded.push(DebugValue {
                                    id: f.def.id,
                                    label: f.def.label,
                                    unit: f.def.unit,
                                    raw_hex: ds2::hex(&bytes[f.offset..f.offset + f.dtype.size()])
                                        .replace(' ', ""),
                                    raw,
                                    value: d.value,
                                });
                            }
                            None => {
                                values[f.column] = None;
                                shared.dropped_samples.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    lock(&shared.stats).record_ok(&ex, decoded.len() as u64);
                    shared.push_debug(DebugRecord {
                        seq: 0,
                        time: now_iso(),
                        dir: "DEC",
                        bytes: String::new(),
                        note: Some(format!("{} decoded", job.request.describe())),
                        rtt_ms: Some(ex.rtt.as_secs_f64() * 1e3),
                        checksum_ok: Some(true),
                        decoded,
                    });
                }
                Err(e) => {
                    lock(&shared.stats).record_err(&e);
                    for f in &job.fields {
                        values[f.column] = None;
                    }
                    shared
                        .dropped_samples
                        .fetch_add(job.fields.len() as u64, Ordering::Relaxed);
                    if e.is_fatal() {
                        let msg = format!("connection lost: {e}");
                        shared.push_error(msg.clone(), true);
                        let _ = finish_logger(&shared, "connection lost");
                        shared.set_state(ConnState::Error, Some(msg));
                        return;
                    }
                    consecutive_failures += 1;
                    if consecutive_failures == FAILURES_BEFORE_ERROR {
                        shared.set_state(
                            ConnState::Error,
                            Some(format!("ECU not responding ({consecutive_failures} failed requests); retrying")),
                        );
                    }
                }
            }
            if shared.stop.load(Ordering::SeqCst) {
                break;
            }
        }
        if consecutive_failures == 0 && shared.state() == ConnState::Error {
            shared.set_state(shared.running_state(), warn.clone());
        }
        cycle += 1;

        let Some(t_row) = row_t else {
            // whole cycle failed: back off a little so a dead line is not hammered
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        let row = Row {
            t: t_row,
            values,
            columns: plan.columns.clone(),
        };
        shared.rows_total.fetch_add(1, Ordering::Relaxed);
        if let Some(lg) = lock(&shared.logger).as_ref() {
            lg.push(row.clone());
        }
        {
            let mut p = lock(&shared.pending_rows);
            if p.len() < PENDING_ROWS_MAX {
                p.push(row.clone());
            } else {
                shared.ui_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut ring = lock(&shared.ring);
        if ring.len() >= RING_ROWS {
            ring.pop_front();
        }
        ring.push_back(row);
    }
    if shared.state() != ConnState::Error {
        shared.set_state(ConnState::Disconnected, None);
    }
}

fn to_batches(rows: Vec<Row>) -> Vec<SampleBatch> {
    let mut out: Vec<SampleBatch> = Vec::new();
    let mut current_cols: Option<Arc<Vec<&'static ChannelDef>>> = None;
    for row in rows {
        let same = current_cols
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &row.columns));
        if !same {
            current_cols = Some(row.columns.clone());
            out.push(SampleBatch {
                columns: row.columns.iter().map(|c| c.id).collect(),
                t: Vec::new(),
                values: vec![Vec::new(); row.columns.len()],
            });
        }
        let b = out.last_mut().unwrap();
        b.t.push(row.t);
        for (i, v) in row.values.into_iter().enumerate() {
            b.values[i].push(v);
        }
    }
    out
}

fn snapshot_stats(shared: &Shared, req_rate: f64, sample_rate: f64, row_rate: f64) -> LiveStats {
    LiveStats {
        poll: lock(&shared.stats).snapshot(),
        requests_per_s: req_rate,
        samples_per_s: sample_rate,
        rows_per_s: row_rate,
        rows_total: shared.rows_total.load(Ordering::Relaxed),
        dropped_samples: shared.dropped_samples.load(Ordering::Relaxed),
        ui_dropped_rows: shared.ui_dropped.load(Ordering::Relaxed),
        logger: lock(&shared.logger).as_ref().map(|l| l.status()),
        plan: lock(&shared.plan).clone(),
    }
}

fn emitter_main(shared: Arc<Shared>) {
    let mut last_stats = Instant::now();
    let mut prev = (0u64, 0u64, 0u64); // requests, samples, rows
    loop {
        let stopping = shared.stop.load(Ordering::SeqCst);
        thread::sleep(EMIT_INTERVAL);

        let rows: Vec<Row> = std::mem::take(&mut *lock(&shared.pending_rows));
        for batch in to_batches(rows) {
            shared.sink.emit(EngineEvent::SampleBatch(batch));
        }

        let debug: Vec<DebugRecord> = std::mem::take(&mut *lock(&shared.debug_pending));
        if !debug.is_empty() {
            if let Some((_, f)) = lock(&shared.raw_file).as_mut() {
                for rec in &debug {
                    let _ = writeln!(f, "{}", rec.to_line());
                }
            }
            if shared.debug_enabled.load(Ordering::Relaxed) {
                let skip = debug.len().saturating_sub(DEBUG_BATCH_MAX);
                shared.sink.emit(EngineEvent::ProtocolDebug(
                    debug.into_iter().skip(skip).collect(),
                ));
            }
        }

        for e in std::mem::take(&mut *lock(&shared.errors_pending)) {
            shared.sink.emit(EngineEvent::ProtocolError(e));
        }

        if last_stats.elapsed() >= STATS_INTERVAL || stopping {
            let dt = last_stats.elapsed().as_secs_f64();
            last_stats = Instant::now();
            let snap = snapshot_stats(&shared, 0.0, 0.0, 0.0);
            let now = (
                snap.poll.requests,
                snap.poll.decoded_samples,
                snap.rows_total,
            );
            // counters can reset when a log starts; clamp at zero
            let rate = |a: u64, b: u64| a.saturating_sub(b) as f64 / dt;
            let live = LiveStats {
                requests_per_s: rate(now.0, prev.0),
                samples_per_s: rate(now.1, prev.1),
                rows_per_s: rate(now.2, prev.2),
                ..snap
            };
            prev = now;
            shared.sink.emit(EngineEvent::LoggerStats(live));
        }
        if stopping {
            if let Some((_, f)) = lock(&shared.raw_file).as_mut() {
                let _ = f.flush();
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;

    #[derive(Default)]
    struct Collect(Mutex<Vec<EngineEvent>>);
    impl EventSink for Collect {
        fn emit(&self, event: EngineEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn simulated_session_logs_csv() {
        let sink = Arc::new(Collect::default());
        let mut engine = Engine::new(sink.clone());
        let ids: Vec<String> = presets::find("power_run")
            .unwrap()
            .channels
            .iter()
            .map(|s| s.to_string())
            .collect();
        engine.set_channels(ids).unwrap();
        engine
            .connect(ConnectOptions {
                simulate: true,
                regen_ms: 0,
                ..Default::default()
            })
            .unwrap();
        // wait for Ready
        let t0 = Instant::now();
        while engine.status().state != ConnState::Ready {
            assert!(
                t0.elapsed() < Duration::from_secs(3),
                "never became ready: {:?}",
                engine.status()
            );
            thread::sleep(Duration::from_millis(20));
        }
        thread::sleep(Duration::from_millis(300)); // replan + first cycles
        let dir = std::env::temp_dir().join(format!("logger43-engine-{}", std::process::id()));
        let st = engine
            .start_logging(LogRequest {
                base_dir: dir.clone(),
                run_name: "sim".into(),
                notes: String::new(),
                vehicle: "BMW E46 330i".into(),
                port: String::new(),
                preset: Some("power_run".into()),
            })
            .unwrap();
        assert_eq!(engine.status().state, ConnState::Logging);
        thread::sleep(Duration::from_millis(1200));
        let meta = engine.stop_logging().unwrap();
        assert!(meta.rows >= 5, "rows {}", meta.rows);
        assert!(meta.simulated);
        let csv = std::fs::read_to_string(st.csv_path).unwrap();
        let header = csv.lines().next().unwrap();
        assert!(header.starts_with("timestamp_ms,rpm,"));
        // knock retard is RAM-only and the simulator ident is not a known MS43 software
        assert!(!header.contains("knock_retard_deg"));
        engine.disconnect();

        let events = sink.0.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, EngineEvent::SampleBatch(_))));
        assert!(events.iter().any(
            |e| matches!(e, EngineEvent::ProtocolError(p) if p.message.contains("knock_retard_deg"))
        ));
        let states: Vec<ConnState> = events
            .iter()
            .filter_map(|e| match e {
                EngineEvent::ConnectionStatus(s) => Some(s.state),
                _ => None,
            })
            .collect();
        for s in [
            ConnState::OpeningSerial,
            ConnState::InitialisingKLine,
            ConnState::StartingDiagnosticSession,
            ConnState::ReadingEcuId,
            ConnState::Ready,
            ConnState::Logging,
        ] {
            assert!(states.contains(&s), "missing state {s:?} in {states:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
