//! Device-side stress-buffer generation (opt-in via `--gpu-generate`).
//!
//! Instead of generating random data on the host and copying it H2D, a tiny
//! NVRTC kernel fills the device buffer directly with the same SplitMix64
//! stream the host helpers produce (`lib.rs::splitmix64`). This removes the
//! host RNG + PCIe upload from the critical path between bursts, giving more
//! wall-clock to actual stress work. Content differs from the host path
//! (kernel-local index arithmetic), which is fine: no validation path reads
//! stress buffers.

use std::sync::Arc;

use cudarc::driver::{CudaContext, CudaFunction, CudaModule};

use cudarc::nvrtc::compile_ptx;

use cli_stressor_rs::BackendError;

use super::backend::CudaBackend;

const GPU_FILL_SRC: &str = r#"
extern "C" __device__ unsigned long long splitmix64(unsigned long long* state) {
    *state += 0x9E3779B97F4A7C15ULL;
    unsigned long long z = *state;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
}

extern "C" __global__ void fill_random_bytes(unsigned char* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        buf[idx] = (unsigned char)(splitmix64(&state) >> 32);
    }
}

extern "C" __global__ void fill_random_f32(float* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        unsigned long long x = splitmix64(&state);
        buf[idx] = (float)((x >> 40) & 0xFFFFFFULL) * (1.0f / 16777216.0f);
    }
}

extern "C" __global__ void fill_random_i32(int* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        buf[idx] = (int)(splitmix64(&state) >> 32);
    }
}

// ---- FP8/FP4 operand generation for the cublasLt stress paths ----
// Mirrors the host codecs in src/lib.rs (bit-exact RN-even quantizers; keep
// in lockstep). Values are drawn from a uniform [-2, 2] distribution —
// bounded so the f16 C output cannot overflow, unlike raw bit patterns.

__device__ float d_fp8e4m3(unsigned char b) {
    unsigned int sign = ((unsigned int)(b & 0x80u)) << 24;
    unsigned int exp = (unsigned int)(b >> 3) & 0xFu;
    unsigned int man = (unsigned int)b & 0x7u;
    if (exp == 0u) {
        if (man == 0u) return __uint_as_float(sign);
        float v = (float)man * 0.001953125f; // man * 2^-9
        return (b & 0x80u) ? -v : v;
    }
    if (exp == 15u && man == 7u) return __uint_as_float(0x7FC00000u); // NaN
    unsigned int fbits = sign | ((exp - 7u + 127u) << 23) | (man << 20);
    return __uint_as_float(fbits);
}

__device__ unsigned char q_fp8e4m3(float v) {
    unsigned int bits = __float_as_uint(v);
    unsigned char sign = (unsigned char)((bits >> 24) & 0x80u);
    unsigned int ax = bits & 0x7FFFFFFFu;
    if (ax >= 0x7F800000u) return 0x7F;              // NaN/Inf -> NaN pattern
    if (ax > 0x43E00000u) return sign | 0x7Eu;       // > 448: saturate
    if (ax < 0x3C800000u) {                          // denormal band (< 2^-6)
        float scaled = __uint_as_float(ax) * 512.0f; // m = ax / 2^-9
        float t = truncf(scaled);
        float frac = scaled - t;
        float m = (frac > 0.5f) ? t + 1.0f
                                : ((frac < 0.5f || (((int)t) & 1) == 0) ? t : t + 1.0f);
        return sign | (unsigned char)m;              // m == 8 -> min normal 0x08
    }
    unsigned int exp8 = ((ax >> 23) & 0xFFu) + 7u - 127u;
    unsigned int man = ax & 0x7FFFFFu;
    unsigned int keep = man >> 20;
    unsigned int rest = man & 0xFFFFFu;
    unsigned int round = (rest >> 19) & 1u;
    unsigned int sticky = rest & 0x7FFFFu;
    unsigned int m3 = keep;
    if (round == 1u && (sticky != 0u || (keep & 1u) == 1u)) m3 += 1u;
    if (m3 >= 8u) { exp8 += 1u; m3 = 0u; }
    return sign | (unsigned char)((exp8 << 3) | m3);
}

