//! NVENC → NVDEC video-engine stress backend.
//!
//! A closed encoder/decoder loop over the driver-shipped video libraries
//! (no ffmpeg or other external binaries — everything is runtime-loaded via
//! `libloading`, mirroring the NVML loader pattern in `core/src/nvml.rs`):
//!
//! 1. Own CUDA context via the primary context (`cuInit` → `cuDeviceGet` →
//!    `cuDevicePrimaryCtxRetain` → `cuCtxSetCurrent`), independent of cudarc.
//! 2. Synthetic NV12 frames whose luma comes from the same deterministic
//!    [`crate::verify::vhash32`] oracle the CUDA detectors use (`Y[y][x] =
//!    vhash32(x*2 + y) & 0xFF`, UV = 128).
//! 3. NVENC (H264 Main, NV12 input, gopLength=1 → all-I stream, SPS/PPS
//!    repeated on every frame) encodes each frame into a bitstream ring.
//! 4. nvcuvid parses the ring's bitstream; the parser callbacks drive
//!    `cuvidDecodePicture`, and every displayed frame is mapped, copied
//!    D2H and checksummed.
//!
//! Validation (`--video-validate`, default on): H.264 is lossy for 4:2:0
//! Main profile, so decoded pixels cannot be compared against the vhash32
//! source pattern directly. The validator instead pins a stable baseline:
//! encoding runs CONSTQP with identical input every frame (deterministic
//! output), the first successfully decoded frame fixes the expected frame
//! CRC, and every later decode of the same pattern must reproduce it. A
//! mismatch is counted as `crc_mismatches` — that catches NVENC bitstream
//! corruption, NVDEC errors and D2H datapath flips, which is exactly the
//! failure class a video stressor hunts for.

use crate::video_backend::ffi as nv;
use libloading::Library;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

pub mod ffi;

// ---------------------------------------------------------------------------
// Public configuration / result types
// ---------------------------------------------------------------------------

/// Configuration for the video-engine sidecar (`--video-*` CLI flags).
#[derive(Debug, Clone)]
pub struct VideoConfig {
    /// CUDA device number. Interpreted as a raw CUDA ordinal when
    /// [`VideoConfig::device_is_pci_sorted`] is false (mixed mode, where the
    /// CUDA backend already resolved `--gpu-index`/`--gpu-uuid`/`--pci-bus`),
    /// and as a PCI-bus-sorted position when true (`--video-only`, resolved
    /// through the driver attributes so the selection rule matches the rest
    /// of the binary).
    pub device_index: u32,
    /// `--video-only` resolves `device_index` through the PCI-bus-sorted
    /// enumeration (see above).
    pub device_is_pci_sorted: bool,
    /// Synthetic frame width in pixels (rounded down to even).
    pub width: u32,
    /// Synthetic frame height in pixels (rounded down to even).
    pub height: u32,
    /// Decode-side CRC validation (`--video-validate`). Forces the
    /// deterministic CONSTQP encode path.
    pub validate: bool,
    /// H264 bitrate in bits/s for CBR mode (`--video-bitrate`). Only used
    /// when validation is off; validation pins CONSTQP for determinism.
    pub bitrate_bps: Option<u32>,
    /// NVENC preset `p1`..`p7` (`--video-preset`).
    pub preset: String,
    /// Nominal frame rate numerator reported to the encoder (does not pace
    /// the loop; the stressor runs as fast as NVENC/NVDEC allow).
    pub fps: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            device_index: 0,
            device_is_pci_sorted: true,
            width: 1920,
            height: 1080,
            validate: true,
            bitrate_bps: None,
            preset: "p3".to_string(),
            fps: 60,
        }
    }
}

impl VideoConfig {
    /// Validate dimensions for the NVENC/NVDEC pipeline: NV12 chroma and
    /// H.264 macroblocks want even edges, and both engines floor out well
    /// above 64 px. Returns the (possibly rounded) `(width, height)`.
    pub fn resolved_dims(&self) -> Result<(u32, u32), String> {
        let width = self.width - (self.width % 2);
        let height = self.height - (self.height % 2);
        if width < 64 || height < 64 {
            return Err(format!(
                "video frame {}x{} too small (minimum 64x64 after even-rounding)",
                width, height
            ));
        }
        Ok((width, height))
    }

    /// Parse the `--video-preset` value (`p1`..`p7`, case-insensitive).
    pub fn parse_preset(raw: &str) -> Result<String, String> {
        let key = raw.trim().to_ascii_lowercase();
        if nv::preset_guid(&key).is_some() {
            Ok(key)
        } else {
            Err(format!(
                "unsupported video preset: {raw} (expected p1..p7, 1 = fastest, 7 = best quality)"
            ))
        }
    }
}

/// Counters for one video-engine run (serialized into the verdict JSON's
/// `"video"` segment).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct VideoStats {
    pub encoded_frames: u64,
    pub decoded_frames: u64,
    pub crc_mismatches: u64,
    /// Encoded H264 bits per second (8 * bytes / elapsed).
    pub encode_mbps: f64,
    /// Decoded (mapped+readback) frames per second.
    pub decode_fps: f64,
    pub elapsed_s: f64,
}

/// The NVENC → NVDEC video-engine sidecar. Mirrors the
/// `VulkanGraphicsEngine` lifecycle: `new` builds the pipeline (or reports
/// why it cannot), `start` moves it to a worker thread, `stop` joins it and
/// returns the counters.
pub struct VideoEngine {
    dims: (u32, u32),
    stop_flag: Arc<AtomicBool>,
    error_flag: Arc<AtomicBool>,
    counters: Arc<VideoCounters>,
    started_at: Option<Instant>,
    state: Option<VideoPipeline>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

/// Atomic counters shared between the worker thread and [`VideoEngine`].
struct VideoCounters {
    encoded_frames: AtomicU64,
    encoded_bytes: AtomicU64,
    decoded_frames: AtomicU64,
    crc_mismatches: AtomicU64,
    /// Frames dropped before reaching the parser (ring overflow).
    ring_dropped: AtomicU64,
    /// Displayed frames that could not be mapped/read back.
    decode_missed: AtomicU64,
}

impl VideoCounters {
    fn new() -> Self {
        Self {
            encoded_frames: AtomicU64::new(0),
            encoded_bytes: AtomicU64::new(0),
            decoded_frames: AtomicU64::new(0),
            crc_mismatches: AtomicU64::new(0),
            ring_dropped: AtomicU64::new(0),
            decode_missed: AtomicU64::new(0),
        }
    }
}

impl VideoEngine {
    /// Build the full NVENC/NVDEC pipeline synchronously. Returns `Err`
    /// with a loader-style detail string when the driver video libraries
    /// are missing, the driver's NVENC API is older than the bound SDK
    /// 13.1 surface, or any initialization step fails — callers degrade
    /// with a warning instead of treating this as fatal.
    pub fn new(config: VideoConfig) -> Result<Self, String> {
        let preset = VideoConfig::parse_preset(&config.preset)?;
        let config = VideoConfig { preset, ..config };
        let dims = config.resolved_dims()?;
        let pipeline = VideoPipeline::new(&config, dims)?;
        Ok(Self {
            dims,
            stop_flag: Arc::new(AtomicBool::new(false)),
            error_flag: Arc::new(AtomicBool::new(false)),
            counters: Arc::new(VideoCounters::new()),
            started_at: None,
            state: Some(pipeline),
            thread_handle: None,
        })
    }

    /// Frame dimensions the pipeline was built for.
    pub fn dims(&self) -> (u32, u32) {
        self.dims
    }

    /// Clone of the worker-error flag so callers can observe a crashed
    /// pipeline without owning the engine.
    pub fn get_error_flag_arc(&self) -> Arc<AtomicBool> {
        self.error_flag.clone()
    }

    /// Spawn the worker thread. The pipeline keeps running until
    /// [`VideoEngine::stop`] (mixed mode).
    pub fn start(&mut self) -> Result<(), String> {
        self.start_inner(None)
    }

    /// Run the pipeline inline for `duration_s` and return the final stats
    /// (`--video-only` mode).
    pub fn run_for_duration(&mut self, duration_s: f64) -> Result<VideoStats, String> {
        self.start_inner(Some(duration_s))?;
        self.join_worker();
        Ok(self.snapshot_stats())
    }

    fn start_inner(&mut self, duration_s: Option<f64>) -> Result<(), String> {
        if self.thread_handle.is_some() {
            return Err("video engine is already running".to_string());
        }
        let Some(state) = self.state.take() else {
            return Err("video engine pipeline already consumed".to_string());
        };
        self.stop_flag.store(false, Ordering::SeqCst);
        self.error_flag.store(false, Ordering::SeqCst);
        self.started_at = Some(Instant::now());
        let stop = self.stop_flag.clone();
        let error = self.error_flag.clone();
        let counters = self.counters.clone();
        let deadline = duration_s.map(|d| std::time::Duration::from_secs_f64(d.max(0.0)));
        self.thread_handle = Some(
            std::thread::Builder::new()
                .name("nvoc-video-engine".to_string())
                .spawn(move || pipeline_loop(state, stop, error, counters, deadline))
                .map_err(|err| format!("failed to spawn video engine thread: {err}"))?,
        );
        Ok(())
    }

