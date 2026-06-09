use std::fs;
use std::path::Path;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TrialRow {
    pub trial_id: usize,
    pub timestamp_unix_ms: u64,
    pub mode: String,
    pub precision: String,
    pub seed: u64,
    pub matrix_n: usize,
    pub voltage_mv: u32,
    pub freq_mhz: i64,
    pub result_type: String,
    pub l2_diff: f64,
    pub hamming_dist: usize,
    pub max_abs_diff: f32,
    #[serde(serialize_with = "serialize_option_bool")]
    pub abft_row_ok: Option<bool>,
    #[serde(serialize_with = "serialize_option_bool")]
    pub abft_col_ok: Option<bool>,
    #[serde(serialize_with = "serialize_option_f32")]
    pub abft_row_residual: Option<f32>,
    #[serde(serialize_with = "serialize_option_f32")]
    pub abft_col_residual: Option<f32>,
    #[serde(serialize_with = "serialize_option_bool")]
    pub abft_detected: Option<bool>,
    #[serde(serialize_with = "serialize_option_bool")]
    pub golden_detected: Option<bool>,
    pub exec_time_ms: u128,
    pub avg_power_mw: i64,
    pub energy_mj: i64,
}

pub struct Logger {
    writer: csv::Writer<fs::File>,
}

impl Logger {
    pub fn new(path: &Path) -> Result<Self, anyhow::Error> {
        let file = fs::File::create(path)?;
        let writer = csv::Writer::from_writer(file);
        Ok(Self { writer })
    }

    pub fn write_trial(&mut self, row: &TrialRow) -> Result<(), anyhow::Error> {
        self.writer.serialize(row)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), anyhow::Error> {
        self.writer.flush()?;
        Ok(())
    }
}

fn serialize_option_bool<S>(value: &Option<bool>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(v) => serializer.serialize_str(if *v { "true" } else { "false" }),
        None => serializer.serialize_str(""),
    }
}

fn serialize_option_f32<S>(value: &Option<f32>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(v) => serializer.serialize_str(&v.to_string()),
        None => serializer.serialize_str(""),
    }
}