__device__ unsigned char q_fp8e5m2(float v) {
    unsigned int bits = __float_as_uint(v);
    unsigned char sign = (unsigned char)((bits >> 24) & 0x80u);
    unsigned int ax = bits & 0x7FFFFFFFu;
    if (ax == 0x7F800000u) return sign | 0x7Cu;      // Inf -> Inf
    if (ax > 0x7F800000u) return 0x7Fu;              // NaN -> NaN pattern
    if (ax > 0x47600000u) return sign | 0x7Bu;       // > 57344: saturate
    if (ax < 0x38800000u) {                          // denormal band (< 2^-14)
        float scaled = __uint_as_float(ax) * 65536.0f; // m = ax / 2^-16
        float t = truncf(scaled);
        float frac = scaled - t;
        float m = (frac > 0.5f) ? t + 1.0f
                                : ((frac < 0.5f || (((int)t) & 1) == 0) ? t : t + 1.0f);
        return sign | (unsigned char)m;              // m == 4 -> min normal 0x04
    }
    unsigned int exp8 = ((ax >> 23) & 0xFFu) + 15u - 127u;
    unsigned int man = ax & 0x7FFFFFu;
    unsigned int keep = man >> 21;
    unsigned int rest = man & 0x1FFFFFu;
    unsigned int round = (rest >> 20) & 1u;
    unsigned int sticky = rest & 0xFFFFFu;
    unsigned int m2 = keep;
    if (round == 1u && (sticky != 0u || (keep & 1u) == 1u)) m2 += 1u;
    if (m2 >= 4u) { exp8 += 1u; m2 = 0u; }
    return sign | (unsigned char)((exp8 << 2) | m2);
}

__device__ unsigned int q_e2m1(float v) {
    // Thresholds sit on the RN-even ties: 0.25/0.75/1.25/1.75/2.5/3.5/5.0.
    float a = fabsf(v);
    unsigned int mag;
    if (a <= 0.25f) mag = 0u;
    else if (a < 0.75f) mag = 1u;
    else if (a <= 1.25f) mag = 2u;
    else if (a < 1.75f) mag = 3u;
    else if (a <= 2.5f) mag = 4u;
    else if (a < 3.5f) mag = 5u;
    else if (a <= 5.0f) mag = 6u;
    else mag = 7u;
    return mag | (signbit(v) ? 0x8u : 0u);
}

// One FP8 operand element per thread: uniform [-2, 2] -> RN-even E4M3/E5M2.
extern "C" __global__ void fill_fp8(unsigned char* buf, unsigned long long n,
                                    unsigned long long seed, unsigned int e5m2) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        unsigned long long x = splitmix64(&state);
        float v = ((float)((x >> 40) & 0xFFFFFFULL) * (1.0f / 16777216.0f) - 0.5f) * 4.0f;
        buf[idx] = e5m2 ? q_fp8e5m2(v) : q_fp8e4m3(v);
    }
}

