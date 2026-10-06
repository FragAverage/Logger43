use std::path::PathBuf;

use chrono::Local;
use ms43_core::calibration::{
    self, BinInfo, Calibration, CalibrationSummary, TableData, TableInfo,
};
use ms43_core::channels::{self, ChannelInfo};
use ms43_core::engine::{ConnectOptions, ConnectionStatus, EcuInfo, LiveStats, SampleBatch};
use ms43_core::logger::{LogRequest, LoggerStatus, RunMeta};
use ms43_core::plan::PlanSummary;
use ms43_core::presets::{self, Preset};
use ms43_core::serial::{self, PortInfo};
use serde::Deserialize;
use tauri::{AppHandle, Manager, State};

use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn engine<'a>(
    state: &'a State<'_, AppState>,
) -> std::sync::MutexGuard<'a, ms43_core::engine::Engine> {
    state.engine.lock().unwrap_or_else(|e| e.into_inner())
}

fn base_dir(app: &AppHandle) -> CmdResult<PathBuf> {
    let docs = app
        .path()
        .document_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| e.to_string())?;
    Ok(docs.join("Logger43"))
}

#[tauri::command]
pub fn list_serial_ports() -> CmdResult<Vec<PortInfo>> {
    serial::list_ports().map_err(|e| e.to_string())
}

/// Channel catalogue with verification levels, straight from `ms43_core::channels`.
#[tauri::command]
pub fn get_channels() -> Vec<ChannelInfo> {
    channels::CHANNELS.iter().map(|c| c.info()).collect()
}

#[tauri::command]
pub fn get_presets() -> Vec<Preset> {
    presets::PRESETS.to_vec()
}

#[tauri::command]
pub fn connect_ecu(state: State<'_, AppState>, options: ConnectOptions) -> CmdResult<()> {
    engine(&state).connect(options)
}

#[tauri::command]
pub fn disconnect_ecu(state: State<'_, AppState>) {
    engine(&state).disconnect();
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> ConnectionStatus {
    engine(&state).status()
}

#[tauri::command]
pub fn get_ecu_info(state: State<'_, AppState>) -> Option<EcuInfo> {
    engine(&state).ecu_info()
}

#[tauri::command]
pub fn set_selected_channels(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> CmdResult<PlanSummary> {
    engine(&state).set_channels(ids)
}

#[tauri::command]
pub fn get_selected_channels(state: State<'_, AppState>) -> Vec<String> {
    engine(&state).selected().to_vec()
}

#[derive(Deserialize)]
pub struct StartLogArgs {
    pub run_name: String,
    pub notes: String,
    pub preset: Option<String>,
    pub base_dir: Option<String>,
}

#[tauri::command]
pub fn start_logging(
    app: AppHandle,
    state: State<'_, AppState>,
    args: StartLogArgs,
) -> CmdResult<LoggerStatus> {
    let base = match args.base_dir.filter(|d| !d.trim().is_empty()) {
        Some(d) => PathBuf::from(d),
        None => base_dir(&app)?.join("runs"),
    };
    engine(&state).start_logging(LogRequest {
        base_dir: base,
        run_name: args.run_name,
        notes: args.notes,
        vehicle: "BMW E46 330i".into(),
        port: String::new(),
        preset: args.preset,
    })
}

#[tauri::command]
pub fn stop_logging(state: State<'_, AppState>) -> CmdResult<RunMeta> {
    engine(&state).stop_logging()
}

#[tauri::command]
pub fn get_logger_stats(state: State<'_, AppState>) -> Option<LiveStats> {
    engine(&state).live_stats()
}

#[tauri::command]
pub fn get_recent_samples(state: State<'_, AppState>, seconds: f64) -> Option<SampleBatch> {
    engine(&state).recent(seconds)
}

#[tauri::command]
pub fn set_debug(state: State<'_, AppState>, enabled: bool) {
    engine(&state).set_debug(enabled);
}

#[tauri::command]
pub fn start_raw_recording(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
    let path = base_dir(&app)?.join("debug").join(format!(
        "raw_{}.log",
        Local::now().format("%Y-%m-%d_%H%M%S")
    ));
    engine(&state)
        .record_raw(Some(path))?
        .map(|p| p.display().to_string())
        .ok_or_else(|| "could not start recording".into())
}

#[tauri::command]
pub fn stop_raw_recording(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    Ok(engine(&state)
        .record_raw(None)?
        .map(|p| p.display().to_string()))
}

#[tauri::command]
pub fn save_debug_history(app: AppHandle, state: State<'_, AppState>) -> CmdResult<String> {
    let path = base_dir(&app)?.join("debug").join(format!(
        "history_{}.log",
        Local::now().format("%Y-%m-%d_%H%M%S")
    ));
    engine(&state).save_debug_history(&path)?;
    Ok(path.display().to_string())
}

#[tauri::command]
pub fn get_default_log_dir(app: AppHandle) -> CmdResult<String> {
    Ok(base_dir(&app)?.join("runs").display().to_string())
}

// ---------------------------------------------------------------- calibration (read-only files)

fn calibration<'a>(
    state: &'a State<'_, AppState>,
) -> std::sync::MutexGuard<'a, Option<Calibration>> {
    state.calibration.lock().unwrap_or_else(|e| e.into_inner())
}

#[tauri::command]
pub fn inspect_bin(path: String) -> CmdResult<BinInfo> {
    calibration::inspect_bin(std::path::Path::new(&path))
}

/// Loads a bin + XDF pair. Refuses mismatched software versions.
#[tauri::command]
pub fn load_calibration(
    state: State<'_, AppState>,
    bin_path: String,
    xdf_path: String,
) -> CmdResult<CalibrationSummary> {
    let cal = Calibration::load(
        std::path::Path::new(&bin_path),
        std::path::Path::new(&xdf_path),
    )?;
    let summary = cal.summary();
    *calibration(&state) = Some(cal);
    Ok(summary)
}

#[tauri::command]
pub fn unload_calibration(state: State<'_, AppState>) {
    *calibration(&state) = None;
}

#[tauri::command]
pub fn get_calibration(state: State<'_, AppState>) -> Option<CalibrationSummary> {
    calibration(&state).as_ref().map(|c| c.summary())
}

#[tauri::command]
pub fn list_tables(state: State<'_, AppState>) -> CmdResult<Vec<TableInfo>> {
    Ok(calibration(&state)
        .as_ref()
        .ok_or("no calibration loaded")?
        .list())
}

#[tauri::command]
pub fn get_table(state: State<'_, AppState>, uid: u32) -> CmdResult<TableData> {
    calibration(&state)
        .as_ref()
        .ok_or("no calibration loaded")?
        .table(uid)
}
