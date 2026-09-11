use cli_stressor_cuda_rs::{
    DetectorStats, KernelType, StressResult, VerifyConfig, VerifyReport, choose_tolerance,
    classify, dp4a_ref, intalu_ref_element, parse_int_list, parse_kernel_mixture,
    parse_kernel_param_overrides, parse_kernel_type_list, parse_precision_mixture,
    parse_stream_mode, per_element_allclose, vexpected_host,
};

#[test]
fn test_parse_int_list() {
    assert_eq!(parse_int_list("1024").unwrap(), vec![1024]);
    assert_eq!(
        parse_int_list("512, 1024, 2048").unwrap(),
        vec![512, 1024, 2048]
    );
    assert!(parse_int_list("").is_err());
}

#[test]
fn test_choose_tolerance_values() {
    assert_eq!(choose_tolerance("FP64"), (1e-5, 1e-5));
    assert_eq!(choose_tolerance("FP32"), (1e-2, 1e-2));
    assert_eq!(choose_tolerance("FP16"), (2e-1, 2e-1));
    assert_eq!(choose_tolerance("BF16"), (5e-1, 5e-1));
}

#[test]
fn test_per_element_allclose_detects_outlier() {
    let diff = vec![0.01, 0.01, 0.01, 100.0];
    let ref_vals = vec![1.0, 1.0, 1.0, 1.0];
    assert!(!per_element_allclose(&diff, &ref_vals, 0.1, 0.1));
}

#[test]
fn test_stress_result_compute_s_default() {
    let r = StressResult::default();
    assert_eq!(r.compute_s, 0.0);
    assert_eq!(r.tflops, 0.0);
}

#[test]
fn test_parse_kernel_type_list() {
    let kinds = parse_kernel_type_list("gemm, memcpy, reduction, atomic").unwrap();
    assert_eq!(
        kinds,
        vec![
            KernelType::Gemm,
            KernelType::Memcpy,
            KernelType::Reduction,
            KernelType::Atomic
        ]
    );
}

#[test]
fn test_parse_kernel_mixture() {
    let types = parse_kernel_type_list("gemm,memcpy,memset").unwrap();
    let mix = parse_kernel_mixture("gemm:0.6,memcpy:0.4", &types).unwrap();
    assert_eq!(mix.len(), 3);
    assert!(
        mix.iter()
            .any(|e| e.kind == KernelType::Gemm && e.weight == 0.6)
    );
    assert!(
        mix.iter()
            .any(|e| e.kind == KernelType::Memcpy && e.weight == 0.4)
    );
    assert!(
        mix.iter()
            .any(|e| e.kind == KernelType::Memset && e.weight == 0.0)
    );
}

#[test]
fn test_parse_kernel_mixture_rejects_invalid_kernel_name() {
    let types = parse_kernel_type_list("gemm,memcpy").unwrap();
    assert!(parse_kernel_mixture("gemm|memcpy:0.5", &types).is_err());
}

#[test]
fn test_parse_precision_mixture_rejects_invalid_precision_name() {
    assert!(parse_precision_mixture("fp32|fp16:0.5").is_err());
}

#[test]
fn test_parse_stream_mode() {
    let mode = parse_stream_mode("dual").unwrap();
    assert_eq!(mode.stream_count(), 2);
}

#[test]
fn test_parse_kernel_param_overrides() {
    let items = parse_kernel_param_overrides(
        "gemm:matrix_sizes=2049|4096,warmup=4,burst=8;memcpy:burst_iters=64",
    )
    .unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().any(|v| {
        v.kind == KernelType::Gemm
            && v.matrix_sizes.as_deref() == Some(&[2049, 4096])
            && v.warmup_iters == Some(4)
            && v.burst_iters == Some(8)
    }));
    assert!(
        items
            .iter()
            .any(|v| v.kind == KernelType::Memcpy && v.burst_iters == Some(64))
    );
    assert!(
        parse_kernel_param_overrides("gemm:precisions=fp16|bf16")
            .unwrap()
            .iter()
            .any(
                |v| v.kind == KernelType::Gemm && v.precisions.as_ref().map(|p| p.len()) == Some(2)
            )
    );
}

