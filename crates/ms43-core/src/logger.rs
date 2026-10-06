//! CSV run logger with a `run.json` metadata sidecar.
//!
//! The poller hands rows over a bounded channel (`try_send`, never blocks). A dedicated writer
//! thread owns the file. If the writer falls behind and the channel is full, the row is counted
//! as dropped rather than stalling ECU polling.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use serde::Serialize;

use crate::channels::ChannelDef;
use crate::ms43::EcuIdent;
use crate::stats::StatsSnapshot;

const QUEUE_DEPTH: usize = 4096;

/// One poller cycle: timestamp (seconds since connect) and one value per column.
#[derive(Debug, Clone)]
pub struct Row {
    pub t: f64,
    pub values: Vec<Option<f64>>,
    pub columns: Arc<Vec<&'static ChannelDef>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogRequest {
    pub base_dir: PathBuf,
    pub run_name: String,
    pub notes: String,
    pub vehicle: String,
    pub port: String,
    pub preset: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoggerStatus {
    pub run_name: String,
    pub run_dir: PathBuf,
    pub csv_path: PathBuf,
    pub started_at: DateTime<Local>,
    pub elapsed_s: f64,
    pub rows_written: u64,
    pub rows_dropped: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelMeta {
    pub id: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub verification: crate::channels::Verification,
    pub source: String,
    pub formula: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunMeta {
    pub app: &'static str,
    pub app_version: &'static str,
    pub vehicle: String,
    pub ecu: &'static str,
    pub ecu_version: Option<String>,
    pub ecu_ident: Option<EcuIdent>,
    pub simulated: bool,
    pub date: DateTime<Local>,
    pub ended: Option<DateTime<Local>>,
    pub end_reason: Option<String>,
    pub port: String,
    pub preset: Option<String>,
    pub run_name: String,
    pub notes: String,
    pub channels: Vec<ChannelMeta>,
    pub csv_file: String,
    pub timestamp_column: &'static str,
    pub rows: u64,
    pub rows_dropped: u64,
    pub duration_s: f64,
    pub average_sample_rate: f64,
    pub polling: Option<StatsSnapshot>,
    pub settings: serde_json::Value,
    pub notes_on_data: &'static str,
}

struct Counters {
    rows: AtomicU64,
    dropped: AtomicU64,
    bytes: AtomicU64,
}

pub struct Logger {
    tx: Option<SyncSender<Row>>,
    writer: Option<JoinHandle<io::Result<()>>>,
    counters: Arc<Counters>,
    started: Instant,
    /// Session time (Row::t) at which the log started; CSV timestamps are relative to it.
    pub t0: f64,
    meta: RunMeta,
    run_dir: PathBuf,
    csv_path: PathBuf,
}

impl Logger {
    pub fn start(
        req: &LogRequest,
        columns: &[&'static ChannelDef],
        t0: f64,
        ident: Option<EcuIdent>,
        simulated: bool,
        settings: serde_json::Value,
    ) -> io::Result<Self> {
        if columns.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no channels selected",
            ));
        }
        let now = Local::now();
        let name = sanitize(&req.run_name);
        let dir_name = if name.is_empty() {
            now.format("%Y-%m-%d_%H%M%S").to_string()
        } else {
            format!("{}_{name}", now.format("%Y-%m-%d_%H%M%S"))
        };
        let run_dir = req.base_dir.join(dir_name);
        fs::create_dir_all(&run_dir)?;
        let csv_path = run_dir.join("run.csv");
        let mut out = BufWriter::new(File::create(&csv_path)?);

        let header: Vec<&str> = std::iter::once("timestamp_ms")
            .chain(columns.iter().map(|c| c.column))
            .collect();
        let header = header.join(",") + "\n";
        out.write_all(header.as_bytes())?;

        let counters = Arc::new(Counters {
            rows: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            bytes: AtomicU64::new(header.len() as u64),
        });
        let (tx, rx) = mpsc::sync_channel::<Row>(QUEUE_DEPTH);
        let c = counters.clone();
        let writer =
            thread::Builder::new()
                .name("csv-writer".into())
                .spawn(move || -> io::Result<()> {
                    let mut last_flush = Instant::now();
                    let mut line = String::with_capacity(512);
                    loop {
                        match rx.recv_timeout(Duration::from_millis(500)) {
                            Ok(row) => {
                                line.clear();
                                line.push_str(&fmt_num((row.t - t0) * 1000.0, 3));
                                for v in &row.values {
                                    line.push(',');
                                    if let Some(v) = v {
                                        line.push_str(&fmt_num(*v, 4));
                                    }
                                }
                                line.push('\n');
                                out.write_all(line.as_bytes())?;
                                c.rows.fetch_add(1, Ordering::Relaxed);
                                c.bytes.fetch_add(line.len() as u64, Ordering::Relaxed);
                            }
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                        if last_flush.elapsed() > Duration::from_secs(1) {
                            out.flush()?;
                            last_flush = Instant::now();
                        }
                    }
                    out.flush()
                })?;

        let meta = RunMeta {
            app: "Logger43",
            app_version: env!("CARGO_PKG_VERSION"),
            vehicle: req.vehicle.clone(),
            ecu: "MS43",
            ecu_version: ident.as_ref().and_then(|i| i.bmw_part_number.clone()),
            ecu_ident: ident,
            simulated,
            date: now,
            ended: None,
            end_reason: None,
            port: req.port.clone(),
            preset: req.preset.clone(),
            run_name: req.run_name.clone(),
            notes: req.notes.clone(),
            channels: columns
                .iter()
                .map(|c| ChannelMeta {
                    id: c.id,
                    label: c.label,
                    unit: c.unit,
                    verification: c.verification,
                    source: c.source_text(),
                    formula: format!("{} * raw + {}", c.factor, c.offset),
                })
                .collect(),
            csv_file: "run.csv".into(),
            timestamp_column: "timestamp_ms = milliseconds since log start (monotonic clock)",
            rows: 0,
            rows_dropped: 0,
            duration_s: 0.0,
            average_sample_rate: 0.0,
            polling: None,
            settings,
            notes_on_data: "One row per poller cycle, timestamped at the first response of the cycle. \
                            Values of slower jobs not due in a cycle are carried forward; values of a \
                            due job that failed are left empty.",
        };
        let logger = Self {
            tx: Some(tx),
            writer: Some(writer),
            counters,
            started: Instant::now(),
            t0,
            meta,
            run_dir,
            csv_path,
        };
        logger.write_meta()?; // sidecar exists from the start, completed on stop
        Ok(logger)
    }

    /// Non-blocking. Returns false if the row was dropped.
    pub fn push(&self, row: Row) -> bool {
        let Some(tx) = &self.tx else { return false };
        match tx.try_send(row) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn status(&self) -> LoggerStatus {
        LoggerStatus {
            run_name: self.meta.run_name.clone(),
            run_dir: self.run_dir.clone(),
            csv_path: self.csv_path.clone(),
            started_at: self.meta.date,
            elapsed_s: self.started.elapsed().as_secs_f64(),
            rows_written: self.counters.rows.load(Ordering::Relaxed),
            rows_dropped: self.counters.dropped.load(Ordering::Relaxed),
            bytes_written: self.counters.bytes.load(Ordering::Relaxed),
        }
    }

    /// Closes the CSV, waits for the writer, completes run.json.
    pub fn stop(mut self, polling: StatsSnapshot, reason: &str) -> io::Result<RunMeta> {
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            w.join()
                .map_err(|_| io::Error::other("CSV writer thread panicked"))??;
        }
        let duration = self.started.elapsed().as_secs_f64();
        let rows = self.counters.rows.load(Ordering::Relaxed);
        self.meta.ended = Some(Local::now());
        self.meta.end_reason = Some(reason.to_string());
        self.meta.rows = rows;
        self.meta.rows_dropped = self.counters.dropped.load(Ordering::Relaxed);
        self.meta.duration_s = duration;
        self.meta.average_sample_rate = if duration > 0.0 {
            rows as f64 / duration
        } else {
            0.0
        };
        self.meta.polling = Some(polling);
        self.write_meta()?;
        Ok(self.meta.clone())
    }

    fn write_meta(&self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.meta).map_err(io::Error::other)?;
        fs::write(self.run_dir.join("run.json"), json)
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
    }
}

fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect()
}