    /// Signal the worker to finish, join it, and return the final stats.
    pub fn stop(&mut self) -> Result<VideoStats, String> {
        if self.thread_handle.is_none() {
            return Ok(self.snapshot_stats());
        }
        self.stop_flag.store(true, Ordering::SeqCst);
        self.join_worker();
        if self.error_flag.load(Ordering::SeqCst) {
            return Err("video engine worker reported an error".to_string());
        }
        Ok(self.snapshot_stats())
    }

    fn join_worker(&mut self) {
        if let Some(handle) = self.thread_handle.take()
            && handle.join().is_err()
        {
            println!("[Video] worker thread panicked");
            self.error_flag.store(true, Ordering::SeqCst);
        }
    }

    /// Current counters as a [`VideoStats`] snapshot (elapsed measured from
    /// `start`; zero-width metrics report 0 rather than NaN).
    pub fn stats(&self) -> VideoStats {
        self.snapshot_stats()
    }

    fn snapshot_stats(&self) -> VideoStats {
        let elapsed_s = self
            .started_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let encoded_frames = self.counters.encoded_frames.load(Ordering::SeqCst);
        let encoded_bytes = self.counters.encoded_bytes.load(Ordering::SeqCst);
        let decoded_frames = self.counters.decoded_frames.load(Ordering::SeqCst);
        VideoStats {
            encoded_frames,
            decoded_frames,
            crc_mismatches: self.counters.crc_mismatches.load(Ordering::SeqCst),
            encode_mbps: if elapsed_s > 0.0 {
                encoded_bytes as f64 * 8.0 / elapsed_s / 1e6
            } else {
                0.0
            },
            decode_fps: if elapsed_s > 0.0 {
                decoded_frames as f64 / elapsed_s
            } else {
                0.0
            },
            elapsed_s,
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime-loaded driver libraries
// ---------------------------------------------------------------------------

/// `libloading::Library` is not `Send` on all platforms; the driver modules
/// are process-global once loaded (same reasoning as the NVML RawLib in
/// core/src/nvml.rs), so moving them to the worker thread is sound.
struct SendLibrary(Library);
unsafe impl Send for SendLibrary {}
unsafe impl Sync for SendLibrary {}

/// Bound entry points of `nvEncodeAPI64.dll` / `libnvidia-encode.so.1`.
struct NvencLib {
    _lib: SendLibrary,
    create_instance: nv::TNvEncodeAPICreateInstance,
    get_max_supported_version: nv::TNvEncodeAPIGetMaxSupportedVersion,
}

impl NvencLib {
    fn load() -> Result<Self, String> {
        let lib = load_library(nv::NVENC_LIB, "NvEncodeAPI")?;
        // SAFETY: the symbols below are the two documented NVENC entry
        // points; their signatures match nvEncodeAPI.h exactly. The fn
        // pointers are copied out before the Library handle moves into the
        // struct (a Symbol borrows the Library).
        unsafe {
            let create_instance = *lib
                .0
                .get::<nv::TNvEncodeAPICreateInstance>(b"NvEncodeAPICreateInstance\0")
                .map_err(symbol_error("NvEncodeAPICreateInstance"))?;
            let get_max_supported_version = *lib
                .0
                .get::<nv::TNvEncodeAPIGetMaxSupportedVersion>(
                    b"NvEncodeAPIGetMaxSupportedVersion\0",
                )
                .map_err(symbol_error("NvEncodeAPIGetMaxSupportedVersion"))?;
            Ok(Self {
                _lib: lib,
                create_instance,
                get_max_supported_version,
            })
        }
    }
}

/// Bound cuvid (NVDEC) functions of `nvcuvid.dll` / `libnvcuvid.so.1`.
struct CuvidLib {
    _lib: SendLibrary,
    get_decoder_caps: nv::TcuvidGetDecoderCaps,
    create_decoder: nv::TcuvidCreateDecoder,
    destroy_decoder: nv::TcuvidDestroyDecoder,
    decode_picture: nv::TcuvidDecodePicture,
    map_video_frame: nv::TcuvidMapVideoFrame64,
    unmap_video_frame: nv::TcuvidUnmapVideoFrame64,
    create_video_parser: nv::TcuvidCreateVideoParser,
    parse_video_data: nv::TcuvidParseVideoData,
    destroy_video_parser: nv::TcuvidDestroyVideoParser,
}

impl CuvidLib {
    fn load() -> Result<Self, String> {
        let lib = load_library(nv::NVCUVID_LIB, "nvcuvid")?;
        // SAFETY: each symbol is the documented dynlink entry point; the
        // typedefs mirror dynlink_nvcuvid.h / dynlink_cuviddec.h (the 64-bit
        // dev-pointer Map/Unmap variants the ffnvcodec loader binds). The fn
        // pointers are copied out before the Library handle moves.
        unsafe {
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    *lib.0
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(symbol_error($name))?
                };
            }
            let get_decoder_caps = sym!("cuvidGetDecoderCaps", nv::TcuvidGetDecoderCaps);
            let create_decoder = sym!("cuvidCreateDecoder", nv::TcuvidCreateDecoder);
            let destroy_decoder = sym!("cuvidDestroyDecoder", nv::TcuvidDestroyDecoder);
            let decode_picture = sym!("cuvidDecodePicture", nv::TcuvidDecodePicture);
            let map_video_frame = sym!("cuvidMapVideoFrame64", nv::TcuvidMapVideoFrame64);
            let unmap_video_frame = sym!("cuvidUnmapVideoFrame64", nv::TcuvidUnmapVideoFrame64);
            let create_video_parser = sym!("cuvidCreateVideoParser", nv::TcuvidCreateVideoParser);
            let parse_video_data = sym!("cuvidParseVideoData", nv::TcuvidParseVideoData);
            let destroy_video_parser =
                sym!("cuvidDestroyVideoParser", nv::TcuvidDestroyVideoParser);
            Ok(Self {
                _lib: lib,
                get_decoder_caps,
                create_decoder,
                destroy_decoder,
                decode_picture,
                map_video_frame,
                unmap_video_frame,
                create_video_parser,
                parse_video_data,
                destroy_video_parser,
            })
        }
    }
}

/// Bound CUDA driver subset of `nvcuda.dll` / `libcuda.so.1` (own primary
/// context; deliberately independent of cudarc).
struct CudaLib {
    _lib: SendLibrary,
    init: nv::TcuInit,
    device_get: nv::TcuDeviceGet,
    device_get_name: nv::TcuDeviceGetName,
    device_get_attribute: nv::TcuDeviceGetAttribute,
    device_get_count: nv::TcuDeviceGetCount,
    device_primary_ctx_retain: nv::TcuDevicePrimaryCtxRetain,
    device_primary_ctx_release: nv::TcuDevicePrimaryCtxRelease,
    ctx_set_current: nv::TcuCtxSetCurrent,
    memcpy_dtoh: nv::TcuMemcpyDtoH,
}

impl CudaLib {
    fn load() -> Result<Self, String> {
        let lib = load_library(nv::CUDA_LIB, "CUDA driver")?;
        // SAFETY: documented CUDA driver API entry points with signatures
        // matching dynlink_cuda.h. The fn pointers are copied out before the
        // Library handle moves.
        unsafe {
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    *lib.0
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(symbol_error($name))?
                };
            }
            let init = sym!("cuInit", nv::TcuInit);
            let device_get = sym!("cuDeviceGet", nv::TcuDeviceGet);
            let device_get_name = sym!("cuDeviceGetName", nv::TcuDeviceGetName);
            let device_get_attribute = sym!("cuDeviceGetAttribute", nv::TcuDeviceGetAttribute);
            let device_get_count = sym!("cuDeviceGetCount", nv::TcuDeviceGetCount);
            let device_primary_ctx_retain =
                sym!("cuDevicePrimaryCtxRetain", nv::TcuDevicePrimaryCtxRetain);
            let device_primary_ctx_release =
                sym!("cuDevicePrimaryCtxRelease", nv::TcuDevicePrimaryCtxRelease);
            let ctx_set_current = sym!("cuCtxSetCurrent", nv::TcuCtxSetCurrent);
            // The 13.1 headers expose cuMemcpyDtoH_v2 (the legacy cuMemcpyDtoH
            // export misbehaves against a 13.1 primary context: CUresult 201).
            let memcpy_dtoh = sym!("cuMemcpyDtoH_v2", nv::TcuMemcpyDtoH);
            Ok(Self {
                _lib: lib,
                init,
                device_get,
                device_get_name,
                device_get_attribute,
                device_get_count,
                device_primary_ctx_retain,
                device_primary_ctx_release,
                ctx_set_current,
                memcpy_dtoh,
            })
        }
    }

