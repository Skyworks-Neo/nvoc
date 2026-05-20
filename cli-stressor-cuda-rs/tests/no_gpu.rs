use cli_stressor_cuda_rs::{
    KernelType, StressResult, choose_tolerance, parse_int_list, parse_kernel_mixture,
    parse_kernel_param_overrides, parse_kernel_type_list, parse_stream_mode, per_element_allclose,
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
            && v.matrix_sizes.as_ref().map(|s| s.as_slice()) == Some(&[2049, 4096])
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
            .any(|v| v.kind == KernelType::Gemm
                && v.precisions.as_ref().map(|p| p.len()) == Some(2))
    );
}
