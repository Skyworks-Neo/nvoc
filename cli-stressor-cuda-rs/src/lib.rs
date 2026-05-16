use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_distr::StandardNormal;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecisionKind {
    FP64,
    FP32,
    TF32,
    FP16,
    BF16,
    FP8E4M3FN,
}

#[derive(Clone, Copy, Debug)]
pub struct PrecisionSpec {
    pub name: &'static str,
    pub kind: PrecisionKind,
    pub tf32_enabled: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct StressResult {
    pub precision: String,
    pub supported: bool,
    pub iterations: u64,
    pub total_flops: u128,
    pub elapsed_s: f64,
    pub compute_s: f64,
    pub tflops: f64,
    pub validations: u32,
    pub validation_failures: u32,
    pub max_abs_error: f32,
    pub max_rel_error: f32,
    pub first_error: Option<String>,
    pub first_error_at_s: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub total_mem_gb: Option<f64>,
    pub compute_capability: Option<(i32, i32)>,
}

#[derive(Debug, Clone)]
pub struct HostMatrix {
    pub size: usize,
    pub data: Vec<f32>,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[error("cuda backend is disabled")]
    Disabled,
    #[error("backend error: {0}")]
    Other(String),
}

pub trait Backend {
    type Matrix;
    type Output;

    fn device_info(&self) -> DeviceInfo;
    fn supports_precision(&self, spec: &PrecisionSpec) -> Result<(), String>;
    fn set_tf32(&mut self, enabled: Option<bool>) -> Result<(), BackendError>;
    fn upload_matrix(
        &self,
        host: &HostMatrix,
        spec: &PrecisionSpec,
    ) -> Result<Self::Matrix, BackendError>;
    fn gemm(
        &mut self,
        a: &Self::Matrix,
        b: &Self::Matrix,
        transpose_a: bool,
        transpose_b: bool,
    ) -> Result<Self::Output, BackendError>;
    fn output_to_f32(&self, output: &Self::Output) -> Result<Vec<f32>, BackendError>;
    fn synchronize(&self) -> Result<(), BackendError>;
    fn empty_cache(&self) -> Result<(), BackendError>;
}

pub fn parse_int_list(raw: &str) -> Result<Vec<usize>, String> {
    let mut values = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = trimmed
            .parse::<usize>()
            .map_err(|_| format!("invalid integer: {trimmed}"))?;
        values.push(value);
    }
    if values.is_empty() {
        return Err("matrix sizes cannot be empty".to_string());
    }
    Ok(values)
}

pub fn parse_precision_list(raw: &str) -> Result<Vec<PrecisionSpec>, String> {
    let mapping = [
        (
            "fp64",
            PrecisionSpec {
                name: "FP64",
                kind: PrecisionKind::FP64,
                tf32_enabled: None,
            },
        ),
        (
            "fp32",
            PrecisionSpec {
                name: "FP32",
                kind: PrecisionKind::FP32,
                tf32_enabled: Some(false),
            },
        ),
        (
            "tf32",
            PrecisionSpec {
                name: "TF32",
                kind: PrecisionKind::TF32,
                tf32_enabled: Some(true),
            },
        ),
        (
            "fp16",
            PrecisionSpec {
                name: "FP16",
                kind: PrecisionKind::FP16,
                tf32_enabled: None,
            },
        ),
        (
            "bf16",
            PrecisionSpec {
                name: "BF16",
                kind: PrecisionKind::BF16,
                tf32_enabled: None,
            },
        ),
        (
            "fp8",
            PrecisionSpec {
                name: "FP8 E4M3FN",
                kind: PrecisionKind::FP8E4M3FN,
                tf32_enabled: None,
            },
        ),
    ];

    let mut selected = Vec::new();
    for item in raw.split(',') {
        let key = item.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let spec = mapping
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, spec)| *spec)
            .ok_or_else(|| format!("unsupported precision: {item}"))?;
        selected.push(spec);
    }

    if selected.is_empty() {
        return Err("precision list cannot be empty".to_string());
    }
    Ok(selected)
}

pub fn choose_tolerance(precision_name: &str) -> (f32, f32) {
    match precision_name {
        "FP64" => (1e-5, 1e-5),
        "FP32" => (1e-2, 1e-2),
        "TF32" => (2e-1, 2e-1),
        "FP16" => (2e-1, 2e-1),
        "BF16" => (5e-1, 5e-1),
        "FP8 E4M3FN" => (1.5, 1.5),
        _ => (1e-2, 1e-2),
    }
}