    /// Mirror of the CUDA backend's PCI-bus-sorted `--gpu-index` semantics
    /// (domain/bus/device ascending; attribute query failures sort last),
    /// for the `--video-only` path where no cudarc backend exists.
    fn resolve_pci_sorted_index(&self, sorted_index: u32) -> Result<nv::CUdevice, String> {
        // SAFETY: plain driver attribute queries on valid device handles.
        unsafe {
            let mut count: i32 = 0;
            check_cu((self.device_get_count)(&mut count), "cuDeviceGetCount")?;
            let mut devices: Vec<PciSortedDevice> = Vec::new();
            for ordinal in 0..count.max(0) {
                let mut dev: nv::CUdevice = 0;
                check_cu((self.device_get)(&mut dev, ordinal), "cuDeviceGet")?;
                let pci = self.pci_address(dev);
                devices.push((pci, dev));
            }
            devices.sort_by(|a, b| match (a.0, b.0) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.1.cmp(&b.1),
            });
            devices
                .get(sorted_index as usize)
                .map(|entry| entry.1)
                .ok_or_else(|| {
                    format!(
                        "sorted GPU index {sorted_index} out of range (available: 0-{})",
                        devices.len().saturating_sub(1)
                    )
                })
        }
    }

    /// `(domain, bus, device)` PCI address of a device, `None` when the
    /// driver does not report one.
    fn pci_address(&self, dev: nv::CUdevice) -> Option<(u32, u32, u32)> {
        // CU_DEVICE_ATTRIBUTE_PCI_DOMAIN_ID=50, PCI_BUS_ID=33,
        // PCI_DEVICE_ID=34 (dynlink_cuda.h). SAFETY: attribute query on a
        // valid handle.
        unsafe {
            let mut domain: i32 = 0;
            let mut bus: i32 = 0;
            let mut device: i32 = 0;
            if (self.device_get_attribute)(&mut domain, 50, dev) != nv::CU_SUCCESS
                || (self.device_get_attribute)(&mut bus, 33, dev) != nv::CU_SUCCESS
                || (self.device_get_attribute)(&mut device, 34, dev) != nv::CU_SUCCESS
            {
                None
            } else {
                Some((domain as u32, bus as u32, device as u32))
            }
        }
    }
}

fn load_library(name: &[u8], label: &str) -> Result<SendLibrary, String> {
    // The names are static ASCII constants (nvEncodeAPI64.dll, nvcuvid.dll,
    // nvcuda.dll / their .so counterparts); &str satisfies libloading 0.9's
    // AsFilename bound directly.
    let name_str = std::str::from_utf8(&name[..name.len() - 1])
        .map_err(|_| format!("invalid library name for {label}"))?;
    // SAFETY: loading a driver-shipped shared library by name; the handle is
    // kept alive in SendLibrary for the pipeline's lifetime.
    unsafe { Library::new(name_str) }
        .map(SendLibrary)
        .map_err(|err| format!("{label}: LoadLibrary({name_str}) failed: {err}"))
}

fn symbol_error(name: &'static str) -> impl Fn(libloading::Error) -> String {
    move |err| format!("symbol {name} not found: {err}")
}

/// One enumerated device: `(domain, bus, device)` when the driver reports
/// it, plus the raw CUDA ordinal.
type PciSortedDevice = (Option<(u32, u32, u32)>, nv::CUdevice);

fn check_cu(result: nv::CUresult, call: &str) -> Result<(), String> {
    if result == nv::CU_SUCCESS {
        Ok(())
    } else {
        Err(format!("{call} failed: CUresult {result}"))
    }
}

fn check_nvenc(result: u32, call: &str, last_error: Option<&str>) -> Result<(), String> {
    if result == nv::nvenc_status::NV_ENC_SUCCESS {
        return Ok(());
    }
    let status = nv::nvenc_status::name(result);
    match last_error {
        Some(detail) if !detail.is_empty() => Err(format!("{call} failed: {status} ({detail})")),
        _ => Err(format!("{call} failed: {status}")),
    }
}

/// NVENC's own last-error string (best-effort; empty when unavailable).
fn nvenc_last_error(
    func_list: &nv::NV_ENCODE_API_FUNCTION_LIST,
    encoder: *mut std::os::raw::c_void,
) -> Option<String> {
    let get_last = func_list.nv_enc_get_last_error_string?;
    if encoder.is_null() {
        return None;
    }
    // SAFETY: encoder is a live session handle; the returned pointer is
    // documented to stay valid until the next NVENC call on this session.
    let ptr = unsafe { get_last(encoder) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: NvEncGetLastErrorString returns a NUL-terminated C string.
    let text = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    if text.is_empty() { None } else { Some(text) }
}

// ---------------------------------------------------------------------------
// Bitstream ring
// ---------------------------------------------------------------------------

/// Fixed-capacity ring of encoded H264 frames between the encoder and the
/// parser. Slots are reused `Vec`s (no per-frame allocation). When the
/// decoder cannot keep up the oldest frame is overwritten and counted —
/// a stressor drops load rather than buffering unboundedly.
pub struct BitstreamRing {
    slots: Vec<Vec<u8>>,
    /// Next slot a `push` writes (also the oldest slot when the ring is
    /// full, since writing there overwrites it).
    head: usize,
    /// Current occupancy (0..=capacity).
    len: usize,
    capacity: usize,
    dropped: u64,
}

impl BitstreamRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slots: vec![Vec::new(); capacity],
            head: 0,
            len: 0,
            capacity,
            dropped: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Frames dropped because the ring was full when they arrived.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Push one encoded frame, overwriting the oldest entry when full.
    pub fn push(&mut self, data: &[u8]) {
        let slot = self.head;
        self.slots[slot].clear();
        self.slots[slot].extend_from_slice(data);
        self.head = (self.head + 1) % self.capacity;
        if self.len == self.capacity {
            self.dropped += 1;
        } else {
            self.len += 1;
        }
    }

    /// Pop the oldest buffered frame into `out` (the slot's buffer is
    /// reused for the next push). Returns `false` when the ring is empty.
    pub fn pop_oldest(&mut self, out: &mut Vec<u8>) -> bool {
        if self.len == 0 {
            return false;
        }
        let oldest = (self.head + self.capacity - self.len) % self.capacity;
        out.clear();
        out.extend_from_slice(&self.slots[oldest]);
        self.slots[oldest].clear();
        self.len -= 1;
        true
    }
}

// ---------------------------------------------------------------------------
// Synthetic NV12 source + frame checksum
// ---------------------------------------------------------------------------

/// FNV-1a 64-bit over a byte slice — the decoded-frame checksum.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// NV12 luma value for pixel `(x, y)`: the shared vhash32 oracle, same
/// pattern family the CUDA SDC detectors fill (`vhash32(x*2 + row)`).
#[inline]
pub fn nv12_pattern_y(x: usize, y: usize) -> u8 {
    (crate::vhash32((x as u64) * 2 + y as u64) & 0xFF) as u8
}

/// Generate one synthetic NV12 frame into `buf` (layout: `pitch`-strided Y
/// plane of `height` rows, then `height/2` interleaved-UV rows). Chroma is
/// neutral 128; padding between width and pitch is left untouched (zeroed at
/// allocation, constant across frames).
pub fn generate_nv12(width: u32, height: u32, pitch: u32, buf: &mut [u8]) {
    let (width, height, pitch) = (width as usize, height as usize, pitch as usize);
    debug_assert!(buf.len() >= pitch * (height + height / 2));
    for y in 0..height {
        let row = &mut buf[y * pitch..y * pitch + width];
        for (x, byte) in row.iter_mut().enumerate() {
            *byte = nv12_pattern_y(x, y);
        }
    }
    for v in 0..height / 2 {
        let row = &mut buf[(height + v) * pitch..(height + v) * pitch + width];
        row.fill(128);
    }
}

/// Incremental FNV-1a for the readback CRC.
struct FnvHasher {
    hash: u64,
}

impl FnvHasher {
    fn new() -> Self {
        Self { hash: FNV_OFFSET }
    }
    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.hash ^= b as u64;
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }
    fn finish(&self) -> u64 {
        self.hash
    }
}

// ---------------------------------------------------------------------------
// Pipeline state
// ---------------------------------------------------------------------------

/// Decode-side state reachable from the parser callbacks (`pUserData`).
/// The callbacks fire synchronously inside `cuvidParseVideoData` on the
/// worker thread, so plain mutable access through the raw pointer is sound.
struct DecodeState {
    decoder: nv::CUvideodecoder,
    create_decoder: nv::TcuvidCreateDecoder,
    decode_picture: nv::TcuvidDecodePicture,
    /// Display callbacks queue frame indices here; the main loop maps and
    /// reads them back after `cuvidParseVideoData` returns.
    pending: VecDeque<i32>,
    pending_cap: usize,
    decode_errors: u64,
    /// CRC of the first successfully decoded frame (the validation baseline).
    baseline_crc: Option<u64>,
    /// One-shot diagnostics for the callback chain (worker thread only).
    seq_seen: bool,
    seq_logged: bool,
}

impl DecodeState {
    fn new(
        create_decoder: nv::TcuvidCreateDecoder,
        decode_picture: nv::TcuvidDecodePicture,
        pending_cap: usize,
    ) -> Self {
        Self {
            decoder: std::ptr::null_mut(),
            create_decoder,
            decode_picture,
            pending: VecDeque::new(),
            pending_cap: pending_cap.max(1),
            decode_errors: 0,
            baseline_crc: None,
            seq_seen: false,
            seq_logged: false,
        }
    }
}

