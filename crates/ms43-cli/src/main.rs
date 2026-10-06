//! Milestone 1 proof of concept.
//!
//! `ms43 poc --port COM4` runs the whole path: list ports → open → initialise DS2 parameters →
//! IDENT → `0B 03` → decode RPM → raw TX/echo/RX frames → latency and rate statistics.
//! Read-only: only allow-listed DS2 services can be sent (see docs/PROTOCOL.md §9).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use ms43_core::channels::{self, ChannelDef, Source};
use ms43_core::ds2;
use ms43_core::engine::{ConnState, ConnectOptions, Engine, EngineEvent, EventSink};
use ms43_core::logger::LogRequest;
use ms43_core::ms43::{EcuIdent, LayoutTrust, KNOWN_MS43_PART_NUMBERS};
use ms43_core::presets;
use ms43_core::safety::{Request, StatusBlock};
use ms43_core::serial::{self, SerialConfig, SerialLink};
use ms43_core::sim::{SimFaults, SimLink};
use ms43_core::stats::Stats;
use ms43_core::transport::{EchoMode, Link, TraceDir, TraceEvent, Transport, TransportConfig};

#[derive(Parser)]
#[command(
    name = "ms43",
    version,
    about = "Read-only BMW MS43 (DS2/K-line) proof of concept"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    #[command(flatten)]
    opts: Opts,
}

#[derive(clap::Args)]
struct Opts {
    /// Serial port of the K+DCAN cable (COM4, /dev/cu.usbserial-XXXX)
    #[arg(long, global = true)]
    port: Option<String>,
    /// Use the built-in simulator instead of a cable (replays a real MS43 capture)
    #[arg(long, global = true)]
    simulate: bool,
    /// Idle time after each response before the next request (BMW SGBD: 100)
    #[arg(long, global = true, default_value_t = 100)]
    regen_ms: u64,
    /// Wait for first response byte (BMW SGBD: 2000)
    #[arg(long, global = true, default_value_t = 2000)]
    timeout_ms: u64,
    /// Max gap inside a telegram (BMW SGBD: 20)
    #[arg(long, global = true, default_value_t = 20)]
    inter_byte_ms: u64,
    /// Allowance for USB-serial buffering (FTDI latency timer, MS4X: 16)
    #[arg(long, global = true, default_value_t = 16)]
    usb_latency_ms: u64,
    /// Interface does not echo K-line bytes (K+DCAN cables do echo)
    #[arg(long, global = true)]
    no_echo: bool,
    /// Assert DTR after opening (EdiabasLib: off)
    #[arg(long, global = true)]
    dtr: bool,
    /// Assert RTS after opening (EdiabasLib: off)
    #[arg(long, global = true)]
    rts: bool,
    /// Do not print raw frames to the console
    #[arg(long, global = true)]
    quiet: bool,
    /// Write a raw session log (ISO timestamps, TX/EC/RX/XX/ER lines)
    #[arg(long, global = true)]
    log: Option<PathBuf>,
    /// Simulator fault injection: corrupt every Nth checksum
    #[arg(long, global = true, hide = true)]
    sim_bad_checksum_every: Option<u32>,
    /// Simulator fault injection: drop every Nth response
    #[arg(long, global = true, hide = true)]
    sim_drop_every: Option<u32>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List serial ports (K+DCAN cables are usually FTDI, VID 0403)
    Ports,
    /// Read DME identification
    Ident {
        #[arg(long, default_value_t = 1)]
        repeat: u32,
    },
    /// Poll a status block and decode channels
    Poll {
        /// Number of requests (0 = until Ctrl-C)
        #[arg(long, default_value_t = 20)]
        count: u64,
        #[arg(long, value_enum, default_value_t = BlockArg::Measurements)]
        block: BlockArg,
        /// Decode every channel in the block, not just RPM
        #[arg(long)]
        all: bool,
    },
    /// Headless logging to CSV + run.json using a preset (same engine as the GUI)
    Log {
        #[arg(long, default_value = "power_run")]
        preset: String,
        /// Seconds to log (0 = until Ctrl-C)
        #[arg(long, default_value_t = 30)]
        seconds: u64,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value = "")]
        notes: String,
        /// Output directory for run folders
        #[arg(long, default_value = "runs")]
        dir: PathBuf,
    },
    /// Print the channel catalogue and presets as JSON (used for the browser dev mock)
    #[command(hide = true)]
    Catalog,
    /// Full milestone-1 sequence: ports, connect, ident, RPM, stats
    Poc {
        #[arg(long, default_value_t = 20)]
        count: u64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum BlockArg {
    Measurements,
    Vanos,
}

impl BlockArg {
    fn block(self) -> StatusBlock {
        match self {
            BlockArg::Measurements => StatusBlock::Measurements,
            BlockArg::Vanos => StatusBlock::Vanos,
        }
    }
}

type Shared<T> = Arc<Mutex<T>>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Ports => list_ports().map(|_| ()),
        Cmd::Ident { repeat } => with_session(&cli.opts, |s| s.ident(repeat).map(|_| ())),
        Cmd::Poll { count, block, all } => with_session(&cli.opts, |s| {
            let ident = s.ident(1)?;
            s.gate(&ident)?;
            s.poll(block.block(), count, all)
        }),
        Cmd::Poc { count } => poc(&cli.opts, count),
        Cmd::Catalog => {
            let channels: Vec<_> = channels::CHANNELS.iter().map(|c| c.info()).collect();
            println!(
                "{}",
                serde_json::json!({ "channels": channels, "presets": presets::PRESETS })
            );
            Ok(())
        }
        Cmd::Log {
            preset,
            seconds,
            name,
            notes,
            dir,
        } => headless_log(&cli.opts, &preset, seconds, name, notes, dir),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nERROR: {e}");
            ExitCode::FAILURE
        }
    }
}