/// Fixed decimals with trailing zeros trimmed: 707, 0.1025, -105.375.
fn fmt_num(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".into()
        } else {
            s.into()
        }
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels;

    #[test]
    fn writes_csv_and_sidecar() {
        let dir = std::env::temp_dir().join(format!("logger43-test-{}", std::process::id()));
        let cols: Vec<&'static ChannelDef> = vec![
            channels::find("rpm").unwrap(),
            channels::find("coolant_c").unwrap(),
        ];
        let arc = Arc::new(cols.clone());
        let req = LogRequest {
            base_dir: dir.clone(),
            run_name: "3rd gear WOT".into(),
            notes: "test".into(),
            vehicle: "BMW E46 330i".into(),
            port: "SIM".into(),
            preset: Some("power_run".into()),
        };
        let lg = Logger::start(&req, &cols, 10.0, None, true, serde_json::json!({})).unwrap();
        assert!(lg.push(Row {
            t: 10.0,
            values: vec![Some(707.0), Some(92.25)],
            columns: arc.clone()
        }));
        assert!(lg.push(Row {
            t: 10.0712,
            values: vec![Some(1500.0), None],
            columns: arc
        }));
        let run_dir = lg.run_dir().to_path_buf();
        let meta = lg.stop(StatsSnapshot::default(), "user stop").unwrap();
        assert_eq!(meta.rows, 2);
        let csv = fs::read_to_string(run_dir.join("run.csv")).unwrap();
        assert_eq!(csv, "timestamp_ms,rpm,coolant_c\n0,707,92.25\n71.2,1500,\n");
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
        assert_eq!(json["ecu"], "MS43");
        assert_eq!(json["channels"][0]["id"], "rpm");
        assert!(run_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("_3rd_gear_WOT"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn number_format() {
        assert_eq!(fmt_num(707.0, 4), "707");
        assert_eq!(fmt_num(-105.375, 4), "-105.375");
        assert_eq!(fmt_num(0.10253, 4), "0.1025");
        assert_eq!(fmt_num(-0.00001, 4), "0");
    }
}