/// One live NVENC/NVDEC pipeline (CUDA context + encoder + parser/decoder).
/// Created on the caller thread, moved to the worker.
struct VideoPipeline {
    cuda: CudaLib,
    /// Kept alive for the session: the NVENC Library must not unload while
    /// the encoder's function pointers are live.
    _nvenc: NvencLib,
    cuvid: CuvidLib,
    device: nv::CUdevice,
    context: nv::CUcontext,
    encoder: *mut std::os::raw::c_void,
    func_list: Box<nv::NV_ENCODE_API_FUNCTION_LIST>,
    input_buffer: *mut std::os::raw::c_void,
    input_pitch: u32,
    bitstream_buffer: *mut std::os::raw::c_void,
    parser: nv::CUvideoparser,
    decode_state: Box<DecodeState>,
    ring: BitstreamRing,
    /// Scratch for handing ring entries to the parser.
    feed: Vec<u8>,
    width: u32,
    height: u32,
    frame_nv12: Vec<u8>,
    /// Reused D2H readback staging (sized to the largest mapped pitch).
    readback: Vec<u8>,
    validate: bool,
    /// PCI label / marketing name of the selected device (banner only).
    device_label: String,
    device_name: String,
}

// The pipeline owns raw driver handles and is only ever touched by one
// thread at a time (built on the caller thread, then moved to the worker).
unsafe impl Send for VideoPipeline {}

impl VideoPipeline {
    fn new(config: &VideoConfig, dims: (u32, u32)) -> Result<Self, String> {
        let (width, height) = dims;

        // --- CUDA: own primary context, independent of cudarc -------------
        let cuda = CudaLib::load()?;
        // SAFETY: driver init + device/attribute queries on valid handles.
        unsafe {
            check_cu((cuda.init)(0), "cuInit")?;
        }
        let (device, device_label) = if config.device_is_pci_sorted {
            let dev = cuda.resolve_pci_sorted_index(config.device_index)?;
            let pci = cuda
                .pci_address(dev)
                .map(|(d, b, e)| format!("{d:04X}:{b:02X}:{e:02X}.0"))
                .unwrap_or_else(|| "<no pci>".to_string());
            (dev, pci)
        } else {
            // Mixed mode: the caller already resolved a CUDA ordinal.
            // SAFETY: out-pointer write into a local.
            unsafe {
                let mut dev: nv::CUdevice = 0;
                check_cu(
                    (cuda.device_get)(&mut dev, config.device_index as i32),
                    "cuDeviceGet",
                )?;
                (dev, format!("ordinal {}", config.device_index))
            }
        };
        let device_name = device_name(&cuda, device);
        // SAFETY: primary-context retain is refcounted; released on the
        // error path below and in Drop on success.
        let context = unsafe {
            let mut ctx: nv::CUcontext = std::ptr::null_mut();
            check_cu(
                (cuda.device_primary_ctx_retain)(&mut ctx, device),
                "cuDevicePrimaryCtxRetain",
            )?;
            ctx
        };
        // SAFETY: making the primary context current on this thread.
        unsafe {
            check_cu((cuda.ctx_set_current)(context), "cuCtxSetCurrent")?;
        }

        // Everything below can fail after the primary context was retained:
        // the inner closure tears its own NVENC/NVDEC resources down, and
        // the match arm releases the context on any residual error.
        let build = || -> Result<VideoPipeline, String> {
            let nvenc = NvencLib::load()?;
            // SDK version gate before any session exists.
            // SAFETY: out-pointer write into a local u32.
            let max_version = unsafe {
                let mut version: u32 = 0;
                check_nvenc(
                    (nvenc.get_max_supported_version)(&mut version),
                    "NvEncodeAPIGetMaxSupportedVersion",
                    None,
                )?;
                version
            };
            let required =
                nv::nvencapi_packed_version(nv::NVENCAPI_MAJOR_VERSION, nv::NVENCAPI_MINOR_VERSION);
            if max_version < required {
                return Err(format!(
                    "driver NVENC API too old: max supported {}.{} but the SDK 13.1 bindings need \
                     at least {}.{}",
                    max_version >> 4,
                    max_version & 0xF,
                    required >> 4,
                    required & 0xF
                ));
            }
            let cuvid = CuvidLib::load()?;

            let mut func_list = Box::new(nv::NV_ENCODE_API_FUNCTION_LIST::default());
            func_list.version = nv::NV_ENCODE_API_FUNCTION_LIST_VER;
            // SAFETY: func_list is a live, correctly versioned function table.
            unsafe {
                check_nvenc(
                    (nvenc.create_instance)(&mut *func_list),
                    "NvEncodeAPICreateInstance",
                    None,
                )?;
            }
            let encoder = open_encode_session(&func_list, context)?;

            // Session resources that may need teardown on later failures.
            let mut input_buffer: *mut std::os::raw::c_void = std::ptr::null_mut();
            let mut bitstream_buffer: *mut std::os::raw::c_void = std::ptr::null_mut();

            let built = (|| -> Result<(u32, Box<DecodeState>, nv::CUvideoparser), String> {
                let preset_guid = nv::preset_guid(&config.preset)
                    .ok_or_else(|| format!("unknown preset {}", config.preset))?;

                // Preset configuration as the base when the driver supports
                // the query (NvEncGetEncodePresetConfigEx). Some current
                // driver builds reject the query outright with
                // NV_ENC_ERR_INVALID_VERSION regardless of the struct version
                // (verified against the real headers from C on an RTX 4060
                // Laptop); a hand-built NV_ENC_CONFIG is the documented
                // alternative and is used as the fallback.
                let mut encode_config = nv::NV_ENC_CONFIG::default();
                let mut preset_base = false;
                if let Some(preset_fn) = func_list.nv_enc_get_encode_preset_config_ex {
                    let mut preset_config = nv::NV_ENC_PRESET_CONFIG {
                        version: nv::NV_ENC_PRESET_CONFIG_VER,
                        ..nv::NV_ENC_PRESET_CONFIG::default()
                    };
                    // SAFETY: preset_config is a live, correctly versioned
                    // output struct; encoder is the session handle.
                    let rc = unsafe {
                        preset_fn(
                            encoder,
                            nv::NV_ENC_CODEC_H264_GUID,
                            preset_guid,
                            nv::NV_ENC_TUNING_INFO_HIGH_QUALITY,
                            &mut preset_config,
                        )
                    };
                    if rc == nv::nvenc_status::NV_ENC_SUCCESS {
                        encode_config = preset_config.preset_cfg;
                        preset_base = true;
                    } else {
                        println!(
                            "[Video] preset config query unavailable ({}, using hand-built encode config)",
                            nv::nvenc_status::name(rc)
                        );
                    }
                }
                encode_config.version = nv::NV_ENC_CONFIG_VER;
                encode_config.profile_guid = nv::NV_ENC_H264_PROFILE_MAIN_GUID;
                encode_config.gop_length = 1; // every frame is an I frame
                encode_config.frame_interval_p = 1; // no B frames
                if !preset_base {
                    // Progressive input; the preset base already sets this.
                    encode_config.frame_field_mode = nv::NV_ENC_PARAMS_FRAME_FIELD_MODE_FRAME;
                }
                encode_config.rc_params.version = nv::NV_ENC_RC_PARAMS_VER;
                let cbr_bitrate = (!config.validate).then_some(config.bitrate_bps).flatten();
                match cbr_bitrate {
                    Some(bitrate) => {
                        encode_config.rc_params.rate_control_mode = nv::NV_ENC_PARAMS_RC_CBR;
                        encode_config.rc_params.average_bit_rate = bitrate;
                    }
                    // Deterministic constant-QP encode: the CRC baseline below
                    // relies on identical input producing identical output.
                    None => {
                        encode_config.rc_params.rate_control_mode = nv::NV_ENC_PARAMS_RC_CONSTQP;
                        encode_config.rc_params.const_qp = nv::NV_ENC_QP {
                            qp_inter_p: 26,
                            qp_inter_b: 26,
                            qp_intra: 26,
                        };
                    }
                }
                // SAFETY: the union's h264 member is the accessed view.
                let h264 = unsafe { &mut encode_config.encode_codec_config.h264_config };
                h264.idr_period = 1;
                h264.chroma_format_idc = 1; // 4:2:0 for NV12 input
                // repeatSPSPPS (bit 12 of the H264 bitfield) so every IDR
                // carries SPS/PPS and the parser can (re)initialize the
                // decoder at will.
                h264.bitfields |= 1 << 12;

                let mut init_params = nv::NV_ENC_INITIALIZE_PARAMS {
                    version: nv::NV_ENC_INITIALIZE_PARAMS_VER,
                    encode_guid: nv::NV_ENC_CODEC_H264_GUID,
                    preset_guid,
                    encode_width: width,
                    encode_height: height,
                    dar_width: width,
                    dar_height: height,
                    frame_rate_num: config.fps.max(1),
                    frame_rate_den: 1,
                    enable_encode_async: 0, // synchronous mode
                    enable_ptd: 1,          // encoder picks picture types
                    tuning_info: nv::NV_ENC_TUNING_INFO_HIGH_QUALITY,
                    encode_config: &mut encode_config,
                    ..nv::NV_ENC_INITIALIZE_PARAMS::default()
                };
                unsafe {
                    let init_fn = func_list
                        .nv_enc_initialize_encoder
                        .ok_or("NVENC function table missing nvEncInitializeEncoder")?;
                    check_nvenc(
                        init_fn(encoder, &mut init_params),
                        "NvEncInitializeEncoder",
                        nvenc_last_error(&func_list, encoder).as_deref(),
                    )?;
                }

                // System-memory input buffer + output bitstream buffer.
                let mut input_create = nv::NV_ENC_CREATE_INPUT_BUFFER {
                    version: nv::NV_ENC_CREATE_INPUT_BUFFER_VER,
                    width,
                    height,
                    buffer_fmt: nv::NV_ENC_BUFFER_FORMAT_NV12,
                    memory_heap: nv::NV_ENC_MEMORY_HEAP_AUTOSELECT,
                    ..nv::NV_ENC_CREATE_INPUT_BUFFER::default()
                };
                unsafe {
                    let create_fn = func_list
                        .nv_enc_create_input_buffer
                        .ok_or("NVENC function table missing nvEncCreateInputBuffer")?;
                    check_nvenc(
                        create_fn(encoder, &mut input_create),
                        "NvEncCreateInputBuffer",
                        nvenc_last_error(&func_list, encoder).as_deref(),
                    )?;
                }
                input_buffer = input_create.input_buffer;
                let input_pitch = locked_input_pitch(&func_list, encoder, input_buffer)?;

                let mut bitstream_create = nv::NV_ENC_CREATE_BITSTREAM_BUFFER {
                    version: nv::NV_ENC_CREATE_BITSTREAM_BUFFER_VER,
                    ..nv::NV_ENC_CREATE_BITSTREAM_BUFFER::default()
                };
                unsafe {
                    let create_fn = func_list
                        .nv_enc_create_bitstream_buffer
                        .ok_or("NVENC function table missing nvEncCreateBitstreamBuffer")?;
                    check_nvenc(
                        create_fn(encoder, &mut bitstream_create),
                        "NvEncCreateBitstreamBuffer",
                        nvenc_last_error(&func_list, encoder).as_deref(),
                    )?;
                }
                bitstream_buffer = bitstream_create.bitstream_buffer;

                // Probe caps so a missing H264 NVDEC degrades with a clear
                // message instead of a confusing parser failure.
                let mut caps = nv::CUVIDDECODECAPS {
                    e_codec_type: nv::CUDA_VIDEO_CODEC_H264,
                    e_chroma_format: nv::CUDA_VIDEO_CHROMA_FORMAT_420,
                    n_bit_depth_minus8: 0,
                    ..nv::CUVIDDECODECAPS::default()
                };
                // SAFETY: caps probe with a fully initialized input struct.
                unsafe {
                    check_cu((cuvid.get_decoder_caps)(&mut caps), "cuvidGetDecoderCaps")?;
                    if caps.b_is_supported == 0 {
                        return Err(
                            "NVDEC reports no support for H264 4:2:0 8-bit on this device"
                                .to_string(),
                        );
                    }
                }

                // The decode-state box must be pinned before the parser is
                // created: the parser stores its address as pUserData.
                let mut decode_state = Box::new(DecodeState::new(
                    cuvid.create_decoder,
                    cuvid.decode_picture,
                    8,
                ));
                let mut parser_params = nv::CUVIDPARSERPARAMS {
                    codec_type: nv::CUDA_VIDEO_CODEC_H264,
                    ul_max_num_decode_surfaces: 4,
                    ul_max_display_delay: 0, // display immediately
                    p_user_data: std::ptr::from_mut(&mut *decode_state)
                        .cast::<std::os::raw::c_void>(),
                    pfn_sequence_callback: Some(sequence_callback),
                    pfn_decode_picture: Some(decode_callback),
                    pfn_display_picture: Some(display_callback),
                    ..nv::CUVIDPARSERPARAMS::default()
                };
                // SAFETY: parser_params is fully initialized; the callbacks
                // dereference pUserData only on the creating thread's parse
                // calls while the box stays pinned here.
                unsafe {
                    let mut parser: nv::CUvideoparser = std::ptr::null_mut();
                    check_cu(
                        (cuvid.create_video_parser)(&mut parser, &mut parser_params),
                        "cuvidCreateVideoParser",
                    )?;
                    Ok((input_pitch, decode_state, parser))
                }
            })();

            match built {
                Ok((input_pitch, decode_state, parser)) => Ok(VideoPipeline {
                    cuda,
                    _nvenc: nvenc,
                    cuvid,
                    device,
                    context,
                    encoder,
                    func_list,
                    input_buffer,
                    input_pitch,
                    bitstream_buffer,
                    parser,
                    decode_state,
                    ring: BitstreamRing::new(4),
                    feed: Vec::new(),
                    width,
                    height,
                    frame_nv12: vec![0u8; input_pitch as usize * height as usize * 3 / 2],
                    readback: vec![0u8; input_pitch as usize * height as usize * 3 / 2],
                    validate: config.validate,
                    device_label: String::new(),
                    device_name: String::new(),
                }),
                Err(err) => {
                    // Best-effort teardown of the partial session.
                    // SAFETY: each handle below was created by this session
                    // and is destroyed here in reverse creation order.
                    unsafe {
                        (cuda.device_primary_ctx_release)(device);
                        if !bitstream_buffer.is_null()
                            && let Some(destroy_fn) = func_list.nv_enc_destroy_bitstream_buffer
                        {
                            let _ = destroy_fn(encoder, bitstream_buffer);
                        }
                        if !input_buffer.is_null()
                            && let Some(destroy_fn) = func_list.nv_enc_destroy_input_buffer
                        {
                            let _ = destroy_fn(encoder, input_buffer);
                        }
                        if let Some(destroy_fn) = func_list.nv_enc_destroy_encoder {
                            let _ = destroy_fn(encoder);
                        }
                    }
                    Err(err)
                }
            }
        };

        match build() {
            Ok(mut pipeline) => {
                pipeline.device_label = device_label.clone();
                pipeline.device_name = device_name.clone();
                println!(
                    "[Video] NVENC->NVDEC pipeline ready: {}x{} NV12, H264 Main (preset {}, \
                     gopLength=1, all-I), validate={}, device #{} {} ({})",
                    width,
                    height,
                    config.preset,
                    config.validate,
                    config.device_index,
                    device_label,
                    device_name,
                );
                Ok(pipeline)
            }
            Err(err) => Err(err),
        }
    }