fn poc(opts: &Opts, count: u64) -> Result<(), String> {
    step(1, "List serial ports");
    let ports = list_ports()?;
    if !opts.simulate && opts.port.is_none() {
        let hint = ports
            .iter()
            .find(|p| p.likely_kdcan)
            .map(|p| p.name.clone());
        return Err(match hint {
            Some(p) => format!("no --port given. Likely K+DCAN cable: {p}  →  ms43 poc --port {p}"),
            None => "no --port given and no FTDI (K+DCAN) port found. Use --simulate to try without a cable.".into(),
        });
    }
    with_session(opts, |s| {
        step(4, "Read ECU identification (DS2 12 04 00 16)");
        let ident = s.ident(1)?;
        s.gate(&ident)?;
        step(
            5,
            "Live data: DS2 status block 0B 03 (12 05 0B 03 1F) → decode RPM, latency, rate",
        );
        s.poll(StatusBlock::Measurements, count, false)
    })
}

fn step(n: u32, what: &str) {
    println!("\n== Step {n}: {what}");
}

fn list_ports() -> Result<Vec<serial::PortInfo>, String> {
    let ports = serial::list_ports().map_err(|e| format!("cannot enumerate serial ports: {e}"))?;
    if ports.is_empty() {
        println!("  (no serial ports found)");
    }
    for p in &ports {
        let usb = match (p.vid, p.pid) {
            (Some(v), Some(pi)) => format!("USB {v:04X}:{pi:04X}"),
            _ => p.kind.clone(),
        };
        println!(
            "  {:<28} {:<14} {} {}{}",
            p.name,
            usb,
            p.manufacturer.as_deref().unwrap_or(""),
            p.product.as_deref().unwrap_or(""),
            if p.likely_kdcan {
                "   <- FTDI, likely K+DCAN"
            } else {
                ""
            }
        );
    }
    Ok(ports)
}

struct Session {
    t: Transport<Box<dyn Link>>,
    trace_file: Option<Shared<BufWriter<File>>>,
}

