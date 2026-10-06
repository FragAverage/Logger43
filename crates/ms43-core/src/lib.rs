//! Read-only BMW DS2 / Siemens MS43 protocol stack.
//!
//! Layers: [`serial`] (ports) → [`transport`] (echo, timeouts, resync, tracing) → [`ds2`] (framing)
//! → [`safety`] (allow-listed requests) → [`ms43`] / [`channels`] (MS43 layouts and scaling).
//! Protocol facts and their sources are in `docs/PROTOCOL.md`.

pub mod calibration;
pub mod channels;
pub mod ds2;
pub mod engine;
pub mod logger;
pub mod ms43;
pub mod plan;
pub mod presets;
pub mod safety;
pub mod serial;
pub mod sim;
pub mod stats;
pub mod transport;
pub mod xdf;
