//! Tauri shell. Protocol, polling and logging live in `ms43-core`; this crate exposes commands
//! and forwards engine events to the webview. The poller runs on its own thread and never
//! waits for the UI.

mod commands;

use std::sync::{Arc, Mutex};

use ms43_core::engine::{ConnectOptions, Engine, EngineEvent, EventSink};
use tauri::{AppHandle, Emitter, Manager};

struct TauriSink(AppHandle);

impl EventSink for TauriSink {
    fn emit(&self, event: EngineEvent) {
        let name = event.name();
        let _ = match event {
            EngineEvent::ConnectionStatus(p) => self.0.emit(name, p),
            EngineEvent::EcuInfo(p) => self.0.emit(name, p),
            EngineEvent::SampleBatch(p) => self.0.emit(name, p),
            EngineEvent::LoggerStats(p) => self.0.emit(name, p),
            EngineEvent::ProtocolDebug(p) => self.0.emit(name, p),
            EngineEvent::ProtocolError(p) => self.0.emit(name, p),
        };
    }
}

pub struct AppState {
    pub engine: Mutex<Engine>,
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let sink = Arc::new(TauriSink(app.handle().clone()));
            let mut engine = Engine::new(sink);
            // Demo/development: LOGGER43_SIMULATE=1 connects to the simulator with Power Run on start.
            if std::env::var_os("LOGGER43_SIMULATE").is_some() {
                if let Some(p) = ms43_core::presets::find("power_run") {
                    let _ = engine.set_channels(p.channels.iter().map(|s| s.to_string()).collect());
                }
                let _ = engine.connect(ConnectOptions {
                    simulate: true,
                    regen_ms: 0,
                    ..Default::default()
                });
            }
            app.manage(AppState {
                engine: Mutex::new(engine),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_serial_ports,
            commands::get_channels,
            commands::get_presets,
            commands::connect_ecu,
            commands::disconnect_ecu,
            commands::get_status,
            commands::get_ecu_info,
            commands::set_selected_channels,
            commands::get_selected_channels,
            commands::start_logging,
            commands::stop_logging,
            commands::get_logger_stats,
            commands::get_recent_samples,
            commands::set_debug,
            commands::start_raw_recording,
            commands::stop_raw_recording,
            commands::save_debug_history,
            commands::get_default_log_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Logger43");
}
