//! Serial port discovery and the `serialport`-backed [`Link`].

use std::io::{self, Read, Write};
use std::time::Duration;

use serde::Serialize;
use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits,
};

use crate::transport::Link;

/// FTDI's USB vendor id. Most K+DCAN cables use an FT232R.
pub const FTDI_VID: u16 = 0x0403;

#[derive(Debug, Clone, Serialize)]
pub struct PortInfo {
    pub name: String,
    pub kind: String,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    /// FTDI USB serial — the usual K+DCAN chip. A hint, not proof.
    pub likely_kdcan: bool,
}

pub fn list_ports() -> io::Result<Vec<PortInfo>> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .map_err(io::Error::other)?
        .into_iter()
        // macOS exposes every device twice; the /dev/cu.* node is the one to open.
        .filter(|p| !p.port_name.starts_with("/dev/tty."))
        .map(|p| match p.port_type {
            SerialPortType::UsbPort(u) => PortInfo {
                likely_kdcan: u.vid == FTDI_VID,
                name: p.port_name,
                kind: "usb".into(),
                vid: Some(u.vid),
                pid: Some(u.pid),
                manufacturer: u.manufacturer,
                product: u.product,
                serial_number: u.serial_number,
            },
            other => PortInfo {
                name: p.port_name,
                kind: match other {
                    SerialPortType::PciPort => "pci",
                    SerialPortType::BluetoothPort => "bluetooth",
                    _ => "unknown",
                }
                .into(),
                vid: None,
                pid: None,
                manufacturer: None,
                product: None,
                serial_number: None,
                likely_kdcan: false,
            },
        })
        .collect();
    ports.sort_by(|a, b| {
        b.likely_kdcan
            .cmp(&a.likely_kdcan)
            .then(a.name.cmp(&b.name))
    });
    Ok(ports)
}

#[derive(Debug, Clone, Serialize)]
pub struct SerialConfig {
    pub baud: u32,
    /// DTR/RTS levels after opening. EdiabasLib opens K+DCAN with both off.
    pub dtr: bool,
    pub rts: bool,
    /// Blocking granularity of a single read call.
    pub poll_interval: Duration,
}

impl Default for SerialConfig {
    fn default() -> Self {
        // DS2 on MS43: 9600 8E1 (MS430DS0 xsetpar, RomRaider, Logger.S).
        Self {
            baud: 9600,
            dtr: false,
            rts: false,
            poll_interval: Duration::from_millis(2),
        }
    }
}

pub struct SerialLink {
    port: Box<dyn SerialPort>,
    name: String,
}

impl SerialLink {
    /// Opens the port as DS2 requires: 8 data bits, even parity, 1 stop bit, no flow control.
    pub fn open(name: &str, cfg: &SerialConfig) -> io::Result<Self> {
        let mut port = serialport::new(name, cfg.baud)
            .data_bits(DataBits::Eight)
            .parity(Parity::Even)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .timeout(cfg.poll_interval)
            .open()
            .map_err(io::Error::other)?;
        port.write_data_terminal_ready(cfg.dtr)
            .map_err(io::Error::other)?;
        port.write_request_to_send(cfg.rts)
            .map_err(io::Error::other)?;
        port.clear(ClearBuffer::All).map_err(io::Error::other)?;
        Ok(Self {
            port,
            name: name.to_string(),
        })
    }
}

impl Link for SerialLink {
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.port.write_all(bytes)?;
        // Waits until the bytes have left the UART driver (tcdrain / FlushFileBuffers).
        self.port.flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn clear_input(&mut self) -> io::Result<()> {
        self.port
            .clear(ClearBuffer::Input)
            .map_err(io::Error::other)
    }

    fn description(&self) -> String {
        self.name.clone()
    }
}