fn with_session(
    opts: &Opts,
    f: impl FnOnce(&mut Session) -> Result<(), String>,
) -> Result<(), String> {
    step(2, "Open serial port (9600 baud, 8E1, DTR/RTS per options)");
    let link: Box<dyn Link> = if opts.simulate {
        println!(
            "  *** SIMULATION — no hardware, synthetic RPM sweep over a real MS43 capture ***"
        );
        Box::new(SimLink::new(SimFaults {
            bad_checksum_every: opts.sim_bad_checksum_every,
            no_response_every: opts.sim_drop_every,
            ..Default::default()
        }))
    } else {
        let port = opts
            .port
            .as_deref()
            .ok_or("--port is required (see `ms43 ports`)")?;
        let cfg = SerialConfig {
            dtr: opts.dtr,
            rts: opts.rts,
            ..Default::default()
        };
        Box::new(SerialLink::open(port, &cfg).map_err(|e| format!("cannot open {port}: {e}"))?)
    };
    println!("  opened {}", link.description());

    step(
        3,
        "Initialise DS2 link parameters (no wake-up telegram exists for MS43 DS2)",
    );
    let cfg = TransportConfig {
        response_timeout: Duration::from_millis(opts.timeout_ms),
        inter_byte_timeout: Duration::from_millis(opts.inter_byte_ms),
        usb_latency: Duration::from_millis(opts.usb_latency_ms),
        regen_time: Duration::from_millis(opts.regen_ms),
        echo: if opts.no_echo {
            EchoMode::None
        } else {
            EchoMode::Expect
        },
        ..Default::default()
    };
    println!(
        "  address 0x12, response timeout {} ms, inter-byte {}+{} ms, regen {} ms, echo {:?}, retries {}",
        opts.timeout_ms, opts.inter_byte_ms, opts.usb_latency_ms, opts.regen_ms, cfg.echo, cfg.retries
    );
    let mut t = Transport::new(link, cfg);

    let trace_file = match &opts.log {
        Some(p) => {
            let f =
                File::create(p).map_err(|e| format!("cannot create log {}: {e}", p.display()))?;
            println!("  raw session log → {}", p.display());
            Some(Arc::new(Mutex::new(BufWriter::new(f))))
        }
        None => None,
    };
    let console = !opts.quiet;
    let file = trace_file.clone();
    t.set_trace(Some(Box::new(move |ev: &TraceEvent| {
        if console {
            print_trace(ev);
        }
        if let Some(f) = &file {
            let _ = writeln!(f.lock().unwrap(), "{}", ev.to_line());
        }
    })));

    let mut session = Session { t, trace_file };
    let r = f(&mut session);
    if let Some(f) = &session.trace_file {
        let _ = f.lock().unwrap().flush();
    }
    r
}

fn print_trace(ev: &TraceEvent) {
    let time = ev.time.format("%H:%M:%S%.3f");
    match ev.dir {
        TraceDir::Error => println!("   {time}  !! {}", ev.note.as_deref().unwrap_or("")),
        _ => {
            let note = match (ev.dir, &ev.note) {
                (TraceDir::Rx, Some(n))
                | (TraceDir::Discarded, Some(n))
                | (TraceDir::Tx, Some(n)) => {
                    format!("  ({n})")
                }
                _ => String::new(),
            };
            println!("{} {time}  [{}]{note}", ev.dir.tag(), ds2::hex(&ev.bytes));
        }
    }
}

impl Session {
    fn ident(&mut self, repeat: u32) -> Result<EcuIdent, String> {
        let mut last = None;
        for i in 0..repeat.max(1) {
            if repeat > 1 {
                println!("\n-- ident {}/{}", i + 1, repeat);
            }
            let ex = self
                .t
                .transact(&Request::ident())
                .map_err(|e| e.to_string())?;
            let id = EcuIdent::parse(&ex.response);
            println!(
                "RTT: {:.1} ms (attempts {})",
                ex.rtt.as_secs_f64() * 1e3,
                ex.attempts
            );
            print_ident(&id);
            last = Some(id);
        }
        Ok(last.unwrap())
    }

