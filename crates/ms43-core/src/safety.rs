//! Read-only request construction and the default-deny service allow-list.
//!
//! `Request` is the only type the transport will put on the wire. Its fields are private and its
//! constructors only produce identification, status-block reads and memory reads. The transport
//! additionally re-runs [`check_payload`] on every frame before writing (defence in depth), so even
//! a future code path that builds bytes some other way cannot issue a write service.

use serde::Serialize;
use thiserror::Error;

use crate::ds2;

/// Largest memory read we issue; RomRaider's own range limit for `06 00` (EcuQueryRangeTest 128).
pub const MAX_MEMORY_READ: u8 = 128;

/// DS2 `0B xx` read blocks this app is allowed to request. Closed set — see docs/PROTOCOL.md §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum StatusBlock {
    /// `0B 03` main measurement block (MS430DS0 `BETRIEBSWTAB`).
    Measurements,
    /// `0B 04` switch / flag bits (RomRaider community layout).
    Switches,
    /// `0B 90` VANOS actual / target (MS430DS0).
    Vanos,
    /// `0B 91` lambda adaptations (MS430DS0).
    LambdaAdaptation,
    /// `0B 92` idle / throttle adaptations (MS430DS0).
    IdleThrottleAdaptation,
    /// `0B 93` octane (RON) adaptation (MS430DS0).
    OctaneAdaptation,
}

impl StatusBlock {
    pub const ALL: [StatusBlock; 6] = [
        StatusBlock::Measurements,
        StatusBlock::Switches,
        StatusBlock::Vanos,
        StatusBlock::LambdaAdaptation,
        StatusBlock::IdleThrottleAdaptation,
        StatusBlock::OctaneAdaptation,
    ];

    pub fn sub_command(self) -> u8 {
        match self {
            StatusBlock::Measurements => 0x03,
            StatusBlock::Switches => 0x04,
            StatusBlock::Vanos => 0x90,
            StatusBlock::LambdaAdaptation => 0x91,
            StatusBlock::IdleThrottleAdaptation => 0x92,
            StatusBlock::OctaneAdaptation => 0x93,
        }
    }

    fn from_sub_command(sub: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.sub_command() == sub)
    }

    pub fn name(self) -> &'static str {
        match self {
            StatusBlock::Measurements => "0B03 measurements",
            StatusBlock::Switches => "0B04 switches",
            StatusBlock::Vanos => "0B90 VANOS",
            StatusBlock::LambdaAdaptation => "0B91 lambda adaptation",
            StatusBlock::IdleThrottleAdaptation => "0B92 idle/throttle adaptation",
            StatusBlock::OctaneAdaptation => "0B93 octane adaptation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RequestKind {
    Ident,
    StatusBlock(StatusBlock),
    ReadMemory { address: u32, len: u8 },
}

/// A request that has passed the read-only allow-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    kind: RequestKind,
    frame: Vec<u8>,
}

impl Request {
    /// `12 04 00 16` — DS2 identification (MS430DS0 job `IDENT`).
    pub fn ident() -> Self {
        Self::build(RequestKind::Ident, &[0x00])
    }

    /// `12 05 0B xx CS` — one of the allowed status blocks.
    pub fn status_block(block: StatusBlock) -> Self {
        Self::build(
            RequestKind::StatusBlock(block),
            &[0x0B, block.sub_command()],
        )
    }

    /// `12 09 06 00 SG HI LO NN CS` — read `len` bytes of ECU memory at a 24-bit address.
    pub fn read_memory(address: u32, len: u8) -> Result<Self, SafetyError> {
        if address > 0x00FF_FFFF {
            return Err(SafetyError::AddressOutOfRange(address));
        }
        if len == 0 || len > MAX_MEMORY_READ {
            return Err(SafetyError::BadReadLength(len));
        }
        let [_, sg, hi, lo] = address.to_be_bytes();
        Ok(Self::build(
            RequestKind::ReadMemory { address, len },
            &[0x06, 0x00, sg, hi, lo, len],
        ))
    }

    fn build(kind: RequestKind, payload: &[u8]) -> Self {
        check_payload(payload)
            .expect("Request constructor produced a payload outside the allow-list");
        Self {
            kind,
            frame: ds2::build_frame(ds2::ADDR_DME, payload),
        }
    }

    pub fn kind(&self) -> &RequestKind {
        &self.kind
    }

