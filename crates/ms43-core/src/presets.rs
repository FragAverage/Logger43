//! Channel presets. Channels that are unavailable for the connected DME (e.g. RAM channels on an
//! unknown software version) are skipped by the planner and reported, never guessed.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub channels: &'static [&'static str],
}

pub static PRESETS: &[Preset] = &[
    Preset {
        id: "power_run",
        name: "Power Run",
        description:
            "3rd/4th gear WOT pulls. Sample rate first: 0B03 every cycle + 0B90 for VANOS \
                      targets + one RAM read for knock retard when the DME software is known.",
        channels: &[
            "rpm",
            "load_mg_stroke",
            "throttle_deg",
            "maf_g_s",
            "ignition_deg",
            "knock_retard_deg",
            "vanos_intake_target",
            "vanos_intake_actual",
            "vanos_exhaust_target",
            "vanos_exhaust_actual",
            "injection_ms",
            "lambda_int_1",
            "coolant_c",
        ],
    },
    Preset {
        id: "ignition_knock",
        name: "Ignition / Knock",
        description:
            "Ignition angle, per-cylinder ignition and knock retard (RAM, community defs).",
        channels: &[
            "rpm",
            "load_mg_stroke",
            "ignition_deg",
            "ign_cyl_1",
            "ign_cyl_2",
            "ign_cyl_3",
            "ign_cyl_4",
            "ign_cyl_5",
            "ign_cyl_6",
            "knock_retard_deg",
            "knock_signal_2_v",
            "knock_signal_5_v",
            "iat_c",
            "coolant_c",
        ],
    },
    Preset {
        id: "vanos",
        name: "VANOS",
        description: "Intake/exhaust cam target vs actual from the 0B90 block.",
        channels: &[
            "rpm",
            "load_mg_stroke",
            "vanos_intake_target",
            "vanos_intake_actual_b90",
            "vanos_exhaust_target",
            "vanos_exhaust_actual_b90",
            "oil_c",
            "coolant_c",
        ],
    },
    Preset {
        id: "fueling",
        name: "Fueling",
        description: "Air mass, load, injection time and closed-loop lambda corrections.",
        channels: &[
            "rpm",
            "load_mg_stroke",
            "throttle_deg",
            "maf_g_s",
            "injection_ms",
            "lambda_int_1",
            "lambda_int_2",
            "iat_c",
            "coolant_c",
        ],
    },
    Preset {
        id: "idle",
        name: "Idle",
        description: "Idle speed control and supporting values.",
        channels: &[
            "rpm",
            "idle_integrator_pct",
            "idle_actuator_pct",
            "maf_kg_h",
            "ignition_deg",
            "lambda_int_1",
            "lambda_int_2",
            "coolant_c",
            "battery_v",
        ],
    },
    Preset {
        id: "raw_debug",
        name: "Raw Debug",
        description: "Single 0B03 request with RPM only. Minimal traffic for protocol diagnosis.",
        channels: &["rpm"],
    },
];

pub fn find(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels;

    #[test]
    fn presets_reference_existing_channels() {
        for p in PRESETS {
            for c in p.channels {
                assert!(
                    channels::find(c).is_some(),
                    "preset {} references unknown channel {c}",
                    p.id
                );
            }
        }
    }
}
