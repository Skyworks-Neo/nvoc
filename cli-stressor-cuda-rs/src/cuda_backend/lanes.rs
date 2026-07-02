//! Stream / cuBLAS-handle lane selection for the multi-stream stress paths.

use std::sync::Arc;

use cudarc::cublas::CudaBlas;
use cudarc::driver::CudaStream;

use cli_stressor_cuda_rs::StreamMode;

use super::backend::CudaBackend;

impl CudaBackend {
    /// Number of concurrent lanes for the given stream mode (clamped to the
    /// three streams that [`CudaBackend`] allocates up-front).
    pub(super) fn lane_count(stream_mode: StreamMode) -> usize {
        stream_mode.stream_count().clamp(1, 3)
    }

    pub(super) fn stream_for_lane(&self, lane: usize) -> &Arc<CudaStream> {
        match lane {
            0 => &self.stream,
            1 => &self.aux_streams[0],
            _ => &self.aux_streams[1],
        }
    }

    pub(super) fn blas_for_lane(&self, lane: usize) -> &CudaBlas {
        match lane {
            0 => &self.blas,
            1 => &self.aux_blas[0],
            _ => &self.aux_blas[1],
        }
    }
}