pub fn per_element_allclose(diff: &[f32], reference: &[f32], atol: f32, rtol: f32) -> bool {
    diff.iter()
        .zip(reference.iter())
        .all(|(d, r)| *d <= atol + rtol * r.abs())
}

pub fn make_random_host_matrix(size: usize, seed: u64) -> HostMatrix {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut data = Vec::with_capacity(size * size);
    for _ in 0..size * size {
        let sample: f32 = rng.sample(StandardNormal);
        data.push(sample);
    }
    HostMatrix { size, data }
}

pub fn cpu_reference_f32(a: &HostMatrix, b: &HostMatrix) -> Vec<f32> {
    let size = a.size;
    let a_f64: Vec<f64> = a.data.iter().map(|&v| v as f64).collect();
    let b_f64: Vec<f64> = b.data.iter().map(|&v| v as f64).collect();
    let mut c_f64 = vec![0.0f64; size * size];

    unsafe {
        matrixmultiply::dgemm(
            size,
            size,
            size,
            1.0,
            a_f64.as_ptr(),
            1,
            size as isize,
            b_f64.as_ptr(),
            1,
            size as isize,
            0.0,
            c_f64.as_mut_ptr(),
            1,
            size as isize,
        );
    }

    c_f64.into_iter().map(|v| v as f32).collect()
}

pub fn validate_precision<B: Backend>(
    backend: &mut B,
    spec: &PrecisionSpec,
    validate_size: usize,
    seed: u64,
) -> Result<(bool, f32, f32, Option<String>), BackendError> {
    backend.set_tf32(spec.tf32_enabled)?;

    let a_host = make_random_host_matrix(validate_size, seed);
    let b_host = make_random_host_matrix(validate_size, seed.wrapping_add(1));
    let reference = cpu_reference_f32(&a_host, &b_host);

    let a_dev = backend.upload_matrix(&a_host, spec)?;
    let b_dev = backend.upload_matrix(&b_host, spec)?;
    let out = backend.gemm(&a_dev, &b_dev, false, false)?;
    backend.synchronize()?;
    let out_f32 = backend.output_to_f32(&out)?;

    let mut max_abs = 0.0f32;
    let mut max_rel = 0.0f32;
    let (abs_thr, rel_thr) = choose_tolerance(spec.name);
    let mut passed = true;
    let mut failures = 0usize;

    for (idx, (out, ref_val)) in out_f32.iter().zip(reference.iter()).enumerate() {
        if !out.is_finite() {
            return Ok((
                false,
                f32::INFINITY,
                f32::INFINITY,
                Some("validation produced NaN/Inf".to_string()),
            ));
        }
        let diff = (*out - *ref_val).abs();
        max_abs = max_abs.max(diff);
        let rel = diff / (ref_val.abs() + 1e-12);
        max_rel = max_rel.max(rel);
        if diff > abs_thr + rel_thr * ref_val.abs() {
            passed = false;
            failures += 1;
            if failures >= 1 {
                let reason = format!(
                    "{} elements exceed atol+rtol*|ref|: max_abs={:.4e}, max_rel={:.4e} (first idx={})",
                    failures, max_abs, max_rel, idx
                );
                return Ok((false, max_abs, max_rel, Some(reason)));
            }
        }
    }

    let reason = if passed {
        None
    } else {
        Some("validation failed".to_string())
    };
    Ok((passed, max_abs, max_rel, reason))
}

