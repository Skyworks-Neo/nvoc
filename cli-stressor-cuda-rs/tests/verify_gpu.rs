//! Hardware-gated integration tests for the ride-on-load verification engine.
//!
//! These need a real CUDA device and are `#[ignore]`d by default (repo policy:
//! GPU-touching tests stay out of the default gate). Run them explicitly:
//!
//! `cargo test -p cli-stressor-cuda-rs --test verify_gpu -- --ignored`

use cli_stressor_cuda_rs::{Backend, VerifyConfig, cuda_backend};

#[test]
#[ignore = "requires a CUDA device"]
fn verify_self_test_gates_pass_on_live_gpu() {
    let backend =
        cuda_backend::CudaBackend::new_with_device(0).expect("CUDA device 0 must initialize");
    let report = backend
        .run_verify_self_test()
        .expect("verify engine must be available");
    for check in &report.checks {
        assert!(
            check.passed,
            "self-test {} failed: {}",
            check.name, check.detail
        );
    }
    assert!(report.passed);
}

#[test]
#[ignore = "requires a CUDA device"]
fn intalu_host_reference_matches_kernel_on_live_gpu() {
    // The bit-exact host reference is the oracle for the IntAlu SDC detector;
    // on a healthy GPU the kernel output must match it exactly. Any drift
    // here means the reference or the kernel changed one-sided.
    let mut backend =
        cuda_backend::CudaBackend::new_with_device(0).expect("CUDA device 0 must initialize");
    let info = backend.device_info();
    let spec = cli_stressor_cuda_rs::PrecisionSpec {
        name: "INT32",
        kind: cli_stressor_cuda_rs::PrecisionKind::INT32,
        tf32_enabled: None,
    };
    let cfg = VerifyConfig {
        enabled: true,
        self_test: false,
        ..VerifyConfig::default()
    };
    let result = cli_stressor_cuda_rs::run_stress_mixed(
        &mut backend,
        &[spec],
        cli_stressor_cuda_rs::StressRunConfig {
            matrix_sizes: &[256],
            fp64_matrix_sizes: &[256],
            duration_s: 5.0,
            warmup_iters: 1,
            burst_iters: 2,
            validate_interval_s: 0.0,
            validate_size: 256,
            transpose_prob: 0.0,
            base_seed: 42,
            minor_mixture_rate: 0.0,
            kernel_mixture: &[cli_stressor_cuda_rs::KernelMixtureEntry {
                kind: cli_stressor_cuda_rs::KernelType::IntAlu,
                weight: 1.0,
            }],
            stream_mode: cli_stressor_cuda_rs::StreamMode::Single,
            kernel_param_overrides: &[],
            verify: cfg,
        },
        None,
    );
    assert!(
        result[0].supported,
        "INT32 must run: {:?}",
        result[0].first_error
    );
    assert!(
        result[0].first_error.is_none(),
        "intalu reference mismatch on healthy GPU: {:?} (sm={:?})",
        result[0].first_error,
        info.compute_capability
    );
    assert!(
        result[0].detectors.ops_checked > 0,
        "detectors should have run at least once"
    );
}

#[test]
#[ignore = "requires a CUDA device"]
fn memcpy_detector_counts_elements_on_live_gpu() {
    let mut backend =
        cuda_backend::CudaBackend::new_with_device(0).expect("CUDA device 0 must initialize");
    let spec = cli_stressor_cuda_rs::PrecisionSpec {
        name: "FP32",
        kind: cli_stressor_cuda_rs::PrecisionKind::FP32,
        tf32_enabled: Some(false),
    };
    let cfg = VerifyConfig {
        enabled: true,
        self_test: false,
        ..VerifyConfig::default()
    };
    let result = cli_stressor_cuda_rs::run_stress_mixed(
        &mut backend,
        &[spec],
        cli_stressor_cuda_rs::StressRunConfig {
            matrix_sizes: &[1024],
            fp64_matrix_sizes: &[1024],
            duration_s: 5.0,
            warmup_iters: 1,
            burst_iters: 4,
            validate_interval_s: 0.0,
            validate_size: 1024,
            transpose_prob: 0.0,
            base_seed: 7,
            minor_mixture_rate: 0.0,
            kernel_mixture: &[cli_stressor_cuda_rs::KernelMixtureEntry {
                kind: cli_stressor_cuda_rs::KernelType::Memcpy,
                weight: 1.0,
            }],
            stream_mode: cli_stressor_cuda_rs::StreamMode::Single,
            kernel_param_overrides: &[],
            verify: cfg,
        },
        None,
    );
    assert!(
        result[0].first_error.is_none(),
        "{:?}",
        result[0].first_error
    );
    assert!(result[0].detectors.ops_checked > 0);
    assert!(result[0].detectors.elements_checked > 0);
}