    /// Refuses MS43 layouts for DMEs known to be something else; warns for unknown part numbers.
    fn gate(&self, id: &EcuIdent) -> Result<(), String> {
        match id.layout_trust() {
            LayoutTrust::KnownMs43 => {
                println!("  part number is a known MS43: MS430DS0 block layouts apply");
                Ok(())
            }
            LayoutTrust::OtherDme => Err(format!(
                "DME part number {:?} is a known non-MS43 Siemens DME; its 0B 03 layout differs. Refusing to decode.",
                id.bmw_part_number
            )),
            LayoutTrust::Unknown => {
                let known: Vec<_> = KNOWN_MS43_PART_NUMBERS.iter().map(|(p, _)| *p).collect();
                println!(
                    "  WARNING: part number {:?} is not on the known MS43 list {:?}. Decoding with the MS43 \
                     layout anyway; please report this part number so it can be added.",
                    id.bmw_part_number.as_deref().unwrap_or("?"),
                    known
                );
                Ok(())
            }
        }
    }

    fn poll(&mut self, block: StatusBlock, count: u64, all: bool) -> Result<(), String> {
        let defs: Vec<&ChannelDef> = channels::CHANNELS
            .iter()
            .filter(|c| c.block() == Some(block))
            .filter(|c| all || c.id == "rpm" || (block == StatusBlock::Vanos))
            .collect();
        let req = Request::status_block(block);
        let stop = Arc::new(AtomicBool::new(false));
        {
            let stop = stop.clone();
            let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));
        }
        let mut stats = Stats::new();
        let mut n = 0u64;
        while (count == 0 || n < count) && !stop.load(Ordering::SeqCst) {
            n += 1;
            match self.t.transact_once(&req) {
                Ok(ex) => {
                    println!(
                        "RTT: {:.1} ms  (ECU+USB delay after echo {:.1} ms)",
                        ex.rtt.as_secs_f64() * 1e3,
                        ex.ecu_delay.as_secs_f64() * 1e3
                    );
                    let mut decoded = 0;
                    for d in &defs {
                        if let Some(v) = d.decode_block(&ex.response) {
                            decoded += 1;
                            let Source::Block { offset, dtype, .. } = d.source else {
                                continue;
                            };
                            let raw_bytes = &ex.response.bytes()[offset..offset + dtype.size()];
                            println!(
                                "Decoded: {:<26} = {:>9.3} {:<9} raw=0x{} ({}) [{:?}]",
                                d.label,
                                v.value,
                                d.unit,
                                raw_bytes
                                    .iter()
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<String>(),
                                v.raw,
                                d.verification
                            );
                        }
                    }
                    stats.record_ok(&ex, decoded);
                }
                Err(e) => {
                    stats.record_err(&e);
                    if e.is_fatal() {
                        print_stats(&stats);
                        return Err(e.to_string());
                    }
                }
            }
        }
        print_stats(&stats);
        Ok(())
    }
}

fn print_ident(id: &EcuIdent) {
    let f = |o: &Option<String>| o.clone().unwrap_or_else(|| "-".into());
    println!(
        "  BMW part number : {}  {}",
        f(&id.bmw_part_number),
        id.description().unwrap_or("")
    );
    println!(
        "  HW / SW / AI    : {} / {} / {}",
        f(&id.hardware_number),
        f(&id.software_number),
        f(&id.change_index)
    );
    println!(
        "  coding/diag/bus : {} / {} / {}",
        f(&id.coding_index),
        f(&id.diag_index),
        f(&id.bus_index)
    );
    println!(
        "  built week/year : {} / {}",
        f(&id.production_week),
        f(&id.production_year)
    );
    println!("  raw ASCII       : {}", id.raw_ascii);
}

fn print_stats(stats: &Stats) {
    let s = stats.snapshot();
    let ms = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.1}"));
    println!("\n== Statistics");
    println!(
        "  requests        : {} ({} ok, {} failed)",
        s.requests, s.ok, s.failed
    );
    println!(
        "  failures        : {} timeouts, {} checksum, {} negative, {} other",
        s.timeouts, s.checksum_errors, s.negative_responses, s.other_errors
    );
    println!(
        "  RTT ms          : min {} / avg {} / max {}",
        ms(s.rtt_min_ms),
        ms(s.rtt_avg_ms),
        ms(s.rtt_max_ms)
    );
    println!(
        "  request rate    : {:.2} req/s over {:.1} s",
        s.requests_per_s, s.elapsed_s
    );
    println!(
        "  sample rate     : {:.2} decoded values/s",
        s.samples_per_s
    );
    println!("  discarded bytes : {}", s.discarded_bytes);
}