// One FP4 scale-block per thread: draws block_len values, derives the block
// scale (NVFP4 = E4M3(max/6), MXFP4 = 2^ceil(log2(max/6)) UE8M0), quantizes
// to packed E2M1 nibbles (low nibble first) and writes the scale byte at
// scales[kb * cols + col] — the layout the sampled cross-check reads back.
extern "C" __global__ void fill_fp4(unsigned char* packed, unsigned char* scales,
                                    unsigned long long cols, unsigned long long k,
                                    unsigned long long kblocks, unsigned int block_len,
                                    unsigned int mxfp4, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    unsigned long long total = cols * kblocks;
    for (unsigned long long t = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; t < total; t += stride) {
        unsigned long long col = t / kblocks;
        unsigned long long kb = t % kblocks;
        float vals[32];
        float maxabs = 0.0f;
        unsigned long long base = col * k + kb * (unsigned long long)block_len;
        for (unsigned int i = 0; i < block_len; ++i) {
            unsigned long long state = seed + (base + i) * 0x9E3779B97F4A7C15ULL;
            unsigned long long x = splitmix64(&state);
            float v = ((float)((x >> 40) & 0xFFFFFFULL) * (1.0f / 16777216.0f) - 0.5f) * 4.0f;
            vals[i] = v;
            float a = fabsf(v);
            if (a > maxabs) maxabs = a;
        }
        unsigned char scale_byte = 0;
        float scale_f = 0.0f;
        if (maxabs > 0.0f) {
            if (mxfp4) {
                float ratio = maxabs / 6.0f;
                int e = (int)ceilf(log2f(ratio));
                if (e < -127) e = -127;
                if (e > 127) e = 127;
                scale_byte = (unsigned char)(e + 127);
                scale_f = exp2f((float)e);
            } else {
                scale_byte = q_fp8e4m3(maxabs / 6.0f);
                scale_f = d_fp8e4m3(scale_byte);
            }
        }
        scales[kb * cols + col] = scale_byte;
        unsigned long long byte_base = base / 2ULL;
        for (unsigned int i = 0; i < block_len; i += 2) {
            unsigned int lo = (scale_f > 0.0f) ? q_e2m1(vals[i] / scale_f) : 0u;
            unsigned int hi = (scale_f > 0.0f) ? q_e2m1(vals[i + 1] / scale_f) : 0u;
            packed[byte_base + i / 2] = (unsigned char)(lo | (hi << 4));
        }
    }
}
"#;

pub(super) struct GpuFillKernels {
    pub(super) _module: Arc<CudaModule>,
    pub(super) bytes_fn: CudaFunction,
    pub(super) f32_fn: CudaFunction,
    pub(super) i32_fn: CudaFunction,
    /// FP8 operand fill (variant arg: 0 = E4M3FN, 1 = E5M2).
    pub(super) fp8_fn: CudaFunction,
    /// FP4 operand fill (packed nibbles + block scales in one pass).
    pub(super) fp4_fn: CudaFunction,
}

pub(super) fn build_gpu_fill_kernels(
    ctx: &Arc<CudaContext>,
) -> Result<GpuFillKernels, BackendError> {
    let ptx = compile_ptx(GPU_FILL_SRC).map_err(|err| BackendError::Other(err.to_string()))?;
    let module = ctx
        .load_module(ptx)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let bytes_fn = module
        .load_function("fill_random_bytes")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let f32_fn = module
        .load_function("fill_random_f32")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let i32_fn = module
        .load_function("fill_random_i32")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let fp8_fn = module
        .load_function("fill_fp8")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let fp4_fn = module
        .load_function("fill_fp4")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    Ok(GpuFillKernels {
        _module: module,
        bytes_fn,
        f32_fn,
        i32_fn,
        fp8_fn,
        fp4_fn,
    })
}

impl CudaBackend {
    /// Opt in to device-side stress-buffer generation. Builds the NVRTC fill
    /// kernels eagerly so per-burst paths stay allocation-free.
    pub fn enable_gpu_generate(&mut self) -> Result<(), BackendError> {
        if self.gpu_fill.is_some() {
            self.gpu_generate = true;
            return Ok(());
        }
        let kernels = build_gpu_fill_kernels(&self._ctx)?;
        self.gpu_fill = Some(kernels);
        self.gpu_generate = true;
        Ok(())
    }

    pub(super) fn gpu_generate_enabled(&self) -> bool {
        self.gpu_generate
    }