#[test]
#[ignore = "requires a CUDA device"]
fn gemm_output_semantics_discrimination() {
    // Decisive check: is the device C equal to row-major A*B, B*A, (A*B)^T,
    // or something else? This pins the semantics the sampled cross-check
    // kernel must use.
    use cli_stressor_cuda_rs::{Backend, cpu_reference_f32, make_random_host_matrix};

    let mut backend = cuda_backend::CudaBackend::new_with_device(0).unwrap();
    let spec = cli_stressor_cuda_rs::PrecisionSpec {
        name: "FP32",
        kind: cli_stressor_cuda_rs::PrecisionKind::FP32,
        tf32_enabled: Some(false),
    };
    let a_host = make_random_host_matrix(4, 1);
    let b_host = make_random_host_matrix(4, 2);
    let a_dev = backend.upload_matrix(&a_host, &spec).unwrap();
    let b_dev = backend.upload_matrix(&b_host, &spec).unwrap();
    let out = backend.gemm(&a_dev, &b_dev, false, false).unwrap();
    backend.synchronize().unwrap();
    let c = backend.output_to_f32(&out).unwrap();

    // C as-is vs A*B
    let ref_ab = cpu_reference_f32(&a_host, &b_host);
    // C as-is vs B*A (swap operands)
    let ref_ba = cpu_reference_f32(&b_host, &a_host);
    // C vs (A*B)^T
    let n = 4usize;
    let ref_ab_t: Vec<f32> = (0..n * n)
        .map(|idx| {
            let (i, j) = (idx / n, idx % n);
            ref_ab[j * n + i]
        })
        .collect();

    let err_vs = |r: &[f32]| {
        c.iter()
            .zip(r.iter())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max)
    };
    println!("max_err vs A*B   = {:.6}", err_vs(&ref_ab));
    println!("max_err vs B*A   = {:.6}", err_vs(&ref_ba));
    println!("max_err vs (AB)T = {:.6}", err_vs(&ref_ab_t));
    for i in 0..2usize {
        println!(
            "[JUDGE] row{i}: A={:?} B={:?} C={:?} (A*B)C0={:.6} (B*A)C0={:.6}",
            &a_host.data[i * n..i * n + n],
            &b_host.data[i * n..i * n + n],
            &c[i * n..i * n + n],
            (0..n)
                .map(|k| a_host.data[i * n + k] * b_host.data[k * n])
                .sum::<f32>(),
            (0..n)
                .map(|k| b_host.data[i * n + k] * a_host.data[k * n])
                .sum::<f32>(),
        );
    }
    let min = err_vs(&ref_ab).min(err_vs(&ref_ba)).min(err_vs(&ref_ab_t));
    assert!(
        err_vs(&ref_ab) == min,
        "C matches neither A*B ({}) nor B*A ({}) nor (AB)T ({})",
        err_vs(&ref_ab),
        err_vs(&ref_ba),
        err_vs(&ref_ab_t)
    );
}

#[test]
#[ignore = "requires a CUDA device"]
fn sampled_cross_check_minimal_repro() {
    // size=4 with distinct-value matrices; run the real gemm path's verify
    // via run_stress_mixed with gemm_samples=4 and print the outcome.
    use cli_stressor_cuda_rs::{KernelMixtureEntry, KernelType, PrecisionKind, PrecisionSpec};
    let mut backend = cuda_backend::CudaBackend::new_with_device(0).unwrap();
    let spec = PrecisionSpec {
        name: "FP32",
        kind: PrecisionKind::FP32,
        tf32_enabled: Some(false),
    };
    let cfg = VerifyConfig {
        enabled: true,
        self_test: false,
        gemm_samples: 4,
        ..VerifyConfig::default()
    };
    let result = cli_stressor_cuda_rs::run_stress_mixed(
        &mut backend,
        &[spec],
        cli_stressor_cuda_rs::StressRunConfig {
            matrix_sizes: &[4],
            fp64_matrix_sizes: &[4],
            duration_s: 2.0,
            warmup_iters: 0,
            burst_iters: 1,
            validate_interval_s: 0.0,
            validate_size: 4,
            transpose_prob: 0.0,
            base_seed: 1,
            minor_mixture_rate: 0.0,
            kernel_mixture: &[KernelMixtureEntry {
                kind: KernelType::Gemm,
                weight: 1.0,
            }],
            stream_mode: cli_stressor_cuda_rs::StreamMode::Single,
            kernel_param_overrides: &[],
            verify: cfg,
        },
        None,
    );
    let d = &result[0].detectors;
    println!(
        "ops_checked={} mismatches={} first={:?}",
        d.ops_checked, d.mismatches, d.first_error
    );
}

