use crate::scan_support::local_time_hms;
use nvoc_core::{KilohertzDelta, Microvolts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const SCHEMA_VERSION: u32 = 1;

pub type VoltagePointResume = (
    i32,
    i32,
    Option<usize>,
    Option<usize>,
    Option<usize>,
    Option<usize>,
);
pub type BreakPointResume = (Option<f64>, Option<f64>, Option<usize>, Option<bool>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanDomain {
    Core,
    Memory,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanKind {
    GpuBoostV3,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    Normal,
    Ultrafast,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestPhase {
    Short,
    Long,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuVoltageRange {
    pub gpu_id: u32,
    pub lower_point: usize,
    pub upper_point: usize,
    pub minimum_voltage_uv: u32,
    pub maximum_voltage_uv: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ScanLogEvent {
    VoltageRange {
        lower_point: usize,
        upper_point: usize,
        gpu_ranges: Vec<GpuVoltageRange>,
    },
    ScanMode {
        mode: ScanMode,
    },
    KeyPoints {
        points: [usize; 4],
    },
    TestResult {
        domain: ScanDomain,
        phase: TestPhase,
        test_code: usize,
        point: usize,
        voltage_uv: Option<u32>,
        delta_khz: i32,
        finished_at: Option<String>,
        result_code: Option<i32>,
    },
    PointFinished {
        domain: ScanDomain,
        point: usize,
    },
    ScanCompleted {
        scan: ScanKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanLogEntry {
    pub schema_version: u32,
    pub time: String,
    #[serde(flatten)]
    pub event: ScanLogEvent,
}

impl ScanLogEntry {
    fn new(event: ScanLogEvent) -> Self {
        Self::new_with_time(event, local_time_hms())
    }

    fn new_with_time(event: ScanLogEvent, time: String) -> Self {
        ScanLogEntry {
            schema_version: SCHEMA_VERSION,
            time,
            event,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanLogLoad {
    pub entries: Vec<ScanLogEntry>,
    pub had_errors: bool,
}

pub struct ScanLogWriter {
    file: File,
    pending_test_result_offset: Option<u64>,
}

impl ScanLogWriter {
    pub fn open_append(path: &str) -> io::Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.seek(SeekFrom::End(0))?;
        Ok(ScanLogWriter {
            file,
            pending_test_result_offset: None,
        })
    }

    pub fn write_event(&mut self, event: ScanLogEvent) -> io::Result<()> {
        self.write_entry(&ScanLogEntry::new(event))
    }

    fn write_entry(&mut self, entry: &ScanLogEntry) -> io::Result<()> {
        self.file.seek(SeekFrom::End(0))?;
        serde_json::to_writer(&mut self.file, entry)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        self.file.write_all(b"\n")?;
        self.file.flush()
    }

    fn overwrite_entry(&mut self, entry: &ScanLogEntry) -> io::Result<()> {
        let start = self
            .pending_test_result_offset
            .ok_or_else(|| io::Error::other("missing pending test result offset"))?;
        self.file.seek(SeekFrom::Start(start))?;
        serde_json::to_writer(&mut self.file, entry)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        self.file.write_all(b"\n")?;
        let end = self.file.stream_position()?;
        self.file.set_len(end)?;
        self.file.flush()?;
        self.pending_test_result_offset = None;
        Ok(())
    }

    pub fn write_pending_test_result(
        &mut self,
        domain: ScanDomain,
        phase: TestPhase,
        test_code: usize,
        point: usize,
        voltage: Option<Microvolts>,
        delta: KilohertzDelta,
    ) -> io::Result<String> {
        let started_at = local_time_hms();
        let entry = ScanLogEntry::new_with_time(
            ScanLogEvent::TestResult {
                domain,
                phase,
                test_code,
                point,
                voltage_uv: voltage.map(|v| v.0),
                delta_khz: delta.0,
                finished_at: None,
                result_code: None,
            },
            started_at.clone(),
        );
        let start = self.file.seek(SeekFrom::End(0))?;
        serde_json::to_writer(&mut self.file, &entry)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        self.pending_test_result_offset = Some(start);
        Ok(started_at)
    }

    pub fn write_completed_test_result(
        &mut self,
        started_at: String,
        domain: ScanDomain,
        phase: TestPhase,
        test_code: usize,
        point: usize,
        voltage: Option<Microvolts>,
        delta: KilohertzDelta,
        result_code: i32,
    ) -> io::Result<()> {
        let entry = ScanLogEntry::new_with_time(
            ScanLogEvent::TestResult {
                domain,
                phase,
                test_code,
                point,
                voltage_uv: voltage.map(|v| v.0),
                delta_khz: delta.0,
                finished_at: Some(local_time_hms()),
                result_code: Some(result_code),
            },
            started_at,
        );
        if self.pending_test_result_offset.is_some() {
            self.overwrite_entry(&entry)
        } else {
            self.write_entry(&entry)
        }
    }

    pub fn write_voltage_range(
        &mut self,
        lower_point: usize,
        upper_point: usize,
        gpu_ranges: Vec<GpuVoltageRange>,
    ) -> io::Result<()> {
        self.write_event(ScanLogEvent::VoltageRange {
            lower_point,
            upper_point,
            gpu_ranges,
        })
    }

    pub fn write_scan_mode(&mut self, mode: ScanMode) -> io::Result<()> {
        self.write_event(ScanLogEvent::ScanMode { mode })
    }

    pub fn write_key_points(&mut self, points: [usize; 4]) -> io::Result<()> {
        self.write_event(ScanLogEvent::KeyPoints { points })
    }

    #[allow(dead_code)]
    pub fn write_test_result(
        &mut self,
        domain: ScanDomain,
        phase: TestPhase,
        test_code: usize,
        point: usize,
        voltage: Option<Microvolts>,
        delta: KilohertzDelta,
        result_code: i32,
    ) -> io::Result<()> {
        self.write_completed_test_result(
            local_time_hms(),
            domain,
            phase,
            test_code,
            point,
            voltage,
            delta,
            result_code,
        )
    }

    pub fn write_point_finished(&mut self, domain: ScanDomain, point: usize) -> io::Result<()> {
        self.write_event(ScanLogEvent::PointFinished { domain, point })
    }

    pub fn write_scan_completed(&mut self, scan: ScanKind) -> io::Result<()> {
        self.write_event(ScanLogEvent::ScanCompleted { scan })
    }
}

pub fn read_scan_log(path: &str) -> io::Result<ScanLogLoad> {
    if !Path::new(path).exists() {
        return Ok(ScanLogLoad {
            entries: Vec::new(),
            had_errors: false,
        });
    }

    let file = File::open(path)?;
    let lines = BufReader::new(file)
        .lines()
        .collect::<io::Result<Vec<_>>>()?;
    let last_nonempty_line = lines.iter().rposition(|line| !line.trim().is_empty());
    let mut entries = Vec::new();
    let mut had_errors = false;

    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        let raw = line.trim();
        if raw.is_empty() {
            continue;
        }

        match parse_scan_log_entry(raw) {
            Ok(Some(entry)) => entries.push(entry),
            Ok(None) => {
                had_errors = true;
                eprintln!(
                    "Warning: ignoring unsupported JSONL schema in {} at line {}.",
                    path, line_no
                );
            }
            Err(err) => {
                had_errors = true;
                if last_nonempty_line == Some(index)
                    && let Some(recovered) = recover_last_line(raw)
                {
                    eprintln!(
                        "Warning: recovered trailing bytes in {} at line {} after JSON parse error: {}",
                        path, line_no, err
                    );
                    match parse_scan_log_entry(recovered) {
                        Ok(Some(entry)) => {
                            entries.push(entry);
                            continue;
                        }
                        Ok(None) => {
                            eprintln!(
                                "Warning: recovered line in {} at line {} has unsupported schema.",
                                path, line_no
                            );
                            continue;
                        }
                        Err(recovery_err) => {
                            eprintln!(
                                "Warning: recovery failed for {} at line {}: {}",
                                path, line_no, recovery_err
                            );
                        }
                    }
                }

                eprintln!(
                    "Warning: ignoring corrupt JSONL record in {} at line {}: {}",
                    path, line_no, err
                );
            }
        }
    }

    Ok(ScanLogLoad {
        entries,
        had_errors,
    })
}

fn parse_scan_log_entry(raw: &str) -> Result<Option<ScanLogEntry>, serde_json::Error> {
    let entry: ScanLogEntry = serde_json::from_str(raw)?;
    Ok((entry.schema_version == SCHEMA_VERSION).then_some(entry))
}

fn recover_last_line(raw: &str) -> Option<&str> {
    raw.rfind('}').and_then(|end| raw.get(..=end))
}

pub fn voltage_points_from_file(path: &str) -> io::Result<Option<VoltagePointResume>> {
    let load = read_scan_log(path)?;
    let resume = voltage_points_from_entries(&load.entries);
    if resume.is_some() && !allow_corrupt_resume(path, load.had_errors) {
        return Ok(None);
    }
    Ok(resume)
}

pub fn breakpoint_from_file(
    path: &str,
    testing_step: usize,
    safe_elastic_khz: i32,
    min_step_khz: i32,
) -> io::Result<BreakPointResume> {
    let load = read_scan_log(path)?;
    let resume =
        breakpoint_from_entries(&load.entries, testing_step, safe_elastic_khz, min_step_khz);
    if resume_has_data(&resume) && !allow_corrupt_resume(path, load.had_errors) {
        return Ok((None, None, None, None));
    }
    Ok(resume)
}

pub fn voltage_points_from_entries(entries: &[ScanLogEntry]) -> Option<VoltagePointResume> {
    let mut lower = None;
    let mut upper = None;
    let mut key_points = None;

    for entry in entries {
        match entry.event {
            ScanLogEvent::VoltageRange {
                lower_point,
                upper_point,
                ..
            } => {
                lower = i32::try_from(lower_point).ok();
                upper = i32::try_from(upper_point).ok();
            }
            ScanLogEvent::KeyPoints { points } => {
                key_points = Some(points);
            }
            _ => {}
        }
    }

    match (lower, upper) {
        (Some(lower), Some(upper)) => {
            let points = key_points.unwrap_or([0, 0, 0, 0]);
            Some((
                lower,
                upper,
                Some(points[0]),
                Some(points[1]),
                Some(points[2]),
                Some(points[3]),
            ))
        }
        _ => None,
    }
}

pub fn breakpoint_from_entries(
    entries: &[ScanLogEntry],
    testing_step: usize,
    safe_elastic_khz: i32,
    min_step_khz: i32,
) -> BreakPointResume {
    let mut last_succeeded_freq = None;
    let mut last_failed_freq = None;
    let mut last_voltage_point = None;
    let mut ultrafast_flag = None;
    let finished_last_point = matches!(
        entries.last().map(|entry| &entry.event),
        Some(ScanLogEvent::PointFinished { .. })
    );
    let mut completed_test_codes = HashSet::new();

    for entry in entries.iter().rev() {
        match &entry.event {
            ScanLogEvent::ScanCompleted { .. }
                if !resume_has_data(&(
                    last_succeeded_freq,
                    last_failed_freq,
                    last_voltage_point,
                    ultrafast_flag,
                )) =>
            {
                return (None, None, None, None);
            }
            ScanLogEvent::ScanMode { mode } => {
                if ultrafast_flag.is_none() {
                    ultrafast_flag = match *mode {
                        ScanMode::Ultrafast => Some(true),
                        ScanMode::Normal | ScanMode::Legacy => Some(false),
                    };
                }
                break;
            }
            ScanLogEvent::PointFinished { domain, point }
                if last_voltage_point.is_none()
                    && matches!(*domain, ScanDomain::Core | ScanDomain::Legacy) =>
            {
                last_voltage_point = (*point)
                    .checked_add(testing_step)
                    .filter(|point| is_plausible_voltage_point(*point));
                if last_voltage_point.is_none() {
                    eprintln!(
                        "Warning: ignoring suspicious resume point after finished point {point}."
                    );
                }
            }
            ScanLogEvent::TestResult {
                domain: ScanDomain::Core | ScanDomain::Legacy,
                point,
                delta_khz,
                finished_at: _finished_at,
                result_code,
                test_code,
                ..
            } => {
                if result_code.is_some() {
                    completed_test_codes.insert(*test_code);
                } else if completed_test_codes.contains(test_code) {
                    continue;
                }

                if last_voltage_point.is_none() {
                    if is_plausible_voltage_point(*point) {
                        last_voltage_point = Some(*point);
                    } else {
                        eprintln!("Warning: ignoring suspicious resume point {point} from JSONL.");
                    }
                }

                let delta_mhz = *delta_khz as f64 / 1000.0;
                if result_code.as_ref().copied().unwrap_or(1) == 0 {
                    last_succeeded_freq.get_or_insert(delta_mhz);
                } else {
                    last_failed_freq.get_or_insert(delta_mhz);
                }
            }
            _ => {}
        }

        // Keep walking until the matching scan_mode event so resumed scans can
        // inherit normal vs ultrafast mode even after enough test data is found.
    }

    if finished_last_point {
        let shift_mhz = safe_elastic_khz as f64 / 1000.0;
        if let Some(freq) = last_succeeded_freq.as_mut() {
            *freq -= shift_mhz;
        }
        if let Some(freq) = last_failed_freq.as_mut() {
            *freq += shift_mhz;
        }
    }

    if let (Some(s), Some(f)) = (last_succeeded_freq, last_failed_freq)
        && s >= f
    {
        last_succeeded_freq = Some(f - min_step_khz as f64 / 1000.0);
    }
    (
        last_succeeded_freq,
        last_failed_freq,
        last_voltage_point,
        ultrafast_flag,
    )
}

fn resume_has_data(resume: &BreakPointResume) -> bool {
    resume.0.is_some() || resume.1.is_some() || resume.2.is_some() || resume.3.is_some()
}

fn is_plausible_voltage_point(point: usize) -> bool {
    point <= 255
}

fn allow_corrupt_resume(path: &str, had_errors: bool) -> bool {
    if !had_errors {
        return true;
    }

    let decisions = CORRUPT_RESUME_DECISIONS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut decisions = decisions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(decision) = decisions.get(path) {
        return *decision;
    }

    eprintln!(
        "Warning: {} contains corrupt JSONL records. Resume state was recovered from valid records only.",
        path
    );

    if !io::stdin().is_terminal() {
        eprintln!(
            "Error: refusing to resume from corrupt JSONL without an interactive confirmation."
        );
        decisions.insert(path.to_string(), false);
        return false;
    }

    eprint!("Resume from the recovered JSONL scan state? [y/N]: ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let allow = match io::stdin().read_line(&mut answer) {
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(err) => {
            eprintln!("Error: failed to read resume confirmation: {err}");
            false
        }
    };

    if !allow {
        eprintln!("Not resuming from corrupt JSONL state; scanner will start from fresh state.");
    }

    decisions.insert(path.to_string(), allow);
    allow
}

static CORRUPT_RESUME_DECISIONS: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(event: ScanLogEvent) -> ScanLogEntry {
        ScanLogEntry {
            schema_version: SCHEMA_VERSION,
            time: "12:34:56".to_string(),
            event,
        }
    }

    #[test]
    fn voltage_points_use_latest_range_and_key_points() {
        let entries = vec![
            entry(ScanLogEvent::VoltageRange {
                lower_point: 10,
                upper_point: 90,
                gpu_ranges: Vec::new(),
            }),
            entry(ScanLogEvent::KeyPoints {
                points: [12, 24, 48, 80],
            }),
        ];

        assert_eq!(
            voltage_points_from_entries(&entries),
            Some((10, 90, Some(12), Some(24), Some(48), Some(80)))
        );
    }

    #[test]
    fn breakpoint_restores_latest_core_result() {
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 1,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 125000,
                finished_at: Some("12:34:57".to_string()),
                result_code: Some(0),
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 2,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 150000,
                finished_at: Some("12:34:58".to_string()),
                result_code: Some(100),
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 15_000),
            (Some(125.0), Some(150.0), Some(42), Some(false))
        );
    }

    #[test]
    fn breakpoint_advances_after_finished_point() {
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Ultrafast,
            }),
            entry(ScanLogEvent::PointFinished {
                domain: ScanDomain::Core,
                point: 42,
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 15_000, 15_000),
            (None, None, Some(45), Some(true))
        );
    }

    #[test]
    fn completed_scan_has_no_resume_state() {
        let entries = vec![entry(ScanLogEvent::ScanCompleted {
            scan: ScanKind::GpuBoostV3,
        })];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 15_000),
            (None, None, None, None)
        );
    }

    #[test]
    fn memory_events_do_not_drive_core_resume() {
        let entries = vec![
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Memory,
                phase: TestPhase::Long,
                test_code: 1,
                point: 90,
                voltage_uv: Some(950000),
                delta_khz: 800000,
                finished_at: Some("12:34:57".to_string()),
                result_code: Some(0),
            }),
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 15_000),
            (None, None, None, Some(false))
        );
    }

    #[test]
    fn breakpoint_counts_pending_result_as_failure() {
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 7,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 150000,
                finished_at: None,
                result_code: None,
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 15_000),
            (None, Some(150.0), Some(42), Some(false))
        );
    }

    #[test]
    fn breakpoint_ignores_pending_when_completed_exists() {
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 7,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 150000,
                finished_at: None,
                result_code: None,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 7,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 150000,
                finished_at: Some("12:34:57".to_string()),
                result_code: Some(0),
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 15_000),
            (Some(150.0), None, Some(42), Some(false))
        );
    }

    #[test]
    fn point_finished_at_log_end_applies_resume_shift() {
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 1,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 125000,
                finished_at: Some("12:34:57".to_string()),
                result_code: Some(0),
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 2,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 150000,
                finished_at: Some("12:34:58".to_string()),
                result_code: Some(100),
            }),
            entry(ScanLogEvent::PointFinished {
                domain: ScanDomain::Core,
                point: 42,
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 15_000, 15_000),
            (Some(110.0), Some(165.0), Some(45), Some(false))
        );
    }

    #[test]
    fn equal_succeeded_and_failed_deltas_back_off_succeeded() {
        // Both the last succeeded and last failed deltas collapsed to 247.5 MHz
        // (delta_khz 247500). The succeeded side is backed off by one minimal
        // step (7.5 MHz for a 7500 kHz step) so the resumed StepController can
        // make progress instead of initializing already-converged.
        let entries = vec![
            entry(ScanLogEvent::ScanMode {
                mode: ScanMode::Normal,
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 1,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 247500,
                finished_at: Some("12:34:57".to_string()),
                result_code: Some(0),
            }),
            entry(ScanLogEvent::TestResult {
                domain: ScanDomain::Core,
                phase: TestPhase::Short,
                test_code: 2,
                point: 42,
                voltage_uv: Some(875000),
                delta_khz: 247500,
                finished_at: Some("12:34:58".to_string()),
                result_code: Some(100),
            }),
        ];

        assert_eq!(
            breakpoint_from_entries(&entries, 3, 60_000, 7500),
            (Some(240.0), Some(247.5), Some(42), Some(false))
        );
    }

    #[test]
    fn read_scan_log_recovers_trailing_bytes_on_last_line() {
        let path = std::env::temp_dir().join(format!(
            "nvoc-vfp-jsonl-recovery-{}.jsonl",
            std::process::id()
        ));
        std::fs::write(
            &path,
            concat!(
                r#"{"schema_version":1,"time":"12:34:56","event":"scan_mode","mode":"normal"}"#,
                "trailing"
            ),
        )
        .unwrap();

        let loaded = read_scan_log(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(loaded.had_errors);
        assert_eq!(loaded.entries.len(), 1);
    }

    #[test]
    fn completed_test_result_overwrites_pending_line() {
        let path = std::env::temp_dir().join(format!(
            "nvoc-vfp-jsonl-overwrite-{}.jsonl",
            std::process::id()
        ));

        {
            let mut writer = ScanLogWriter::open_append(path.to_str().unwrap()).unwrap();
            let started_at = writer
                .write_pending_test_result(
                    ScanDomain::Core,
                    TestPhase::Short,
                    1,
                    85,
                    Some(Microvolts(981_250)),
                    KilohertzDelta(60_000),
                )
                .unwrap();
            writer
                .write_completed_test_result(
                    started_at,
                    ScanDomain::Core,
                    TestPhase::Short,
                    1,
                    85,
                    Some(Microvolts(981_250)),
                    KilohertzDelta(60_000),
                    0,
                )
                .unwrap();
        }

        let loaded = read_scan_log(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.entries.len(), 1);
        match &loaded.entries[0].event {
            ScanLogEvent::TestResult {
                finished_at,
                result_code,
                ..
            } => {
                assert!(finished_at.as_deref().is_some_and(|s| !s.is_empty()));
                assert_eq!(result_code, &Some(0));
            }
            other => panic!("unexpected event: {:?}", other),
        }
    }

    #[test]
    fn open_append_preserves_existing_entries() {
        let path = std::env::temp_dir().join(format!(
            "nvoc-vfp-jsonl-append-{}.jsonl",
            std::process::id()
        ));

        {
            let mut writer = ScanLogWriter::open_append(path.to_str().unwrap()).unwrap();
            writer
                .write_event(ScanLogEvent::ScanMode {
                    mode: ScanMode::Normal,
                })
                .unwrap();
        }
        {
            let mut writer = ScanLogWriter::open_append(path.to_str().unwrap()).unwrap();
            writer
                .write_event(ScanLogEvent::ScanCompleted {
                    scan: ScanKind::GpuBoostV3,
                })
                .unwrap();
        }

        let loaded = read_scan_log(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.entries.len(), 2);
        assert!(matches!(
            loaded.entries[0].event,
            ScanLogEvent::ScanMode {
                mode: ScanMode::Normal
            }
        ));
        assert!(matches!(
            loaded.entries[1].event,
            ScanLogEvent::ScanCompleted {
                scan: ScanKind::GpuBoostV3
            }
        ));
    }
}
