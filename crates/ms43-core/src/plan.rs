//! Turns a channel selection into a polling plan.
//!
//! One job per DS2 request: each status block used by any selected channel, plus merged RAM
//! ranges. A job inherits the highest priority of its channels. Scheduling is cycle based:
//! a job with period `p` runs on cycles where `(cycle + phase) % p == 0`.
//! FAST = every cycle, NORMAL = every 3rd, SLOW = every 10th (relative to the fastest job present,
//! so selecting only slow channels does not idle the bus).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::channels::{self, ChannelDef, DataType, Priority, Source};
use crate::safety::{Request, StatusBlock, MAX_MEMORY_READ};

/// Merge RAM reads whose addresses are at most this many bytes apart.
const MERGE_GAP: u32 = 8;

#[derive(Debug, Clone)]
pub struct Field {
    pub column: usize,
    pub def: &'static ChannelDef,
    /// Frame byte index (0 = address byte).
    pub offset: usize,
    pub dtype: DataType,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub request: Request,
    pub priority: Priority,
    pub period: u32,
    pub phase: u32,
    pub fields: Vec<Field>,
}

impl Job {
    pub fn due(&self, cycle: u64) -> bool {
        (cycle + self.phase as u64).is_multiple_of(self.period as u64)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Unavailable {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanSummary {
    pub columns: Vec<&'static str>,
    pub requests: Vec<JobSummary>,
    pub unavailable: Vec<Unavailable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSummary {
    pub request: String,
    pub frame: String,
    pub priority: Priority,
    pub every_n_cycles: u32,
    pub channels: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub columns: Arc<Vec<&'static ChannelDef>>,
    pub jobs: Vec<Job>,
    pub unavailable: Vec<Unavailable>,
}

impl Plan {
    pub fn empty() -> Self {
        Self {
            columns: Arc::new(Vec::new()),
            jobs: Vec::new(),
            unavailable: Vec::new(),
        }
    }

    pub fn summary(&self) -> PlanSummary {
        PlanSummary {
            columns: self.columns.iter().map(|c| c.id).collect(),
            requests: self
                .jobs
                .iter()
                .map(|j| JobSummary {
                    request: j.request.describe(),
                    frame: crate::ds2::hex(j.request.frame()),
                    priority: j.priority,
                    every_n_cycles: j.period,
                    channels: j.fields.iter().map(|f| f.def.id).collect(),
                })
                .collect(),
            unavailable: self.unavailable.clone(),
        }
    }
}

/// `part_number` is the DME `ID_BMW_NR`; RAM channels need it to pick an address.
pub fn build(ids: &[String], part_number: Option<&str>) -> Plan {
    let mut columns: Vec<&'static ChannelDef> = Vec::new();
    let mut unavailable = Vec::new();
    let mut blocks: BTreeMap<u8, (StatusBlock, Vec<Field>)> = BTreeMap::new();
    let mut ram: Vec<(u32, Field)> = Vec::new();

    for id in ids {
        let Some(def) = channels::find(id) else {
            unavailable.push(Unavailable {
                id: id.clone(),
                reason: "unknown channel id".into(),
            });
            continue;
        };
        if columns.iter().any(|c| c.id == def.id) {
            continue;
        }
        match def.source {
            Source::Block {
                block,
                offset,
                dtype,
            } => {
                let column = columns.len();
                columns.push(def);
                blocks
                    .entry(block.sub_command())
                    .or_insert_with(|| (block, Vec::new()))
                    .1
                    .push(Field {
                        column,
                        def,
                        offset,
                        dtype,
                    });
            }
            Source::Memory { dtype, .. } => {
                let Some(pn) = part_number else {
                    unavailable.push(Unavailable {
                        id: id.clone(),
                        reason: "RAM channel needs the DME part number (not connected yet)".into(),
                    });
                    continue;
                };
                let Some(addr) = def.memory_address(pn) else {
                    unavailable.push(Unavailable {
                        id: id.clone(),
                        reason: format!("no known RAM address for DME software {pn}"),
                    });
                    continue;
                };
                let column = columns.len();
                columns.push(def);
                ram.push((
                    addr,
                    Field {
                        column,
                        def,
                        offset: 0,
                        dtype,
                    },
                ));
            }
        }
    }

    let mut jobs: Vec<Job> = blocks
        .into_values()
        .map(|(block, fields)| Job {
            request: Request::status_block(block),
            priority: best_priority(&fields),
            period: 1,
            phase: 0,
            fields,
        })
        .collect();

    // Merge RAM addresses into as few reads as possible.
    ram.sort_by_key(|(a, _)| *a);
    let mut group: Vec<(u32, Field)> = Vec::new();
    let flush = |group: &mut Vec<(u32, Field)>, jobs: &mut Vec<Job>| {
        if group.is_empty() {
            return;
        }
        let base = group[0].0;
        let end = group
            .iter()
            .map(|(a, f)| a + f.dtype.size() as u32)
            .max()
            .unwrap();
        let fields: Vec<Field> = group
            .drain(..)
            .map(|(a, mut f)| {
                f.offset = 3 + (a - base) as usize; // data starts after 12 LL A0
                f
            })
            .collect();
        jobs.push(Job {
            request: Request::read_memory(base, (end - base) as u8)
                .expect("merged range within limits"),
            priority: best_priority(&fields),
            period: 1,
            phase: 0,
            fields,
        });
    };
    for (addr, field) in ram {
        if let Some((base, _)) = group.first() {
            let group_end = group
                .iter()
                .map(|(a, f)| a + f.dtype.size() as u32)
                .max()
                .unwrap();
            let new_end = (addr + field.dtype.size() as u32).max(group_end);
            if addr > group_end + MERGE_GAP || new_end - base > MAX_MEMORY_READ as u32 {
                flush(&mut group, &mut jobs);
            }
        }
        group.push((addr, field));
    }
    flush(&mut group, &mut jobs);

    // Periods relative to the fastest priority present.
    if let Some(top) = jobs.iter().map(|j| j.priority).min() {
        let rank = |p: Priority| p as u32 - top as u32;
        let mut phases: BTreeMap<u32, u32> = BTreeMap::new();
        for j in &mut jobs {
            j.period = match rank(j.priority) {
                0 => 1,
                1 => 3,
                _ => 10,
            };
            // spread slower jobs over different cycles
            let n = phases.entry(j.period).or_insert(0);
            j.phase = *n % j.period;
            *n += 1;
        }
    }

    Plan {
        columns: Arc::new(columns),
        jobs,
        unavailable,
    }
}

fn best_priority(fields: &[Field]) -> Priority {
    fields
        .iter()
        .map(|f| f.def.priority)
        .min()
        .unwrap_or(Priority::Slow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn power_run_on_known_software() {
        let p = build(
            &ids(presets::find("power_run").unwrap().channels),
            Some("7551615"),
        );
        assert!(p.unavailable.is_empty(), "{:?}", p.unavailable);
        // 0B03 + 0B90 + one RAM read for knock
        assert_eq!(p.jobs.len(), 3);
        assert!(p.jobs.iter().all(|j| j.period == 1));
        let ram = p
            .jobs
            .iter()
            .find(|j| j.request.frame()[2] == 0x06)
            .unwrap();
        assert_eq!(
            ram.request.frame(),
            &[
                0x12,
                0x09,
                0x06,
                0x00,
                0x00,
                0xFB,
                0x6E,
                0x01,
                ram.request.frame()[8]
            ]
        );
        assert_eq!(ram.fields[0].offset, 3);
    }

    #[test]
    fn power_run_on_unknown_software_skips_ram() {
        let p = build(
            &ids(presets::find("power_run").unwrap().channels),
            Some("SIM4300"),
        );
        assert_eq!(p.unavailable.len(), 1);
        assert_eq!(p.unavailable[0].id, "knock_retard_deg");
        assert_eq!(p.jobs.len(), 2);
    }

    #[test]
    fn per_cylinder_ignition_merges_into_one_read() {
        let p = build(
            &ids(&[
                "ign_cyl_1",
                "ign_cyl_2",
                "ign_cyl_3",
                "ign_cyl_4",
                "ign_cyl_5",
                "ign_cyl_6",
            ]),
            Some("7551615"),
        );
        assert_eq!(p.jobs.len(), 1);
        let f = p.jobs[0].request.frame();
        assert_eq!(&f[2..8], &[0x06, 0x00, 0x00, 0xFB, 0xAB, 6]);
        let cyl4 = p.jobs[0]
            .fields
            .iter()
            .find(|f| f.def.id == "ign_cyl_4")
            .unwrap();
        assert_eq!(cyl4.offset, 3 + (0xFBB0 - 0xFBAB));
    }

    #[test]
    fn slow_jobs_are_spread() {
        // rpm (fast, 0B03) + vanos target (fast, 0B90) — then only slow 0B03 channels
        let p = build(&ids(&["coolant_c", "iat_c"]), None);
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(
            p.jobs[0].period, 1,
            "fastest job present always runs every cycle"
        );
        let p = build(&ids(&["vanos_intake_target", "coolant_c"]), None);
        // two different blocks: 0B90 fast, 0B03 slow -> every 10th cycle
        let b03 = p
            .jobs
            .iter()
            .find(|j| j.request.frame()[3] == 0x03)
            .unwrap();
        assert_eq!(b03.period, 10);
    }
}