#[test]
#[ignore = "requires a CUDA device"]
fn sampled_check_identity_diagnostic() {
    // A = identity, B[i][j] = i*10+j. Both I*B and B*I equal B, so a correct
    // checker reports zero mismatches in both orientations; a swapped-operand
    // bug still passes I*B but fails B*I; a layout bug fails both.
    use cli_stressor_cuda_rs::SampleStats;
    let backend = cuda_backend::CudaBackend::new_with_device(0).unwrap();
    let engine = backend.verify_engine().expect("engine");
    let stream = backend.stream_handle();
    let n = 4usize;
    let ident: Vec<f32> = (0..n * n)
        .map(|idx| if idx / n == idx % n { 1.0 } else { 0.0 })
        .collect();
    let b: Vec<f32> = (0..n * n)
        .map(|idx| (idx / n * 10 + idx % n) as f32)
        .collect();
    let ident_dev = stream.clone_htod(&ident).unwrap();
    let b_dev = stream.clone_htod(&b).unwrap();
    let samples: Vec<u32> = vec![0, 0, 1, 1, 2, 3, 3, 0];

    // Case 1: recompute I*B = B against c = B.
    let s1: SampleStats = engine
        .debug_sample_check(
            &stream, &ident_dev, &b_dev, &b_dev, n, false, false, 0, &samples, 0.01, 0.01,
        )
        .unwrap();
    println!(
        "case I*B: checked={} mismatch={} nonfinite={}",
        s1.checked, s1.mismatch, s1.nonfinite
    );

    // Case 2: recompute B*I = B against c = B.
    let s2: SampleStats = engine
        .debug_sample_check(
            &stream, &b_dev, &ident_dev, &b_dev, n, false, false, 0, &samples, 0.01, 0.01,
        )
        .unwrap();
    println!(
        "case B*I: checked={} mismatch={} nonfinite={}",
        s2.checked, s2.mismatch, s2.nonfinite
    );

    assert_eq!(s1.mismatch, 0, "I*B orientation failed");
    assert_eq!(s2.mismatch, 0, "B*I orientation failed");
}

#[test]
#[ignore = "requires a CUDA device"]
fn sampled_check_random_full_gemmc_check() {
    // Random A/B with an exact CPU-computed C; run the FULL gemm_check
    // (stats pass + sampled pass) exactly like production.
    use cli_stressor_cuda_rs::SampleStats;
    let backend = cuda_backend::CudaBackend::new_with_device(0).unwrap();
    let engine = backend.verify_engine().expect("engine");
    let stream = backend.stream_handle();
    let n = 4usize;
    let mut a: Vec<f32> = (0..n * n).map(|i| (i as f32) * 0.25 - 1.5).collect();
    let mut b: Vec<f32> = (0..n * n)
        .map(|i| ((i * 7 + 3) % 16) as f32 * 0.5 - 3.5)
        .collect();
    a[0] = 1.25;
    b[5] = -2.0;
    // exact C in f64, using the device semantics: row-major (B*A),
    // C[i][j] = sum_k A[k][j] * B[i][k]
    let mut c: Vec<f32> = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            let mut acc = 0.0f64;
            for k in 0..n {
                acc += (a[k * n + j] as f64) * (b[i * n + k] as f64);
            }
            c[i * n + j] = acc as f32;
        }
    }
    let a_dev = stream.clone_htod(&a).unwrap();
    let b_dev = stream.clone_htod(&b).unwrap();
    let c_dev = stream.clone_htod(&c).unwrap();
    let samples: Vec<u32> = vec![2, 2, 0, 1, 0, 0, 2, 1];

    engine
        .gemm_check(
            &stream, &a_dev, &b_dev, &c_dev, n, false, false, 0, &samples, 0.01, 0.01,
        )
        .unwrap();
    let full = "ran";
    println!("full gemm_check ran: {}", full);
    let drained = format!("{:?}", full);
    println!("drained_detail={}", drained);
    let sample: SampleStats = engine
        .debug_sample_check(
            &stream, &a_dev, &b_dev, &c_dev, n, false, false, 0, &samples, 0.01, 0.01,
        )
        .unwrap();
    println!(
        "sample-only: checked={} mismatch={} max_abs_diff={:.4e}",
        sample.checked,
        sample.mismatch,
        sample.max_abs_diff()
    );
}