    /// Encode one frame and push its bitstream onto the ring. Returns the
    /// encoded size.
    fn encode_frame(&mut self, frame_idx: u64) -> Result<usize, String> {
        let width = self.width as usize;
        let height = self.height as usize;
        let pitch = self.input_pitch as usize;
        generate_nv12(
            self.width,
            self.height,
            self.input_pitch,
            &mut self.frame_nv12,
        );

        let last_error = || nvenc_last_error(&self.func_list, self.encoder);

        // Lock the sysmem input buffer, write the NV12 rows, unlock.
        let mut lock = nv::NV_ENC_LOCK_INPUT_BUFFER {
            version: nv::NV_ENC_LOCK_INPUT_BUFFER_VER,
            input_buffer: self.input_buffer,
            ..nv::NV_ENC_LOCK_INPUT_BUFFER::default()
        };
        // SAFETY: lock/unlock bracket the only host access to input_buffer;
        // the driver-returned pitch bounds the writable extent.
        unsafe {
            let lock_fn = self
                .func_list
                .nv_enc_lock_input_buffer
                .ok_or("NVENC function table missing nvEncLockInputBuffer")?;
            check_nvenc(
                lock_fn(self.encoder, &mut lock),
                "NvEncLockInputBuffer",
                last_error().as_deref(),
            )?;
            let live_pitch = (lock.pitch as usize).max(pitch);
            let rows = height + height / 2;
            let dst = std::slice::from_raw_parts_mut(
                lock.buffer_data_ptr.cast::<u8>(),
                live_pitch * rows,
            );
            for row in 0..rows {
                dst[row * live_pitch..row * live_pitch + width]
                    .copy_from_slice(&self.frame_nv12[row * pitch..row * pitch + width]);
            }
            let unlock_fn = self
                .func_list
                .nv_enc_unlock_input_buffer
                .ok_or("NVENC function table missing nvEncUnlockInputBuffer")?;
            check_nvenc(
                unlock_fn(self.encoder, self.input_buffer),
                "NvEncUnlockInputBuffer",
                last_error().as_deref(),
            )?;
        }
        let buffer_pitch = lock.pitch.max(self.input_pitch);

        // Encode the picture (synchronous mode: LockBitstream waits for HW).
        let mut pic = nv::NV_ENC_PIC_PARAMS {
            version: nv::NV_ENC_PIC_PARAMS_VER,
            input_width: self.width,
            input_height: self.height,
            input_pitch: buffer_pitch,
            input_buffer: self.input_buffer,
            output_bitstream: self.bitstream_buffer,
            buffer_fmt: nv::NV_ENC_BUFFER_FORMAT_NV12,
            picture_struct: nv::NV_ENC_PIC_STRUCT_FRAME,
            picture_type: nv::NV_ENC_PIC_TYPE_UNKNOWN, // enablePTD decides
            frame_idx: frame_idx as u32,
            ..nv::NV_ENC_PIC_PARAMS::default()
        };
        // SAFETY: all pointers are live session resources.
        unsafe {
            let encode_fn = self
                .func_list
                .nv_enc_encode_picture
                .ok_or("NVENC function table missing nvEncEncodePicture")?;
            check_nvenc(
                encode_fn(self.encoder, &mut pic),
                "NvEncEncodePicture",
                last_error().as_deref(),
            )?;
        }

        // Retrieve the bitstream.
        let mut lock_bs = nv::NV_ENC_LOCK_BITSTREAM {
            version: nv::NV_ENC_LOCK_BITSTREAM_VER,
            output_bitstream: self.bitstream_buffer,
            ..nv::NV_ENC_LOCK_BITSTREAM::default()
        };
        let encoded;
        // SAFETY: lock/unlock bracket the only host access to the bitstream.
        unsafe {
            let lock_fn = self
                .func_list
                .nv_enc_lock_bitstream
                .ok_or("NVENC function table missing nvEncLockBitstream")?;
            check_nvenc(
                lock_fn(self.encoder, &mut lock_bs),
                "NvEncLockBitstream",
                last_error().as_deref(),
            )?;
            // NOTE: `hw_encode_status` is not authoritative on all driver
            // builds (live-verified on an RTX 4060 Laptop @ SDK 13.1: a
            // successful encode reports a stale value there); the status of
            // NvEncEncodePicture / NvEncLockBitstream themselves governs.
            let size = lock_bs.bitstream_size_in_bytes as usize;
            let src = std::slice::from_raw_parts(lock_bs.bitstream_buffer_ptr.cast::<u8>(), size);
            self.ring.push(src);
            encoded = size;
            let unlock_fn = self
                .func_list
                .nv_enc_unlock_bitstream
                .ok_or("NVENC function table missing nvEncUnlockBitstream")?;
            check_nvenc(
                unlock_fn(self.encoder, self.bitstream_buffer),
                "NvEncUnlockBitstream",
                last_error().as_deref(),
            )?;
        }
        Ok(encoded)
    }