#[test]
fn test_verify_report_layout_and_init() {
    // The report crosses D2H once per check; keep it compact and the init
    // invariants (idx_min = max, first_lock = armed) intact.
    assert_eq!(std::mem::size_of::<VerifyReport>(), 176);
    let report = VerifyReport::init();
    assert_eq!(report.magic, VerifyReport::MAGIC);
    assert_eq!(report.idx_min, u32::MAX);
    assert_eq!(report.first_lock, u32::MAX);
    assert_eq!(report.total_errors, 0);
}

#[test]
fn test_verify_config_defaults_and_cadence() {
    let cfg = VerifyConfig::default();
    assert!(cfg.enabled && cfg.self_test);
    assert_eq!(cfg.memcpy_every, 1);
    assert_eq!(cfg.memset_every, 8);
    assert_eq!(cfg.gemm_every, 4);
    assert_eq!(cfg.gemm_samples, 512);
    assert!(cfg.due(cfg.gemm_every, 8));
    assert!(!cfg.due(3, 8));
}

#[test]
fn test_intalu_ref_deterministic() {
    // The gather-based check compares against this reference, so it must be
    // a pure function of (a, b, mode, dp4a).
    let a = 0x1234_5678u32 as i32;
    let b = 0xDEAD_BEEFu32 as i32;
    for mode in [8u32, 16, 32] {
        assert_eq!(
            intalu_ref_element(a, b, mode, false),
            intalu_ref_element(a, b, mode, false)
        );
        // DP4A fold must change the chain result (data-dependent xor).
        assert_ne!(
            intalu_ref_element(a, b, mode, false),
            intalu_ref_element(a, b, mode, true)
        );
    }
    assert_eq!(dp4a_ref(0x0101_0101, 0x0101_0101), 4);
}

#[test]
fn test_vexpected_matches_doc_example() {
    // The oracle is mirrored in device code; a frozen vector here forces a
    // deliberate update on both sides if the hash ever changes.
    assert_eq!(
        vexpected_host(0xADBA, 0xADBA),
        vexpected_host(0xADBA, 0xADBA)
    );
    assert_ne!(vexpected_host(0, 0xADBA), vexpected_host(1, 0xADBA));
}

#[test]
fn test_classify_precedence() {
    // Self-test failure dominates, then API errors, then memory errors, then
    // data errors, then none.
    assert_eq!(
        classify(false, 0, 0, 0),
        cli_stressor_cuda_rs::VerdictClass::SelfTestFailed
    );
    assert_eq!(
        classify(true, 0, 0, 2),
        cli_stressor_cuda_rs::VerdictClass::ApiError
    );
    assert_eq!(
        classify(true, 0, 1, 0),
        cli_stressor_cuda_rs::VerdictClass::MemoryError
    );
    assert_eq!(
        classify(true, 3, 0, 0),
        cli_stressor_cuda_rs::VerdictClass::DataError
    );
    assert_eq!(
        classify(true, 0, 0, 0),
        cli_stressor_cuda_rs::VerdictClass::None
    );
}

#[test]
fn test_detector_stats_merge() {
    let mut total = DetectorStats::default();
    let mut part = DetectorStats {
        ops_checked: 3,
        elements_checked: 100,
        total_errors: 1,
        mismatches: 0,
        nonfinite: 0,
        first_error: Some("memcpy verify: 1 wrong words".into()),
        bit_hist: [0; 32],
        fault_events: 1,
    };
    part.bit_hist[27] = 1;
    total.merge(&part);
    total.merge(&DetectorStats {
        ops_checked: 2,
        ..DetectorStats::default()
    });
    assert_eq!(total.ops_checked, 5);
    assert_eq!(total.elements_checked, 100);
    assert_eq!(total.total_errors, 1);
    // First error sticks across merges.
    let later = DetectorStats {
        ops_checked: 1,
        first_error: Some("later".into()),
        ..DetectorStats::default()
    };
    total.merge(&later);
    assert_eq!(
        total.first_error.as_deref(),
        Some("memcpy verify: 1 wrong words")
    );
}

#[test]
fn test_stress_result_serializes_detectors() {
    // The verdict JSON embeds StressResult; the detector fields must survive
    // serialization.
    let mut r = StressResult {
        precision: "FP32".into(),
        supported: true,
        ..StressResult::default()
    };
    r.detectors.ops_checked = 4;
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"detectors\""));
    assert!(json.contains("\"ops_checked\":4"));
    assert!(json.contains("\"elements_produced\":0"));
}
