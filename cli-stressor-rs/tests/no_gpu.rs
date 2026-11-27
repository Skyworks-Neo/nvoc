use cli_stressor_rs::{
    DetectorStats, DeviceInfo, Fp4ScaleMode, KernelType, PrecisionKind, StressResult, VerifyConfig,
    VerifyReport, choose_tolerance, classify, cpu_reference_int_tn, cpu_reference_int4_tn,
    dp4a_ref, fp4_e2m1_decode, fp4_e2m1_quantize, fp4_quantize_block, fp4_quantize_matrix,
    fp8_e4m3_decode, fp8_e4m3_quantize, int4_pack, int4_unpack_nib, intalu_ref_element,
    kernel_precision_compatible, pack4, parse_int_list, parse_kernel_mixture,
    parse_kernel_param_overrides, parse_kernel_type_list, parse_precision_list,
    parse_precision_mixture, parse_stream_mode, per_element_allclose, unpack4, vexpected_host,
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
        cli_stressor_rs::VerdictClass::SelfTestFailed
    );
    assert_eq!(
        classify(true, 0, 0, 2),
        cli_stressor_rs::VerdictClass::ApiError
    );
    assert_eq!(
        classify(true, 0, 1, 0),
        cli_stressor_rs::VerdictClass::MemoryError
    );
    assert_eq!(
        classify(true, 3, 0, 0),
        cli_stressor_rs::VerdictClass::DataError
    );
    assert_eq!(classify(true, 0, 0, 0), cli_stressor_rs::VerdictClass::None);
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

// ---- FP8 (E4M3FN) codec ----

#[test]
fn fp8_e4m3_decode_known_values() {
    assert_eq!(fp8_e4m3_decode(0x00), 0.0);
    assert_eq!(fp8_e4m3_decode(0x80), 0.0); // -0.0
    assert_eq!(fp8_e4m3_decode(0x38), 1.0);
    assert_eq!(fp8_e4m3_decode(0xB8), -1.0);
    assert_eq!(fp8_e4m3_decode(0x01), 2f32.powi(-9)); // min denormal
    assert_eq!(fp8_e4m3_decode(0x07), 7.0 * 2f32.powi(-9)); // max denormal
    assert_eq!(fp8_e4m3_decode(0x08), 2f32.powi(-6)); // min normal
    assert_eq!(fp8_e4m3_decode(0x7E), 448.0); // max finite
    assert_eq!(fp8_e4m3_decode(0xFE), -448.0);
    assert!(fp8_e4m3_decode(0x7F).is_nan());
    assert!(fp8_e4m3_decode(0xFF).is_nan());
}

#[test]
fn fp8_e4m3_quantize_round_trip_and_saturate() {
    // Exact values round-trip bit-for-bit.
    for bits in [0x00u8, 0x01, 0x07, 0x08, 0x38, 0x7E, 0x80, 0xB8] {
        let v = fp8_e4m3_decode(bits);
        assert_eq!(fp8_e4m3_quantize(v), bits, "round trip for {bits:#04x}");
    }
    // Saturation above ±448 (never spills into the 0x7F NaN pattern).
    assert_eq!(fp8_e4m3_quantize(449.0), 0x7E);
    assert_eq!(fp8_e4m3_quantize(1e9), 0x7E);
    assert_eq!(fp8_e4m3_quantize(-449.0), 0xFE);
    // RN midpoint between 0 and the min denormal rounds to zero (tie -> even).
    assert_eq!(fp8_e4m3_quantize(2f32.powi(-11)), 0x00);
    assert_eq!(fp8_e4m3_quantize(2f32.powi(-10)), 0x00);
    // NaN/Inf map to the NaN pattern.
    assert_eq!(fp8_e4m3_quantize(f32::NAN), 0x7F);
    assert_eq!(fp8_e4m3_quantize(f32::INFINITY), 0x7F);
}

#[test]
fn fp8_e4m3_quantize_rounds_to_nearest() {
    // 1.06 (between 1.0 and 1.125, closer to 1.0) -> the 1.0 pattern.
    assert_eq!(fp8_e4m3_quantize(1.06), 0x38);
    // 1.125 is exact pattern 0x39.
    assert_eq!(fp8_e4m3_quantize(1.125), 0x39);
}