    /// Feed one encoded frame to the parser (decode + display callbacks fire
    /// synchronously inside the parse), then map/read/validate every frame
    /// the parser made displayable during that parse.
    fn decode_pass(&mut self, counters: &VideoCounters) -> Result<(), String> {
        let mut packet = nv::CUVIDSOURCEDATAPACKET {
            flags: nv::CUVID_PKT_ENDOFPICTURE,
            payload_size: self.feed.len() as nv::TcuUlong,
            payload: self.feed.as_ptr(),
            timestamp: 0,
        };
        // SAFETY: parser and packet are live; the payload outlives the call.
        unsafe {
            check_cu(
                (self.cuvid.parse_video_data)(self.parser, &mut packet),
                "cuvidParseVideoData",
            )?;
        }

        // Map + read back every displayed frame produced by this parse.
        while let Some(pic_idx) = self.decode_state.pending.pop_front() {
            match self.read_decoded_frame(pic_idx) {
                Ok(crc) => {
                    counters.decoded_frames.fetch_add(1, Ordering::SeqCst);
                    if self.validate {
                        match self.decode_state.baseline_crc {
                            None => {
                                self.decode_state.baseline_crc = Some(crc);
                                println!(
                                    "[Video] validate: baseline frame CRC 0x{crc:016x} pinned"
                                );
                            }
                            Some(baseline) if baseline != crc => {
                                counters.crc_mismatches.fetch_add(1, Ordering::SeqCst);
                                println!(
                                    "[Video] validate: CRC MISMATCH (expected 0x{baseline:016x}, got 0x{crc:016x})"
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Err(err) => {
                    counters.decode_missed.fetch_add(1, Ordering::SeqCst);
                    self.decode_state.decode_errors += 1;
                    if self.decode_state.decode_errors <= 3 {
                        println!("[Video] decode readback failed: {err}");
                    }
                }
            }
        }
        Ok(())
    }

    /// Map a decoded surface, copy its NV12 rows D2H, checksum, unmap.
    fn read_decoded_frame(&mut self, pic_idx: i32) -> Result<u64, String> {
        let (width, height) = (self.width as usize, self.height as usize);
        let mut proc_params = nv::CUVIDPROCPARAMS {
            progressive_frame: 1,
            ..nv::CUVIDPROCPARAMS::default()
        };
        let mut dev_ptr: u64 = 0;
        let mut pitch: u32 = 0;
        // SAFETY: decoder handle live; mapping within ulNumOutputSurfaces
        // (one mapped at a time).
        unsafe {
            check_cu(
                (self.cuvid.map_video_frame)(
                    self.decode_state.decoder,
                    pic_idx,
                    &mut dev_ptr,
                    &mut pitch,
                    &mut proc_params,
                ),
                "cuvidMapVideoFrame64",
            )?;
        }
        // Always unmap, even when the D2H copy fails, or the mapped-surface
        // budget (ulNumOutputSurfaces) would leak until the loop stalls.
        let result = self.copy_and_crc(dev_ptr, pitch as usize, width, height);
        // SAFETY: unmapping the pointer cuvidMapVideoFrame64 just returned.
        unsafe {
            if (self.cuvid.unmap_video_frame)(self.decode_state.decoder, dev_ptr) != nv::CU_SUCCESS
            {
                println!("[Video] cuvidUnmapVideoFrame64 failed for pic {pic_idx}");
            }
        }
        result
    }

    fn copy_and_crc(
        &mut self,
        dev_ptr: u64,
        pitch: usize,
        width: usize,
        height: usize,
    ) -> Result<u64, String> {
        // cuvidMapVideoFrame64 leaves the thread's current CUDA context
        // cleared on this driver build (same pop-too-many behavior as the
        // NVENC calls); re-assert it before the D2H copies.
        // SAFETY: re-asserting the retained primary context on the worker.
        unsafe {
            check_cu(
                (self.cuda.ctx_set_current)(self.context),
                "cuCtxSetCurrent[readback]",
            )?;
        }
        let rows = height + height / 2;
        let needed = pitch * rows;
        if self.readback.len() < needed {
            self.readback.resize(needed, 0);
        }
        // One bulk D2H for the whole pitch-strided NV12 surface (per-row
        // copies cost ~30 us of driver overhead each and throttle the loop
        // to a fraction of the achievable rate).
        // SAFETY: dev_ptr is a mapped NVDEC surface of `pitch * rows` bytes.
        unsafe {
            check_cu(
                (self.cuda.memcpy_dtoh)(self.readback.as_mut_ptr().cast(), dev_ptr, needed),
                "cuMemcpyDtoH_v2",
            )?;
        }
        // CRC only the live pixels (padding is whatever the driver leaves).
        let mut hasher = FnvHasher::new();
        for row in 0..rows {
            hasher.update(&self.readback[row * pitch..row * pitch + width]);
        }
        Ok(hasher.finish())
    }

    /// Destroy everything in reverse dependency order (best-effort; runs
    /// from Drop after the loop ends). Errors are logged, never fatal.
    fn destroy(&mut self) {
        // SAFETY: each call destroys a resource this pipeline owns, in an
        // order that respects session/surface dependencies.
        unsafe {
            if !self.encoder.is_null() {
                if let Some(destroy_fn) = self.func_list.nv_enc_destroy_encoder {
                    let rc = destroy_fn(self.encoder);
                    if rc != nv::nvenc_status::NV_ENC_SUCCESS {
                        println!(
                            "[Video] NvEncDestroyEncoder failed: {}",
                            nv::nvenc_status::name(rc)
                        );
                    }
                }
                self.encoder = std::ptr::null_mut();
            }
            if !self.parser.is_null() {
                if (self.cuvid.destroy_video_parser)(self.parser) != nv::CU_SUCCESS {
                    println!("[Video] cuvidDestroyVideoParser failed");
                }
                self.parser = std::ptr::null_mut();
            }
            // The decoder handle is owned by the decode state (created inside
            // the sequence callback); parser destruction does not release it.
            if !self.decode_state.decoder.is_null() {
                if (self.cuvid.destroy_decoder)(self.decode_state.decoder) != nv::CU_SUCCESS {
                    println!("[Video] cuvidDestroyDecoder failed");
                }
                self.decode_state.decoder = std::ptr::null_mut();
            }
            if !self.context.is_null() {
                (self.cuda.device_primary_ctx_release)(self.device);
                self.context = std::ptr::null_mut();
            }
        }
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn device_name(cuda: &CudaLib, device: nv::CUdevice) -> String {
    // SAFETY: name buffer write via the documented driver API.
    unsafe {
        let mut name = [0u8; 128];
        if (cuda.device_get_name)(name.as_mut_ptr(), name.len() as i32 - 1, device)
            == nv::CU_SUCCESS
        {
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            String::from_utf8_lossy(&name[..end]).into_owned()
        } else {
            format!("CUDA device {device}")
        }
    }
}

/// Open the NVENC session on our CUDA primary context (device type CUDA,
/// device = CUcontext pointer, per nvEncodeAPI.h).
fn open_encode_session(
    func_list: &nv::NV_ENCODE_API_FUNCTION_LIST,
    context: nv::CUcontext,
) -> Result<*mut std::os::raw::c_void, String> {
    let mut open_params = nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
        version: nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER,
        device_type: nv::NV_ENC_DEVICE_TYPE_CUDA,
        device: context,
        api_version: nv::NVENCAPI_VERSION,
        ..nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS::default()
    };
    // SAFETY: open_params is fully initialized; the out-handle is stored.
    unsafe {
        let open_fn = func_list
            .nv_enc_open_encode_session_ex
            .ok_or("NVENC function table missing nvEncOpenEncodeSessionEx")?;
        let mut encoder: *mut std::os::raw::c_void = std::ptr::null_mut();
        check_nvenc(
            open_fn(&mut open_params, &mut encoder),
            "NvEncOpenEncodeSessionEx",
            None,
        )?;
        Ok(encoder)
    }
}

/// Lock the freshly created input buffer once to learn its pitch (the
/// encoder-allocated NV12 surface stride; reused as the lock pitch for
/// every later frame write).
fn locked_input_pitch(
    func_list: &nv::NV_ENCODE_API_FUNCTION_LIST,
    encoder: *mut std::os::raw::c_void,
    input_buffer: *mut std::os::raw::c_void,
) -> Result<u32, String> {
    let last_error = || nvenc_last_error(func_list, encoder);
    let mut lock = nv::NV_ENC_LOCK_INPUT_BUFFER {
        version: nv::NV_ENC_LOCK_INPUT_BUFFER_VER,
        input_buffer,
        ..nv::NV_ENC_LOCK_INPUT_BUFFER::default()
    };
    // SAFETY: lock/unlock bracket a probe access; nothing is written.
    unsafe {
        let lock_fn = func_list
            .nv_enc_lock_input_buffer
            .ok_or("NVENC function table missing nvEncLockInputBuffer")?;
        check_nvenc(
            lock_fn(encoder, &mut lock),
            "NvEncLockInputBuffer",
            last_error().as_deref(),
        )?;
        let pitch = lock.pitch;
        let unlock_fn = func_list
            .nv_enc_unlock_input_buffer
            .ok_or("NVENC function table missing nvEncUnlockInputBuffer")?;
        check_nvenc(
            unlock_fn(encoder, input_buffer),
            "NvEncUnlockInputBuffer",
            last_error().as_deref(),
        )?;
        if pitch == 0 {
            return Err("NvEncLockInputBuffer reported pitch 0".to_string());
        }
        Ok(pitch)
    }
}

// ---------------------------------------------------------------------------
// Parser callbacks (synchronous, worker thread only)
// ---------------------------------------------------------------------------

/// Sequence callback: remember the format, create the decoder on first
/// sight, and report the decode-surface count back to the parser (>1
/// overrides the parser DPB size, per dynlink_nvcuvid.h).
unsafe extern "system" fn sequence_callback(
    user_data: *mut std::os::raw::c_void,
    format: *mut nv::CUVIDEOFORMAT,
) -> i32 {
    if user_data.is_null() || format.is_null() {
        return 0;
    }
    // SAFETY: pUserData points at the pipeline's pinned DecodeState, and the
    // parser invokes callbacks synchronously on the worker thread.
    let state = unsafe { &mut *(user_data as *mut DecodeState) };
    let format = unsafe { &*format };
    state.seq_seen = true;
    if !state.seq_logged {
        state.seq_logged = true;
        println!(
            "[Video] parser sequence: codec={} coded={}x{} chroma={} min_surfaces={}",
            format.codec,
            format.coded_width,
            format.coded_height,
            format.chroma_format,
            format.min_num_decode_surfaces,
        );
    }
    if format.codec != nv::CUDA_VIDEO_CODEC_H264 {
        state.decode_errors += 1;
        return 0;
    }
    if state.decoder.is_null() {
        // SAFETY: cuvidCreateDecoder fills the out-handle; the info struct
        // mirrors CUVIDDECODECREATEINFO's header layout exactly.
        let surfaces = (format.min_num_decode_surfaces as u32).max(4);
        let mut info = nv::CUVIDDECODECREATEINFO {
            ul_width: format.coded_width as nv::TcuUlong,
            ul_height: format.coded_height as nv::TcuUlong,
            ul_num_decode_surfaces: surfaces,
            codec_type: nv::CUDA_VIDEO_CODEC_H264,
            chroma_format: format.chroma_format,
            ul_creation_flags: nv::CUDA_VIDEO_CREATE_DEFAULT as nv::TcuUlong,
            bit_depth_minus8: format.bit_depth_luma_minus8 as nv::TcuUlong,
            ul_intra_decode_only: 1, // our stream is all-I
            ul_max_width: format.coded_width as nv::TcuUlong,
            ul_max_height: format.coded_height as nv::TcuUlong,
            output_format: nv::CUDA_VIDEO_SURFACE_FORMAT_NV12,
            deinterlace_mode: nv::CUDA_VIDEO_DEINTERLACE_MODE_WEAVE,
            ul_target_width: format.coded_width as nv::TcuUlong,
            ul_target_height: format.coded_height as nv::TcuUlong,
            ul_num_output_surfaces: 2,
            ..nv::CUVIDDECODECREATEINFO::default()
        };
        let mut decoder: nv::CUvideodecoder = std::ptr::null_mut();
        let create_rc =
            unsafe { (state.create_decoder)(std::ptr::from_mut(&mut decoder).cast(), &mut info) };
        if create_rc != nv::CU_SUCCESS {
            println!("[Video] cuvidCreateDecoder failed: CUresult {create_rc}");
            state.decode_errors += 1;
            return 0;
        }
        state.decoder = decoder;
        state.pending_cap = surfaces as usize + 2;
        return surfaces as i32;
    }
    // Format change on a live decoder is not supported by this stressor.
    0
}

/// Decode callback: kick HW decoding for the picture the parser staged.
unsafe extern "system" fn decode_callback(
    user_data: *mut std::os::raw::c_void,
    pic_params: *mut nv::CUVIDPICPARAMS,
) -> i32 {
    if user_data.is_null() || pic_params.is_null() {
        return 0;
    }
    // SAFETY: parser-owned pic params + pinned DecodeState (worker thread).
    let state = unsafe { &mut *(user_data as *mut DecodeState) };
    if state.decoder.is_null() {
        return 0;
    }
    if unsafe { (state.decode_picture)(state.decoder, pic_params) } == nv::CU_SUCCESS {
        1
    } else {
        if state.decode_errors < 3 {
            println!("[Video] cuvidDecodePicture failed for pic {}", unsafe {
                (*pic_params).curr_pic_idx
            });
        }
        state.decode_errors += 1;
        0
    }
}

/// Display callback: queue the surface index for mapping by the main loop.
unsafe extern "system" fn display_callback(
    user_data: *mut std::os::raw::c_void,
    disp_info: *mut nv::CUVIDPARSERDISPINFO,
) -> i32 {
    if user_data.is_null() || disp_info.is_null() {
        return 0;
    }
    // SAFETY: parser-owned disp info + pinned DecodeState (worker thread).
    let state = unsafe { &mut *(user_data as *mut DecodeState) };
    let disp = unsafe { &*disp_info };
    if state.pending.len() >= state.pending_cap {
        state.pending.pop_front();
        state.decode_errors += 1;
    }
    state.pending.push_back(disp.picture_index);
    1
}

// ---------------------------------------------------------------------------
// Worker loop
// ---------------------------------------------------------------------------

fn pipeline_loop(
    mut pipeline: VideoPipeline,
    stop: Arc<AtomicBool>,
    error: Arc<AtomicBool>,
    counters: Arc<VideoCounters>,
    deadline: Option<std::time::Duration>,
) {
    let started = Instant::now();
    let mut frame_idx: u64 = 0;
    let mut last_report = started;
    let result = run_loop(
        &mut pipeline,
        &stop,
        &counters,
        deadline,
        started,
        &mut frame_idx,
        &mut last_report,
    );
    // VideoPipeline::drop runs the full teardown.
    if let Err(err) = result {
        println!("[Video] {err}");
        error.store(true, Ordering::SeqCst);
    }
}

fn run_loop(
    pipeline: &mut VideoPipeline,
    stop: &Arc<AtomicBool>,
    counters: &Arc<VideoCounters>,
    deadline: Option<std::time::Duration>,
    started: Instant,
    frame_idx: &mut u64,
    last_report: &mut Instant,
) -> Result<(), String> {
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(limit) = deadline
            && started.elapsed() >= limit
        {
            return Ok(());
        }

        // 1. Encode one synthetic frame into the ring.
        match pipeline.encode_frame(*frame_idx) {
            Ok(size) => {
                counters.encoded_frames.fetch_add(1, Ordering::SeqCst);
                counters
                    .encoded_bytes
                    .fetch_add(size as u64, Ordering::SeqCst);
                counters
                    .ring_dropped
                    .store(pipeline.ring.dropped(), Ordering::SeqCst);
            }
            Err(err) => return Err(format!("encode frame {} failed: {err}", *frame_idx)),
        }
        *frame_idx += 1;

        // 2. Hand the oldest encoded frame(s) to the parser (decode + display
        //    callbacks fire synchronously inside the parse) and read back
        //    whatever became displayable. The NVENC calls above leave the
        //    thread's current CUDA context cleared on some driver builds
        //    (live-verified: cuvidCreateDecoder returns
        //    CUDA_ERROR_INVALID_CONTEXT otherwise), so re-assert it here.
        // SAFETY: re-asserting the retained primary context on the worker.
        unsafe {
            check_cu(
                (pipeline.cuda.ctx_set_current)(pipeline.context),
                "cuCtxSetCurrent[decode]",
            )?;
        }
        loop {
            if !pipeline.ring.pop_oldest(&mut pipeline.feed) {
                break;
            }
            pipeline.decode_pass(counters)?;
        }

        // Progress line every 2 s.
        if last_report.elapsed().as_secs_f64() >= 2.0 {
            let elapsed = started.elapsed().as_secs_f64();
            let encoded = counters.encoded_frames.load(Ordering::SeqCst);
            let decoded = counters.decoded_frames.load(Ordering::SeqCst);
            let mismatches = counters.crc_mismatches.load(Ordering::SeqCst);
            let bytes = counters.encoded_bytes.load(Ordering::SeqCst);
            println!(
                "[Video] t={elapsed:6.1}s | enc={encoded} ({:.1} Mbps) | dec={decoded} ({:.1} fps) | crc_mismatch={mismatches}",
                if elapsed > 0.0 {
                    bytes as f64 * 8.0 / elapsed / 1e6
                } else {
                    0.0
                },
                if elapsed > 0.0 {
                    decoded as f64 / elapsed
                } else {
                    0.0
                },
            );
            *last_report = Instant::now();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (pure logic only — no GPU touched; hardware runs stay behind
// #[ignore] like tests/verify_gpu.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_pattern_matches_vhash32_oracle() {
        // Y = vhash32(x*2 + y) & 0xFF: spot-check against the shared oracle
        // and verify determinism + position sensitivity.
        for &(x, y) in &[(0usize, 0usize), (1, 0), (0, 1), (1919, 1079), (123, 456)] {
            let expected = (crate::vhash32((x as u64) * 2 + y as u64) & 0xFF) as u8;
            assert_eq!(nv12_pattern_y(x, y), expected);
        }
        assert_ne!(nv12_pattern_y(5, 7), nv12_pattern_y(7, 5));
    }

    #[test]
    fn generate_nv12_fills_y_and_neutral_chroma() {
        let (width, height, pitch) = (16u32, 8u32, 32u32); // pitch pads rows
        let mut buf = vec![0u8; (pitch * height * 3 / 2) as usize];
        generate_nv12(width, height, pitch, &mut buf);
        // Luma: every live pixel matches the oracle.
        for y in 0..height as usize {
            for x in 0..width as usize {
                assert_eq!(buf[y * pitch as usize + x], nv12_pattern_y(x, y));
            }
        }
        // Chroma: interleaved UV rows are all neutral 128.
        for row in height as usize..height as usize + height as usize / 2 {
            for &b in &buf[row * pitch as usize..row * pitch as usize + width as usize] {
                assert_eq!(b, 128);
            }
        }
        // Padding between width and pitch stays untouched (zero here).
        for y in 0..(height + height / 2) as usize {
            for &b in &buf[y * pitch as usize + width as usize..y * pitch as usize + pitch as usize]
            {
                assert_eq!(b, 0);
            }
        }
    }

    #[test]
    fn decoded_frame_crc_distinguishes_content() {
        // The validator's baseline scheme: same bytes -> same CRC, any flip
        // -> different CRC (FNV-1a over the live pixels).
        let frame_a = [1u8, 2, 3, 4, 5];
        let frame_a_again = [1u8, 2, 3, 4, 5];
        let frame_b = [1u8, 2, 3, 4, 6];
        assert_eq!(fnv1a64(&frame_a), fnv1a64(&frame_a_again));
        assert_ne!(fnv1a64(&frame_a), fnv1a64(&frame_b));
        assert_eq!(fnv1a64(&[]), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn bitstream_ring_wraps_and_counts_drops() {
        let mut ring = BitstreamRing::new(3);
        assert!(ring.is_empty());
        for i in 0..5u64 {
            ring.push(&[i as u8; 8]);
        }
        // Capacity 3, five pushes -> the two oldest were overwritten.
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.dropped(), 2);
        let mut out = Vec::new();
        assert!(ring.pop_oldest(&mut out));
        assert_eq!(out, [2u8; 8]);
        assert!(ring.pop_oldest(&mut out));
        assert_eq!(out, [3u8; 8]);
        assert!(ring.pop_oldest(&mut out));
        assert_eq!(out, [4u8; 8]);
        assert!(!ring.pop_oldest(&mut out), "ring must be empty now");
        assert_eq!(ring.len(), 0);
        // Pop frees capacity: the next push is not a drop.
        ring.push(&[42u8; 4]);
        assert_eq!(ring.dropped(), 2);
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn bitstream_ring_single_capacity_always_keeps_newest() {
        let mut ring = BitstreamRing::new(1);
        ring.push(&[1u8]);
        ring.push(&[2u8]);
        ring.push(&[3u8]);
        assert_eq!(ring.dropped(), 2);
        let mut out = Vec::new();
        assert!(ring.pop_oldest(&mut out));
        assert_eq!(out, [3u8]);
    }

    #[test]
    fn video_config_dims_round_to_even_and_reject_tiny() {
        let wide = VideoConfig {
            width: 1921,
            height: 1081,
            ..VideoConfig::default()
        };
        assert_eq!(wide.resolved_dims().unwrap(), (1920, 1080));
        let tiny = VideoConfig {
            width: 32,
            height: 32,
            ..VideoConfig::default()
        };
        assert!(tiny.resolved_dims().is_err());
        let odd = VideoConfig {
            width: 65, // rounds to 64
            height: 70,
            ..VideoConfig::default()
        };
        assert_eq!(odd.resolved_dims().unwrap(), (64, 70));
    }

    #[test]
    fn video_config_preset_parsing() {
        assert_eq!(VideoConfig::parse_preset("P3").unwrap(), "p3");
        assert_eq!(VideoConfig::parse_preset(" p7 ").unwrap(), "p7");
        assert!(VideoConfig::parse_preset("p8").is_err());
        assert!(VideoConfig::parse_preset("fast").is_err());
        for p in ["p1", "p2", "p3", "p4", "p5", "p6", "p7"] {
            assert!(VideoConfig::parse_preset(p).is_ok());
        }
    }

    #[test]
    fn video_stats_serialize_matches_verdict_keys() {
        let stats = VideoStats {
            encoded_frames: 12,
            decoded_frames: 11,
            crc_mismatches: 0,
            encode_mbps: 123.5,
            decode_fps: 60.0,
            elapsed_s: 2.0,
        };
        let json = serde_json::to_value(&stats).unwrap();
        for key in [
            "encoded_frames",
            "decoded_frames",
            "crc_mismatches",
            "encode_mbps",
            "decode_fps",
            "elapsed_s",
        ] {
            assert!(json.get(key).is_some(), "missing verdict key {key}");
        }
    }

    #[test]
    fn video_engine_defaults_are_sane() {
        let config = VideoConfig::default();
        assert_eq!(config.resolved_dims().unwrap(), (1920, 1080));
        assert!(config.validate);
        assert!(VideoConfig::parse_preset(&config.preset).is_ok());
    }

    /// Live-hardware smoke run (NVENC + NVDEC + CUDA on device 0). Ignored
    /// by default per repo policy; run with
    /// `cargo test -p cli-stressor-rs --lib -- --ignored video_pipeline_smoke --nocapture`.
    #[test]
    #[ignore = "requires NVENC/NVDEC hardware and driver video libraries"]
    fn video_pipeline_smoke() {
        let mut engine = VideoEngine::new(VideoConfig {
            device_index: 0,
            device_is_pci_sorted: true,
            width: 320,
            height: 240,
            validate: true,
            bitrate_bps: None,
            preset: "p1".to_string(),
            fps: 60,
        })
        .expect("video pipeline must initialize on NVENC/NVDEC hardware");
        let stats = engine.run_for_duration(2.0).expect("run must succeed");
        assert!(stats.encoded_frames > 0, "encoder produced no frames");
        assert!(stats.decoded_frames > 0, "decoder produced no frames");
        assert_eq!(
            stats.crc_mismatches, 0,
            "deterministic loop must validate clean"
        );
    }
}