struct PrintSink;

impl EventSink for PrintSink {
    fn emit(&self, event: EngineEvent) {
        match event {
            EngineEvent::ConnectionStatus(s) => {
                println!(
                    "[state] {:?}{}",
                    s.state,
                    s.message.map(|m| format!(" — {m}")).unwrap_or_default()
                )
            }
            EngineEvent::EcuInfo(e) => println!(
                "[ecu]   part {} ({:?})",
                e.ident.bmw_part_number.as_deref().unwrap_or("?"),
                e.layout_trust
            ),
            EngineEvent::ProtocolError(e) => println!("[error] {}", e.message),
            EngineEvent::LoggerStats(s) => {
                if let Some(l) = &s.logger {
                    println!(
                        "[log]   {:>6.1} s  rows {:>6}  {:>5.1} rows/s  {:>5.1} req/s  avg RTT {} ms  failed {}  dropped {}",
                        l.elapsed_s,
                        l.rows_written,
                        s.rows_per_s,
                        s.requests_per_s,
                        s.poll.rtt_avg_ms.map_or("-".into(), |v| format!("{v:.1}")),
                        s.poll.failed,
                        s.dropped_samples + l.rows_dropped
                    );
                }
            }
            EngineEvent::SampleBatch(_) | EngineEvent::ProtocolDebug(_) => {}
        }
    }
}

fn headless_log(
    opts: &Opts,
    preset: &str,
    seconds: u64,
    name: String,
    notes: String,
    dir: PathBuf,
) -> Result<(), String> {
    let preset = presets::find(preset).ok_or_else(|| {
        let ids: Vec<_> = presets::PRESETS.iter().map(|p| p.id).collect();
        format!("unknown preset {preset}; available: {ids:?}")
    })?;
    let mut engine = Engine::new(Arc::new(PrintSink));
    let plan = engine.set_channels(preset.channels.iter().map(|s| s.to_string()).collect())?;
    println!("preset {} → {} channels", preset.name, plan.columns.len());
    engine.connect(ConnectOptions {
        port: opts.port.clone().unwrap_or_default(),
        simulate: opts.simulate,
        regen_ms: opts.regen_ms,
        timeout_ms: opts.timeout_ms,
        inter_byte_ms: opts.inter_byte_ms,
        usb_latency_ms: opts.usb_latency_ms,
        echo: !opts.no_echo,
        dtr: opts.dtr,
        rts: opts.rts,
    })?;
    let started = std::time::Instant::now();
    loop {
        match engine.status().state {
            ConnState::Ready => break,
            ConnState::Error | ConnState::Disconnected
                if started.elapsed() > Duration::from_millis(200) =>
            {
                return Err(engine
                    .status()
                    .message
                    .unwrap_or_else(|| "connection failed".into()));
            }
            _ if started.elapsed() > Duration::from_secs(15) => {
                return Err("connection timed out".into())
            }
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    std::thread::sleep(Duration::from_millis(300)); // first plan + cycles
    if let Some(s) = engine.live_stats().and_then(|s| s.plan) {
        for r in &s.requests {
            println!(
                "  request {:<24} [{}] every {} cycle(s): {}",
                r.request,
                r.frame,
                r.every_n_cycles,
                r.channels.join(", ")
            );
        }
    }
    let st = engine.start_logging(LogRequest {
        base_dir: dir,
        run_name: name,
        notes,
        vehicle: "BMW E46 330i".into(),
        port: String::new(),
        preset: Some(preset.id.into()),
    })?;
    println!("logging → {}", st.csv_path.display());
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));
    }
    let t = std::time::Instant::now();
    while !stop.load(Ordering::SeqCst)
        && (seconds == 0 || t.elapsed() < Duration::from_secs(seconds))
    {
        if engine.status().state == ConnState::Error && !engine.is_connected() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let meta = engine.stop_logging();
    engine.disconnect();
    let meta = meta?;
    println!(
        "done: {} rows in {:.1} s = {:.2} rows/s, {} dropped → {}",
        meta.rows,
        meta.duration_s,
        meta.average_sample_rate,
        meta.rows_dropped,
        st.run_dir.display()
    );
    Ok(())
}