    /// Full frame including address, length and checksum.
    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    pub fn describe(&self) -> String {
        match &self.kind {
            RequestKind::Ident => "IDENT".into(),
            RequestKind::StatusBlock(b) => b.name().into(),
            RequestKind::ReadMemory { address, len } => format!("read {len} B @ 0x{address:06X}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum SafetyError {
    #[error("blocked DS2 service {0}: not on the read-only allow-list")]
    Blocked(String),
    #[error("memory address 0x{0:X} exceeds 24 bits")]
    AddressOutOfRange(u32),
    #[error("memory read length {0} outside 1..={MAX_MEMORY_READ}")]
    BadReadLength(u8),
    #[error("frame is not a well-formed DS2 request to the DME")]
    Malformed,
}

/// Default-deny allow-list on the request payload (bytes between length and checksum).
pub fn check_payload(payload: &[u8]) -> Result<(), SafetyError> {
    match payload {
        [0x00] => Ok(()),
        [0x0B, sub] if StatusBlock::from_sub_command(*sub).is_some() => Ok(()),
        [0x06, 0x00, _, _, _, len] if (1..=MAX_MEMORY_READ).contains(len) => Ok(()),
        [0x06, 0x00, _, _, _, len] => Err(SafetyError::BadReadLength(*len)),
        _ => Err(SafetyError::Blocked(ds2::hex(payload))),
    }
}

/// Checks a complete frame (as it will be written) against the allow-list.
pub fn check_frame(frame: &[u8]) -> Result<(), SafetyError> {
    let ok_shape = frame.len() >= ds2::MIN_FRAME_LEN
        && frame[0] == ds2::ADDR_DME
        && frame[1] as usize == frame.len()
        && ds2::checksum(&frame[..frame.len() - 1]) == frame[frame.len() - 1];
    if !ok_shape {
        return Err(SafetyError::Malformed);
    }
    check_payload(&frame[2..frame.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_requests_encode_as_documented() {
        assert_eq!(Request::ident().frame(), &[0x12, 0x04, 0x00, 0x16]);
        assert_eq!(
            Request::status_block(StatusBlock::Measurements).frame(),
            &[0x12, 0x05, 0x0B, 0x03, 0x1F]
        );
        assert_eq!(
            Request::status_block(StatusBlock::Vanos).frame(),
            &[0x12, 0x05, 0x0B, 0x90, 0x8C]
        );
        // RomRaider/MS41 documented layout: 12 09 06 00 SG HI LO NN CS
        let r = Request::read_memory(0x00DA34, 2).unwrap();
        assert_eq!(
            &r.frame()[..8],
            &[0x12, 0x09, 0x06, 0x00, 0x00, 0xDA, 0x34, 0x02]
        );
        for b in StatusBlock::ALL {
            check_frame(Request::status_block(b).frame()).unwrap();
        }
    }

    #[test]
    fn write_and_control_services_are_blocked() {
        let blocked: &[&[u8]] = &[
            &[0x07, 0x00, 0x00, 0xE0, 0xE4, 0x01, 0x00], // write memory
            &[0x0B, 0x01, 0x01, 0x01, 0x00, 0x00, 0xE0, 0xE4], // load telegram list
            &[0x0B, 0x00],                               // read telegram list (needs 0B01)
            &[0x91, 0x00, 0x25, 0x80, 0x00],             // baud change
            &[0x91, 0x01, 0xE8, 0x48, 0x00],
            &[0x43, 0x00, 0x00], // reset
            &[0x05],             // clear faults
            &[0x04],             // fault memory
            &[0x0C, 0x00],
            &[0x22, 0x83, 0x00], // actuator (STEUERN_*)
            &[0x2B, 0xA0],       // system checks
            &[0x0B, 0xFF],
            &[],
        ];
        for p in blocked {
            assert!(
                check_payload(p).is_err(),
                "payload {:02X?} must be blocked",
                p
            );
        }
        assert!(Request::read_memory(0x0100_0000, 1).is_err());
        assert!(Request::read_memory(0xE0E4, 0).is_err());
        assert!(Request::read_memory(0xE0E4, 129).is_err());
    }

    #[test]
    fn check_frame_rejects_tampered_frames() {
        let mut f = Request::ident().frame().to_vec();
        f[2] = 0x07;
        assert_eq!(check_frame(&f), Err(SafetyError::Malformed)); // checksum now wrong
        let f = ds2::build_frame(0x12, &[0x07, 0x00, 0x00, 0xE0, 0xE4, 0x01]);
        assert!(matches!(check_frame(&f), Err(SafetyError::Blocked(_))));
    }
}