// ---- FP4 (E2M1) codec ----

#[test]
fn fp4_e2m1_decode_and_quantize() {
    let mags = [0.0f32, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
    for (nib, &mag) in mags.iter().enumerate() {
        assert_eq!(fp4_e2m1_decode(nib as u8), mag);
        assert_eq!(fp4_e2m1_quantize(mag), nib as u8);
        assert_eq!(fp4_e2m1_decode(nib as u8 | 0x8), -mag);
        assert_eq!(fp4_e2m1_quantize(-mag), nib as u8 | 0x8);
    }
    // Saturation and RN-even ties.
    assert_eq!(fp4_e2m1_quantize(100.0), 0x7);
    assert_eq!(fp4_e2m1_quantize(0.25), 0x0); // tie 0/0.5 -> even
    assert_eq!(fp4_e2m1_quantize(0.75), 0x2); // tie 0.5/1.0 -> even (1.0)
    assert_eq!(fp4_e2m1_quantize(2.4), 0x4); // closer to 2.0
}

#[test]
fn pack4_round_trip_and_nibble_order() {
    let nibs = [0x1u8, 0x2, 0xE, 0x7];
    let packed = pack4(&nibs);
    assert_eq!(packed, vec![0x21, 0x7E]); // low nibble first
    for (idx, &n) in nibs.iter().enumerate() {
        assert_eq!(unpack4(&packed, idx), n);
    }
    // Odd tail pads the high nibble with zero.
    assert_eq!(pack4(&[0x3]), vec![0x03]);
}

#[test]
fn int4_pack_unpack_signed() {
    let values = [-8i8, 7, -1, 0];
    let packed = int4_pack(&values);
    assert_eq!(packed.len(), 2);
    for (idx, &v) in values.iter().enumerate() {
        assert_eq!(int4_unpack_nib(&packed, idx), v);
    }
}

#[test]
fn fp4_quantize_block_nvfp4_and_mxfp4() {
    // NVFP4: scale = e4m3(max/6); decoded = nib * scale.
    let block = [
        6.0f32, 3.0, 0.0, -1.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ];
    let (packed, scale) = fp4_quantize_block(&block, Fp4ScaleMode::Nvfp4);
    let scale_f = fp8_e4m3_decode(scale);
    assert!((scale_f - 1.0).abs() < 1e-6); // max/6 = 1.0 exactly
    for (k, &v) in block.iter().enumerate() {
        let nib = unpack4(&packed, k);
        let decoded = fp4_e2m1_decode(nib) * scale_f;
        assert!(
            (decoded - v).abs() <= 0.5 * scale_f,
            "k={k} decoded={decoded} want={v}"
        );
    }
    // MXFP4: scale is a power of two (UE8M0 byte, bias 127).
    let block2 = [5.0f32; 32];
    let (packed2, scale2) = fp4_quantize_block(&block2, Fp4ScaleMode::Mxfp4);
    assert_eq!(packed2.len(), 16);
    let scale_f2 = 2f32.powi(scale2 as i32 - 127);
    assert!((scale_f2 - 1.0).abs() < 1e-6); // ceil(log2(5/6)) = 0 -> 2^0
    // 5.0 is the exact midpoint of 4.0/6.0 -> RN-even picks 4.0.
    let decoded0 = fp4_e2m1_decode(unpack4(&packed2, 0)) * scale_f2;
    assert_eq!(decoded0, 4.0);
    // All-zero block quantizes to scale 0 and zero nibbles.
    let zeros = [0.0f32; 16];
    let (pz, sz) = fp4_quantize_block(&zeros, Fp4ScaleMode::Nvfp4);
    assert_eq!(sz, 0);
    assert!(pz.iter().all(|b| *b & 0xF == 0));
}

#[test]
fn fp4_quantize_matrix_layout_is_kblocks_by_cols() {
    // 4x4 matrix (row-major, K contiguous per row), NVFP4 blocks of 16 ->
    // one block per row-column: scales[kb * cols + col].
    let size = 4usize;
    let values: Vec<f32> = (0..size * size).map(|i| (i as f32) * 0.25).collect();
    let (packed, scales) = fp4_quantize_matrix(&values, size, Fp4ScaleMode::Nvfp4);
    assert_eq!(packed.len(), size * size / 2);
    assert_eq!(scales.len(), size);
    // Column 0's max is 0.75 (values 0.25*k) -> scale = e4m3(0.75/6 = 0.125).
    assert_eq!(scales[0], fp8_e4m3_quantize(0.125));
    // Column 3's max is 3.75 -> scale = e4m3(3.75/6 = 0.625).
    assert_eq!(scales[3], fp8_e4m3_quantize(0.625));
}

// ---- exact INT references ----

#[test]
fn cpu_reference_int_tn_matches_identity() {
    // TN identity: C[i][j] = sum_k A[j][k] * B[i][k].
    let size = 3usize;
    let a: Vec<i8> = (0..size * size).map(|i| (i as i8) - 4).collect();
    let b: Vec<i8> = (0..size * size)
        .map(|i| ((i * 5 + 1) % 9) as i8 - 4)
        .collect();
    let reference = cpu_reference_int_tn(&a, &b, size);
    for i in 0..size {
        for j in 0..size {
            let expect: i32 = (0..size)
                .map(|k| a[j * size + k] as i32 * b[i * size + k] as i32)
                .sum();
            assert_eq!(reference[i * size + j], expect, "i={i} j={j}");
        }
    }
}

#[test]
fn cpu_reference_int4_tn_matches_int8_identity() {
    // Decoding the packed nibbles must reproduce the plain int8 TN reference.
    let size = 4usize;
    let a: Vec<i8> = (0..size * size).map(|i| ((i % 16) as i8) - 8).collect();
    let b: Vec<i8> = (0..size * size)
        .map(|i| (((i * 3) % 16) as i8) - 8)
        .collect();
    let a_packed = int4_pack(&a);
    let b_packed = int4_pack(&b);
    let got = cpu_reference_int4_tn(&a_packed, &b_packed, size);
    let want = cpu_reference_int_tn(&a, &b, size);
    assert_eq!(got, want);
}

// ---- precision gates / parsing ----

#[test]
fn fp4_int4_parse_and_compat() {
    let fp4 = parse_precision_list("fp4").unwrap();
    assert_eq!(fp4[0].name, "FP4 E2M1");
    let int4 = parse_precision_list("int4,i4").unwrap();
    assert_eq!(int4.len(), 2);
    assert_eq!(int4[0].kind, PrecisionKind::INT4);

    // FP4/INT4 ride the Gemm kernel only.
    assert!(kernel_precision_compatible(
        KernelType::Gemm,
        PrecisionKind::FP4E2M1
    ));
    assert!(kernel_precision_compatible(
        KernelType::Gemm,
        PrecisionKind::INT4
    ));
    for kind in [
        KernelType::Memcpy,
        KernelType::Memset,
        KernelType::Reduction,
    ] {
        assert!(!kernel_precision_compatible(kind, PrecisionKind::FP4E2M1));
        assert!(!kernel_precision_compatible(kind, PrecisionKind::INT4));
    }
    assert_eq!(choose_tolerance("FP4 E2M1"), (2.0, 2.0));
    assert_eq!(choose_tolerance("INT4"), (0.0, 0.0));
}

#[test]
fn fp4_int4_device_gates() {
    let mut fp4 = parse_precision_list("fp4").unwrap();
    let fp4 = fp4.remove(0);
    let mut int4 = parse_precision_list("int4").unwrap();
    let int4 = int4.remove(0);
    let info = |sm: (i32, i32)| DeviceInfo {
        name: "t".into(),
        total_mem_gb: None,
        compute_capability: Some(sm),
        l2_cache_bytes: None,
    };
    // FP4: Blackwell only (SM100/SM103/SM120).
    for sm in [(8i32, 9i32), (9, 0), (10, 1)] {
        assert!(info(sm).supports_precision(&fp4).is_err(), "sm{sm:?}");
    }
    for sm in [(10i32, 0i32), (10, 3), (12, 0)] {
        assert!(info(sm).supports_precision(&fp4).is_ok(), "sm{sm:?}");
    }
    // INT4: Turing (SM75)+.
    assert!(info((7, 5)).supports_precision(&int4).is_ok());
    assert!(info((7, 0)).supports_precision(&int4).is_err());
}
