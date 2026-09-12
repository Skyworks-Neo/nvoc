//! Memory-bandwidth and element-wise stress paths (memcpy / memset / sgeam /
//! reduction), all driven through cuBLAS or the driver memory primitives.
//!
//! The copy/fill buffers are pattern-seeded device-side and verified in-stream
//! by the ride-on-load detectors: the checked bytes are the bytes the stress
//! op is hammering, so verification adds bandwidth load instead of replacing it.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::time::Instant;

use cudarc::cublas::{Asum, AsumConfig, sys as cublas_sys};
use cudarc::driver::{DevicePtr, DevicePtrMut};

use cli_stressor_cuda_rs::{
    BackendError, PrecisionKind, PrecisionSpec, StreamMode, VerifyConfig, make_random_host_matrix,
};

use super::backend::CudaBackend;
use super::verify_kernels::lane_seed;

impl CudaBackend {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_memcpy_path(
        &mut self,
        spec: &PrecisionSpec,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
        verify: VerifyConfig,
    ) -> Result<f64, BackendError> {
        let elem_size = match spec.kind {
            PrecisionKind::BF16 | PrecisionKind::FP16 => 2usize,
            PrecisionKind::FP32 | PrecisionKind::TF32 => 4usize,
            PrecisionKind::FP64 => 8usize,
            PrecisionKind::FP8E4M3FN => 1usize,
            PrecisionKind::INT8 => 1usize,
            PrecisionKind::INT16 => 2usize,
            PrecisionKind::INT32 => 4usize,
        };
        // Word-rounded so the pattern compare can run over whole u32 words.
        let bytes = size * size * elem_size;
        let words = bytes.div_ceil(4);
        let lane_count = Self::lane_count(stream_mode);
        let mut srcs = Vec::with_capacity(lane_count);
        let mut dsts = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            srcs.push(
                stream
                    .alloc_zeros::<u32>(words)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            dsts.push(
                stream
                    .alloc_zeros::<u32>(words)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        // Deterministic device-side pattern instead of host random bytes:
        // same pseudo-random traffic, no host generation or H2D in setup.
        if let Some(engine) = &self.verify
            && verify.enabled
        {
            for (lane, src) in srcs.iter().enumerate() {
                engine.fill(self.stream_for_lane(lane), src, lane_seed(seed, lane))?;
            }
        }
        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                stream
                    .memcpy_dtod(&srcs[lane], &mut dsts[lane])
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        let op_start = Instant::now();
        for _ in 0..burst_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                stream
                    .memcpy_dtod(&srcs[lane], &mut dsts[lane])
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        // Verify the actual stress buffers (dst) against the same oracle. The
        // full read of dst rides the same streams; errors surface on the next
        // dispatch-loop drain.
        let streams: Vec<_> = (0..lane_count)
            .map(|l| self.stream_for_lane(l).clone())
            .collect();
        if verify.enabled
            && verify.due(verify.memcpy_every, seed)
            && let Some(engine) = self.verify.as_mut()
        {
            for (lane, stream) in streams.iter().enumerate() {
                engine.reset_lane_report(stream, lane)?;
            }
            let mut expected_done = Vec::with_capacity(streams.len());
            for (lane, stream) in streams.iter().enumerate() {
                expected_done.push(engine.compare(
                    stream,
                    &dsts[lane],
                    lane_seed(seed, lane),
                    lane,
                )?);
            }
            for stream in &streams {
                stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
            for (lane, stream) in streams.iter().enumerate() {
                let report = engine.lane_report(stream, lane)?;
                engine.absorb(&report, words as u64, expected_done[lane], "memcpy verify");
            }
        } else {
            for lane in 0..lane_count {
                self.stream_for_lane(lane)
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn run_memset_path(
        &mut self,
        spec: &PrecisionSpec,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
        verify: VerifyConfig,
    ) -> Result<f64, BackendError> {
        let elem_size = match spec.kind {
            PrecisionKind::BF16 | PrecisionKind::FP16 => 2usize,
            PrecisionKind::FP32 | PrecisionKind::TF32 => 4usize,
            PrecisionKind::FP64 => 8usize,
            PrecisionKind::FP8E4M3FN => 1usize,
            PrecisionKind::INT8 => 1usize,
            PrecisionKind::INT16 => 2usize,
            PrecisionKind::INT32 => 4usize,
        };
        let bytes = size * size * elem_size;
        let words = bytes.div_ceil(4);
        let lane_count = Self::lane_count(stream_mode);
        // Address-walk windows: when the slab is active, each lane works on
        // its own sliding view (different physical pages every op) instead
        // of a dedicated buffer parked on the same pages forever.
        let mut slab_windows: Vec<Option<(usize, usize)>> = Vec::with_capacity(lane_count);
        let mut bufs: Vec<Option<cudarc::driver::CudaSlice<u32>>> = Vec::with_capacity(lane_count);
        if let Some(engine) = &self.verify {
            for _ in 0..lane_count {
                slab_windows.push(engine.take_slab_window(words));
                bufs.push(None);
            }
        }
        for lane in 0..lane_count {
            if slab_windows[lane].is_none() {
                let stream = self.stream_for_lane(lane);
                bufs[lane] = Some(
                    stream
                        .alloc_zeros::<u32>(words)
                        .map_err(|err| BackendError::Other(err.to_string()))?,
                );
            }
        }
        let streams: Vec<_> = (0..lane_count)
            .map(|l| self.stream_for_lane(l).clone())
            .collect();
        for _ in 0..warmup_iters {
            for (lane, stream) in streams.iter().enumerate() {
                match bufs[lane].as_mut() {
                    Some(buf) => {
                        stream
                            .memset_zeros(buf)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                    }
                    None => {
                        if let (Some(engine), Some((off, w))) =
                            (self.verify.as_ref(), slab_windows[lane])
                        {
                            engine.with_slab_window_mut(off, w, |view| {
                                stream
                                    .memset_zeros(view)
                                    .map_err(|err| BackendError::Other(err.to_string()))
                            })?;
                        }
                    }
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        // Every Nth burst iteration is replaced by pattern fill + compare on
        // the same buffer (write + read = the same bandwidth class of load);
        // the remaining iterations stay pure memset.
        let verify_on = self.verify.is_some() && verify.enabled && verify.memset_every > 0;
        if let (true, Some(engine)) = (verify_on, self.verify.as_mut()) {
            for (lane, stream) in streams.iter().enumerate() {
                engine.reset_lane_report(stream, lane)?;
            }
        }
        let mut expected_done = vec![0u32; lane_count];
        let op_start = Instant::now();
        for iter in 0..burst_iters {
            let verify_iter = verify_on
                && (iter as u64) % verify.memset_every as u64 == verify.memset_every as u64 - 1;
            if verify_iter {
                let engine = self.verify.as_ref().expect("verify_on implies engine");
                // Pattern rotation (TM5-style): hash -> all-ones -> zeros ->
                // checkerboard, cycling. Different patterns exercise different
                // write-disturb directions (all error captures so far are 1->0).
                let pattern_mode = ((iter as u64 / verify.memset_every as u64) % 4) as u32;
                for lane in 0..lane_count {
                    match bufs[lane].as_mut() {
                        Some(buf) => {
                            use cudarc::driver::{DevicePtr, DevicePtrMut};
                            let stream_ref = &streams[lane];
                            let (dst_ptr, _) = buf.device_ptr_mut(stream_ref);
                            engine.fill_mode(
                                &streams[lane],
                                dst_ptr,
                                words as u64,
                                lane_seed(seed, lane),
                                pattern_mode,
                            )?;
                            let (src_ptr, _) = buf.device_ptr(stream_ref);
                            expected_done[lane] += engine.compare_mode(
                                &streams[lane],
                                src_ptr,
                                words as u64,
                                lane_seed(seed, lane),
                                pattern_mode,
                                lane,
                            )?;
                        }
                        None => {
                            if let (Some((off, w)), true) =
                                (slab_windows[lane], engine.slab_active())
                            {
                                let stream_ref = &streams[lane];
                                let mut fill_res = Ok(());
                                engine.with_slab_window_mut(off, w, |view| {
                                    use cudarc::driver::DevicePtrMut;
                                    let (dst_ptr, _) = view.device_ptr_mut(stream_ref);
                                    fill_res = engine.fill_mode(
                                        &streams[lane],
                                        dst_ptr,
                                        words as u64,
                                        lane_seed(seed, lane),
                                        pattern_mode,
                                    );
                                });
                                fill_res?;
                                engine.with_slab_window(off, w, |view| {
                                    use cudarc::driver::DevicePtr;
                                    let (src_ptr, _) = view.device_ptr(stream_ref);
                                    expected_done[lane] += engine
                                        .compare_mode(
                                            &streams[lane],
                                            src_ptr,
                                            words as u64,
                                            lane_seed(seed, lane),
                                            pattern_mode,
                                            lane,
                                        )
                                        .map_err(|err| BackendError::Other(err.to_string()))?;
                                    Ok::<(), BackendError>(())
                                })?;
                            }
                        }
                    }
                }
            } else {
                for (lane, buf) in bufs.iter_mut().enumerate().take(lane_count) {
                    match buf {
                        Some(buf) => {
                            streams[lane]
                                .memset_zeros(buf)
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                        None => {
                            if let (Some(engine), Some((off, w))) =
                                (self.verify.as_ref(), slab_windows[lane])
                            {
                                let stream_ref = &streams[lane];
                                engine.with_slab_window_mut(off, w, |view| {
                                    use cudarc::driver::DevicePtrMut;
                                    let (dst_ptr, _) = view.device_ptr_mut(stream_ref);
                                    engine.fill_mode(
                                        &streams[lane],
                                        dst_ptr,
                                        words as u64,
                                        lane_seed(seed, lane),
                                        2,
                                    )
                                })?;
                            }
                        }
                    }
                }
            }
        }
        for stream in &streams {
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        if let (true, Some(engine)) = (verify_on, self.verify.as_ref()) {
            for lane in 0..lane_count {
                let report = engine.lane_report(&streams[lane], lane)?;
                engine.absorb(&report, words as u64, expected_done[lane], "memset verify");
            }
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    pub(super) fn run_sgeam_path(
        &self,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        transpose: bool,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut a_devs = Vec::with_capacity(lane_count);
        let mut b_devs = Vec::with_capacity(lane_count);
        let mut c_devs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            let a_host = make_random_host_matrix(size, rng.random::<u64>());
            let b_host = make_random_host_matrix(size, rng.random::<u64>());
            a_devs.push(
                stream
                    .clone_htod(&a_host.data)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            b_devs.push(
                stream
                    .clone_htod(&b_host.data)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            c_devs.push(
                stream
                    .alloc_zeros::<f32>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let n = size as i32;
        let transa = if transpose {
            cublas_sys::cublasOperation_t::CUBLAS_OP_T
        } else {
            cublas_sys::cublasOperation_t::CUBLAS_OP_N
        };
        let transb = cublas_sys::cublasOperation_t::CUBLAS_OP_N;
        let alpha = 1.0f32;
        let beta = if transpose { 0.0f32 } else { 1.0f32 };

        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                let blas = self.blas_for_lane(lane);
                let (a_ptr, _a_sync) = a_devs[lane].device_ptr(stream);
                let (b_ptr, _b_sync) = b_devs[lane].device_ptr(stream);
                let (c_ptr, _c_sync) = c_devs[lane].device_ptr_mut(stream);
                let status = unsafe {
                    cublas_sys::cublasSgeam(
                        *blas.handle(),
                        transa,
                        transb,
                        n,
                        n,
                        &alpha as *const f32,
                        a_ptr as *const f32,
                        n,
                        &beta as *const f32,
                        b_ptr as *const f32,
                        n,
                        c_ptr as *mut f32,
                        n,
                    )
                };
                if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                    return Err(BackendError::Other(format!(
                        "cublasSgeam failed: {:?}",
                        status
                    )));
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        let op_start = Instant::now();
        for _ in 0..burst_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                let blas = self.blas_for_lane(lane);
                let (a_ptr, _a_sync) = a_devs[lane].device_ptr(stream);
                let (b_ptr, _b_sync) = b_devs[lane].device_ptr(stream);
                let (c_ptr, _c_sync) = c_devs[lane].device_ptr_mut(stream);
                let status = unsafe {
                    cublas_sys::cublasSgeam(
                        *blas.handle(),
                        transa,
                        transb,
                        n,
                        n,
                        &alpha as *const f32,
                        a_ptr as *const f32,
                        n,
                        &beta as *const f32,
                        b_ptr as *const f32,
                        n,
                        c_ptr as *mut f32,
                        n,
                    )
                };
                if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                    return Err(BackendError::Other(format!(
                        "cublasSgeam failed: {:?}",
                        status
                    )));
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    pub(super) fn run_reduction_path(
        &self,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut xs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            let x_host = make_random_host_matrix(size, rng.random::<u64>());
            xs.push(
                stream
                    .clone_htod(&x_host.data)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let cfg = AsumConfig {
            n: (size * size) as i32,
            incx: 1,
        };
        let mut outs = vec![0.0f32; lane_count];
        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let blas = self.blas_for_lane(lane);
                unsafe {
                    blas.asum(cfg, &xs[lane], &mut outs[lane])
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        let op_start = Instant::now();
        for _ in 0..burst_iters {
            for lane in 0..lane_count {
                let blas = self.blas_for_lane(lane);
                unsafe {
                    blas.asum(cfg, &xs[lane], &mut outs[lane])
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }
}