#[allow(clippy::too_many_arguments)]
pub fn run_stress_for_precision<B: Backend>(
    backend: &mut B,
    spec: PrecisionSpec,
    matrix_sizes: &[usize],
    duration_s: f64,
    warmup_iters: u32,
    burst_iters: u32,
    validate_interval_s: f64,
    validate_size: usize,
    transpose_prob: f64,
    base_seed: u64,
) -> StressResult {
    let mut result = StressResult {
        precision: spec.name.to_string(),
        supported: true,
        ..StressResult::default()
    };

    if let Err(reason) = backend.supports_precision(&spec) {
        result.supported = false;
        result.first_error = Some(format!("SKIP: {reason}"));
        return result;
    }

    if let Err(err) = backend.set_tf32(spec.tf32_enabled) {
        result.supported = false;
        result.first_error = Some(format!("tf32 setup failed: {err}"));
        return result;
    }

    // Probe for dtype support.
    let probe_a = make_random_host_matrix(8, base_seed.wrapping_add(1));
    let probe_b = make_random_host_matrix(8, base_seed.wrapping_add(2));
    if let Err(err) = (|| {
        let a_dev = backend.upload_matrix(&probe_a, &spec)?;
        let b_dev = backend.upload_matrix(&probe_b, &spec)?;
        let _ = backend.gemm(&a_dev, &b_dev, false, false)?;
        backend.synchronize()?;
        Ok::<(), BackendError>(())
    })() {
        result.supported = false;
        result.first_error = Some(format!("probe failed: {err}"));
        return result;
    }

    let mut rng = StdRng::seed_from_u64(base_seed);
    let start = Instant::now();
    let mut next_validate = validate_interval_s.max(0.0);
    let mut validation_seed = base_seed ^ 0x5F3759DF;

    while start.elapsed().as_secs_f64() < duration_s {
        let size = if rng.random::<f64>() < 0.15 {
            *matrix_sizes.choose(&mut rng).unwrap_or(&matrix_sizes[0])
        } else {
            let small_sizes = [127usize, 256, 511, 512, 1023];
            *small_sizes.choose(&mut rng).unwrap()
        };

        let transpose_a = rng.random::<f64>() < transpose_prob;
        let transpose_b = rng.random::<f64>() < transpose_prob;

        let a_host = make_random_host_matrix(size, rng.random::<u64>());
        let b_host = make_random_host_matrix(size, rng.random::<u64>());

        let op_elapsed = match (|| {
            let a_dev = backend.upload_matrix(&a_host, &spec)?;
            let b_dev = backend.upload_matrix(&b_host, &spec)?;

            for _ in 0..warmup_iters {
                let _ = backend.gemm(&a_dev, &b_dev, transpose_a, transpose_b)?;
            }
            backend.synchronize()?;

            let op_start = Instant::now();
            for _ in 0..burst_iters {
                let _ = backend.gemm(&a_dev, &b_dev, transpose_a, transpose_b)?;
            }
            backend.synchronize()?;
            let elapsed = op_start.elapsed().as_secs_f64();
            Ok::<f64, BackendError>(elapsed)
        })() {
            Ok(value) => value,
            Err(err) => {
                result.first_error = Some(format!("runtime error: {err}"));
                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                break;
            }
        };

        let burst_iters_f64 = burst_iters as f64;
        let flops = 2.0 * (size as f64).powi(3) * burst_iters_f64;
        let inst_tflops = if op_elapsed > 0.0 {
            flops / op_elapsed / 1e12
        } else {
            0.0
        };
        let elapsed_total = start.elapsed().as_secs_f64();

        println!(
            "[{}] t={:6.1}s/{:.0}s | size={:5} | inst={:7.2} TFLOPS",
            spec.name, elapsed_total, duration_s, size, inst_tflops
        );

        result.iterations += burst_iters as u64;
        result.total_flops += 2u128 * (size as u128).pow(3) * burst_iters as u128;
        result.compute_s += op_elapsed;
        result.elapsed_s = elapsed_total;
        if result.compute_s > 0.0 {
            result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
        }

        let _ = backend.empty_cache();

        if elapsed_total >= next_validate {
            match validate_precision(backend, &spec, validate_size, validation_seed) {
                Ok((passed, max_abs, max_rel, reason)) => {
                    let status = if passed { "OK" } else { "FAIL" };
                    println!(
                        "[{}] validate | abs={:.3e} | rel={:.3e} | {}",
                        spec.name, max_abs, max_rel, status
                    );
                    result.validations += 1;
                    result.max_abs_error = result.max_abs_error.max(max_abs);
                    result.max_rel_error = result.max_rel_error.max(max_rel);
                    if !passed {
                        result.validation_failures += 1;
                        if result.first_error.is_none() {
                            result.first_error = reason;
                            result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                        }
                        break;
                    }
                    next_validate = elapsed_total + validate_interval_s.max(0.0);
                    validation_seed = validation_seed.wrapping_add(1);
                }
                Err(err) => {
                    result.first_error = Some(format!("validation error: {err}"));
                    result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                    break;
                }
            }
        }

        if op_elapsed < 0.01 {
            std::thread::yield_now();
        }
    }

    result.elapsed_s = start.elapsed().as_secs_f64();
    if result.compute_s > 0.0 {
        result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
    }
    result
}
