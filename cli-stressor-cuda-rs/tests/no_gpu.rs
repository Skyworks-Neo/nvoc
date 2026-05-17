use cli_stressor_cuda_rs::{StressResult, choose_tolerance, parse_int_list, per_element_allclose};

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