    /// Smoke-test the FP8/FP4 fill kernels on the live device: fill small
    /// buffers, download, and validate the operands through the host codecs
    /// (finite values inside the quantized [-2, 2] band; FP4 scales consistent
    /// with their blocks). Runs on any SM — the fill kernels are not
    /// GEMM-dtype-gated — so the device quantizer can be bit-aligned against
    /// the host one on hardware without FP8/FP4 tensor cores.
    pub fn gpu_fill_fp8_fp4_smoke_test(&mut self) -> Result<String, BackendError> {
        use cudarc::driver::{LaunchConfig, PushKernelArg};

        if self.gpu_fill.is_none() {
            self.enable_gpu_generate()?;
        }
        let kernels = self.gpu_fill.as_ref().expect("built above");
        let stream = self.stream.clone();
        let mut report = String::new();

        // FP8: one 256-element buffer per variant; every decoded value must be
        // finite and within the quantized [-2, 2] input band.
        let n = 256usize;
        for e5 in [0u32, 1u32] {
            let buf = stream
                .alloc_zeros::<u8>(n)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let cfg = LaunchConfig::for_num_elems(n as u32);
            unsafe {
                stream
                    .launch_builder(&kernels.fp8_fn)
                    .arg(&buf)
                    .arg(&(n as u64))
                    .arg(&0xC0FFEEu64)
                    .arg(&e5)
                    .launch(cfg)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let host: Vec<u8> = stream
                .clone_dtoh(&buf)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let mut nonzero = 0usize;
            let mut max_abs = 0f32;
            for b in &host {
                let v = if e5 == 1 {
                    cli_stressor_rs::fp8_e5m2_decode(*b)
                } else {
                    cli_stressor_rs::fp8_e4m3_decode(*b)
                };
                if !v.is_finite() || v.abs() > 2.0 {
                    return Err(BackendError::Other(format!(
                        "fp8 fill produced out-of-band value {v} (byte {b:#04x}, e5={e5})"
                    )));
                }
                if v != 0.0 {
                    nonzero += 1;
                }
                max_abs = max_abs.max(v.abs());
            }
            report.push_str(&format!(
                "fp8(e5={e5}): nonzero={nonzero}/{n} max_abs={max_abs:.3}; "
            ));
        }

        // FP4: a few NVFP4 and MXFP4 blocks; every decoded element must sit
        // inside ±6·scale. The launch mirrors the real call sites' invariant
        // k == kblocks · block_len (the kernel's block bases stay disjoint).
        let cols = 4usize;
        let kblocks = 2usize;
        let block_len = 16usize;
        let k = kblocks * block_len;
        let packed_len = cols * k / 2;
        let scales_len = kblocks * cols;
        for mxfp4 in [0u32, 1u32] {
            let packed = stream
                .alloc_zeros::<u8>(packed_len)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let scales = stream
                .alloc_zeros::<u8>(scales_len)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let cfg = LaunchConfig::for_num_elems((cols * kblocks) as u32);
            unsafe {
                stream
                    .launch_builder(&kernels.fp4_fn)
                    .arg(&packed)
                    .arg(&scales)
                    .arg(&(cols as u64))
                    .arg(&(k as u64))
                    .arg(&(kblocks as u64))
                    .arg(&(block_len as u32))
                    .arg(&mxfp4)
                    .arg(&0xC0FFEEu64)
                    .launch(cfg)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let packed_host: Vec<u8> = stream
                .clone_dtoh(&packed)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let scales_host: Vec<u8> = stream
                .clone_dtoh(&scales)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            for kb in 0..kblocks {
                for col in 0..cols {
                    let s = scales_host[kb * cols + col];
                    let scale_f = if mxfp4 == 1 {
                        2f32.powi(s as i32 - 127)
                    } else {
                        cli_stressor_rs::fp8_e4m3_decode(s)
                    };
                    for i in 0..block_len {
                        let lin = col * k + kb * block_len + i;
                        let nib = cli_stressor_rs::unpack4(&packed_host, lin);
                        let v = cli_stressor_rs::fp4_e2m1_decode(nib) * scale_f;
                        if !v.is_finite() {
                            return Err(BackendError::Other(format!(
                                "fp4 fill produced non-finite value (mxfp4={mxfp4}, block {kb}/{col})"
                            )));
                        }
                        if v.abs() > 6.0 * scale_f + f32::EPSILON {
                            return Err(BackendError::Other(format!(
                                "fp4 fill produced out-of-band value {v} > 6·{scale_f}"
                            )));
                        }
                    }
                }
            }
            report.push_str(&format!(
                "fp4(mxfp4={mxfp4}): {} blocks validated; ",
                kblocks * cols
            ));
        }
        Ok(report)
    }
}
