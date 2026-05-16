use clap::Parser;

#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::parse_int_list;

use cli_stressor_cuda_rs::Backend;
#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::{
    DeviceInfo, PrecisionKind, StressResult, parse_precision_list, run_stress_for_precision,
};

#[cfg(feature = "cuda")]
mod cuda_backend;

#[derive(Parser, Debug)]
#[command(
    name = "cli-stressor-cuda-rs",
    about = "GPU core-domain stressor (Rust): randomized GEMM + validation sidecar"
)]
struct Args {
    #[arg(long, default_value_t = 90.0)]
    duration: f64,

    #[arg(long, default_value = "2049,4096,4097,8192,8193,16384")]
    matrix_sizes: String,

    #[arg(long, default_value = "2048,4096")]
    fp64_matrix_sizes: String,

    #[arg(long, default_value = "fp16,bf16")]
    precisions: String,

    #[arg(long, default_value_t = 3)]
    warmup_iters: u32,

    #[arg(long, default_value_t = 6)]
    burst_iters: u32,

    #[arg(long, default_value_t = 10.0)]
    validate_interval: f64,

    #[arg(long, default_value_t = 1024)]
    validate_size: usize,

    #[arg(long, default_value_t = 0.5)]
    transpose_prob: f64,

    #[arg(long, default_value_t = 12345)]
    seed: u64,

    #[arg(long)]
    disable_fp8: bool,
}

#[cfg(feature = "cuda")]
fn print_device_info(info: &DeviceInfo) {
    println!("Testing Device: {}", info.name);
    if let Some((major, minor)) = info.compute_capability {
        println!("Compute Capability: SM{}.{}", major, minor);
    }
    if let Some(mem) = info.total_mem_gb {
        println!("Video Memory: {:.1} GB", mem);
    }
}

#[cfg(feature = "cuda")]
fn print_summary(results: &[StressResult], info: &DeviceInfo) {
    println!("\n{}", "=".repeat(72));
    println!("Phase 1 core stability summary");
    println!("Testing Device: {}", info.name);
    if let Some(mem) = info.total_mem_gb {
        println!("Video Memory: {:.1} GB", mem);
    }

    let mut overall_ok = true;
    for r in results {
        let status = if !r.supported {
            "SKIP"
        } else if r.first_error.is_none() && r.validation_failures == 0 {
            "OK"
        } else {
            "FAIL"
        };
        if status == "FAIL" {
            overall_ok = false;
        }
        let eff = if r.elapsed_s > 0.0 {
            r.compute_s / r.elapsed_s * 100.0
        } else {
            0.0
        };
        println!(
            "{:<12} {:<4} | iters={:8} | wall={:7.1}s | compute={:6.1}s | eff={:4.0}% | {:8.2} TFLOPS | val_fail={:3} | max_abs={:.3e} | max_rel={:.3e}",
            r.precision,
            status,
            r.iterations,
            r.elapsed_s,
            r.compute_s,
            eff,
            r.tflops,
            r.validation_failures,
            r.max_abs_error,
            r.max_rel_error
        );
        if let Some(err) = &r.first_error {
            println!("{:12}      first_error: {}", "", err);
            if let Some(at) = r.first_error_at_s {
                println!("{:12}      at: {:.1}s", "", at);
            }
        }
    }

    println!("{}", "=".repeat(72));
    println!(" Result:");
    if overall_ok {
        println!(
            "- No obvious computation errors or validation failures were observed in the current test window."
        );
    } else {
        println!("- At least one precision mode reported an error or validation failure.");
        std::process::exit(1);
    }
    println!("{}", "=".repeat(72));
}

#[cfg(feature = "cuda")]
fn main() {
    let args = Args::parse();

    let matrix_sizes = match parse_int_list(&args.matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid matrix sizes argument: {}", err);
            std::process::exit(2);
        }
    };
    let fp64_matrix_sizes = match parse_int_list(&args.fp64_matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid fp64 matrix sizes argument: {}", err);
            std::process::exit(2);
        }
    };

    let mut backend = match cuda_backend::CudaBackend::new() {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("CUDA init failed: {}", err);
            std::process::exit(1);
        }
    };

    let info = backend.device_info();
    print_device_info(&info);

    let precisions = match parse_precision_list(&args.precisions) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid argument: {}", err);
            std::process::exit(2);
        }
    };

    let include_fp8 = !args.disable_fp8;
    let mut filtered = Vec::new();
    for spec in precisions {
        if spec.kind == PrecisionKind::FP8E4M3FN && !include_fp8 {
            println!("FP8 E4M3FN disabled by flag, skipping");
            continue;
        }
        filtered.push(spec);
    }
    if filtered.is_empty() {
        eprintln!("No runnable precision modes available");
        std::process::exit(1);
    }

    let mut results = Vec::new();
    let mut overall_passed = true;

    for (idx, spec) in filtered.into_iter().enumerate() {
        let current_sizes = if spec.kind == PrecisionKind::FP64 {
            &fp64_matrix_sizes
        } else {
            &matrix_sizes
        };

        println!("\n{}", "-".repeat(72));
        println!("Starting test {}", spec.name);
        if let Some(tf32) = spec.tf32_enabled {
            println!("  TF32 setting: {}", tf32);
        }
        println!("  Matrix sizes: {:?}", current_sizes);
        println!("  Duration: {:.1} s", args.duration);
        println!("  Warmup iterations: {}", args.warmup_iters);
        println!("  Burst iterations: {}", args.burst_iters);
        println!("  Validation interval: {:.1} s", args.validate_interval);
        println!("  Validation size: {}", args.validate_size);

        let res = run_stress_for_precision(
            &mut backend,
            spec,
            current_sizes,
            args.duration,
            args.warmup_iters,
            args.burst_iters,
            args.validate_interval,
            args.validate_size,
            args.transpose_prob,
            args.seed + (idx as u64) * 1000,
        );
        if res.first_error.is_some() && res.supported {
            overall_passed = false;
        }
        if res.first_error.is_some() {
            println!("  Result: {}", res.first_error.clone().unwrap_or_default());
        } else {
            println!("  Result: completed without detected error");
        }
        println!(
            "  Total: {} matmul operations, {:.2} TFLOPS",
            res.iterations, res.tflops
        );
        results.push(res);
    }

    print_summary(&results, &info);

    if !overall_passed {
        std::process::exit(1);
    }
}

#[cfg(not(feature = "cuda"))]
fn main() {
    let _ = Args::parse();
    eprintln!("CUDA support is disabled. Rebuild with --features cuda.");
    std::process::exit(1);
}
