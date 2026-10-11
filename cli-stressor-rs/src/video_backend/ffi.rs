//! Hand-written minimal FFI for the driver-shipped NVENC / NVDEC / CUDA
//! libraries (`nvEncodeAPI64.dll` / `libnvidia-encode.so.1`,
//! `nvcuvid.dll` / `libnvcuvid.so.1`, `nvcuda.dll` / `libcuda.so.1`).
//!
//! Ground truth is the ffnvcodec SDK 13.1 headers (`nvEncodeAPI.h`,
//! `dynlink_cuviddec.h`, `dynlink_nvcuvid.h`): every struct below is
//! field-for-field that header's layout, and the version macros are computed
//! with the same arithmetic the C preprocessor applies. The `tests` module at
//! the bottom pins `size_of`/`offset_of` against values produced by compiling
//! a C probe against the real headers (MinGW x64), so a header bump that
//! changes any layout fails loudly here instead of corrupting driver memory.
//!
//! Deliberately NOT bound: MEOnly / LookaheadPicture paths, MV buffers,
//! HEVC/AV1 codec configs (smaller than the H264 member they share a union
//! with — a `reserved` array keeps the union at the true size), async event
//! registration (we run synchronous mode), and the cuvid *source* APIs
//! (we feed the parser packets ourselves).

#![allow(dead_code)]

use std::os::raw::c_void;

// ---------------------------------------------------------------------------
// Library names (dynlink_loader.h NVCUVID_LIBNAME / NVENC_LIBNAME / CUDA_LIBNAME)
// ---------------------------------------------------------------------------

#[cfg(windows)]
pub const NVENC_LIB: &[u8] = b"nvEncodeAPI64.dll\0";
#[cfg(all(not(windows), target_os = "linux"))]
pub const NVENC_LIB: &[u8] = b"libnvidia-encode.so.1\0";
#[cfg(windows)]
pub const NVCUVID_LIB: &[u8] = b"nvcuvid.dll\0";
#[cfg(all(not(windows), target_os = "linux"))]
pub const NVCUVID_LIB: &[u8] = b"libnvcuvid.so.1\0";
#[cfg(windows)]
pub const CUDA_LIB: &[u8] = b"nvcuda.dll\0";
#[cfg(all(not(windows), target_os = "linux"))]
pub const CUDA_LIB: &[u8] = b"libcuda.so.1\0";

// ---------------------------------------------------------------------------
// Version macros (nvEncodeAPI.h / dynlink_cuviddec.h verbatim arithmetic)
// ---------------------------------------------------------------------------

/// `NVENCAPI_VERSION` = `NVENCAPI_MAJOR_VERSION | (NVENCAPI_MINOR_VERSION << 24)`.
pub const NVENCAPI_MAJOR_VERSION: u32 = 13;
pub const NVENCAPI_MINOR_VERSION: u32 = 1;
pub const NVENCAPI_VERSION: u32 = NVENCAPI_MAJOR_VERSION | (NVENCAPI_MINOR_VERSION << 24);

/// `NVENCAPI_STRUCT_VERSION(ver)` = `NVENCAPI_VERSION | (ver << 16) | (0x7 << 28)`.
pub const fn nvencapi_struct_version(ver: u32) -> u32 {
    NVENCAPI_VERSION | (ver << 16) | (0x7 << 28)
}

/// `NvEncodeAPIGetMaxSupportedVersion` packs the answer as `major << 4 | minor`.
pub const fn nvencapi_packed_version(major: u32, minor: u32) -> u32 {
    (major << 4) | minor
}

// Per-structure versions, exactly as #defined in nvEncodeAPI.h. The five with
// `(1u << 31)` carry the same high bit the C macro applies.
pub const NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER: u32 = nvencapi_struct_version(1);
pub const NV_ENCODE_API_FUNCTION_LIST_VER: u32 = nvencapi_struct_version(2);
pub const NV_ENC_CREATE_INPUT_BUFFER_VER: u32 = nvencapi_struct_version(2);
pub const NV_ENC_CREATE_BITSTREAM_BUFFER_VER: u32 = nvencapi_struct_version(1);
pub const NV_ENC_RC_PARAMS_VER: u32 = nvencapi_struct_version(1);
pub const NV_ENC_LOCK_INPUT_BUFFER_VER: u32 = nvencapi_struct_version(1);
pub const NV_ENC_CONFIG_VER: u32 = nvencapi_struct_version(9) | (1u32 << 31);
pub const NV_ENC_INITIALIZE_PARAMS_VER: u32 = nvencapi_struct_version(7) | (1u32 << 31);
pub const NV_ENC_PRESET_CONFIG_VER: u32 = nvencapi_struct_version(5) | (1u32 << 31);
pub const NV_ENC_PIC_PARAMS_VER: u32 = nvencapi_struct_version(7) | (1u32 << 31);
pub const NV_ENC_LOCK_BITSTREAM_VER: u32 = nvencapi_struct_version(2) | (1u32 << 31);
pub const NV_ENC_REGISTER_RESOURCE_VER: u32 = nvencapi_struct_version(5);
pub const NV_ENC_MAP_INPUT_RESOURCE_VER: u32 = nvencapi_struct_version(4);

/// `NVENC_INFINITE_GOPLENGTH`.
pub const NVENC_INFINITE_GOPLENGTH: u32 = 0xffff_ffff;

// ---------------------------------------------------------------------------
// Enum values actually used (values verbatim from the header enums)
// ---------------------------------------------------------------------------

/// `_NVENCSTATUS` (values are the enum discriminants).
pub mod nvenc_status {
    pub const NV_ENC_SUCCESS: u32 = 0;
    pub const NV_ENC_ERR_NO_ENCODE_DEVICE: u32 = 1;
    pub const NV_ENC_ERR_UNSUPPORTED_DEVICE: u32 = 2;
    pub const NV_ENC_ERR_INVALID_ENCODERDEVICE: u32 = 3;
    pub const NV_ENC_ERR_INVALID_DEVICE: u32 = 4;
    pub const NV_ENC_ERR_DEVICE_NOT_EXIST: u32 = 5;
    pub const NV_ENC_ERR_INVALID_PTR: u32 = 6;
    pub const NV_ENC_ERR_INVALID_EVENT: u32 = 7;
    pub const NV_ENC_ERR_INVALID_PARAM: u32 = 8;
    pub const NV_ENC_ERR_INVALID_CALL: u32 = 9;
    pub const NV_ENC_ERR_OUT_OF_MEMORY: u32 = 10;
    pub const NV_ENC_ERR_ENCODER_NOT_INITIALIZED: u32 = 11;
    pub const NV_ENC_ERR_UNSUPPORTED_PARAM: u32 = 12;
    pub const NV_ENC_ERR_LOCK_BUSY: u32 = 13;
    pub const NV_ENC_ERR_NOT_ENOUGH_BUFFER: u32 = 14;
    pub const NV_ENC_ERR_INVALID_VERSION: u32 = 15;
    pub const NV_ENC_ERR_MAP_FAILED: u32 = 16;
    pub const NV_ENC_ERR_NEED_MORE_INPUT: u32 = 17;
    pub const NV_ENC_ERR_ENCODER_BUSY: u32 = 18;
    pub const NV_ENC_ERR_EVENT_NOT_REGISTERD: u32 = 19;
    pub const NV_ENC_ERR_GENERIC: u32 = 20;
    pub const NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY: u32 = 21;
    pub const NV_ENC_ERR_UNIMPLEMENTED: u32 = 22;
    pub const NV_ENC_ERR_RESOURCE_REGISTER_FAILED: u32 = 23;
    pub const NV_ENC_ERR_RESOURCE_NOT_REGISTERED: u32 = 24;
    pub const NV_ENC_ERR_RESOURCE_NOT_MAPPED: u32 = 25;

    /// Human-readable name for a `_NVENCSTATUS` code (covers the enum above).
    pub fn name(code: u32) -> &'static str {
        match code {
            NV_ENC_SUCCESS => "NV_ENC_SUCCESS",
            NV_ENC_ERR_NO_ENCODE_DEVICE => "NV_ENC_ERR_NO_ENCODE_DEVICE",
            NV_ENC_ERR_UNSUPPORTED_DEVICE => "NV_ENC_ERR_UNSUPPORTED_DEVICE",
            NV_ENC_ERR_INVALID_ENCODERDEVICE => "NV_ENC_ERR_INVALID_ENCODERDEVICE",
            NV_ENC_ERR_INVALID_DEVICE => "NV_ENC_ERR_INVALID_DEVICE",
            NV_ENC_ERR_DEVICE_NOT_EXIST => "NV_ENC_ERR_DEVICE_NOT_EXIST",
            NV_ENC_ERR_INVALID_PTR => "NV_ENC_ERR_INVALID_PTR",
            NV_ENC_ERR_INVALID_EVENT => "NV_ENC_ERR_INVALID_EVENT",
            NV_ENC_ERR_INVALID_PARAM => "NV_ENC_ERR_INVALID_PARAM",
            NV_ENC_ERR_INVALID_CALL => "NV_ENC_ERR_INVALID_CALL",
            NV_ENC_ERR_OUT_OF_MEMORY => "NV_ENC_ERR_OUT_OF_MEMORY",
            NV_ENC_ERR_ENCODER_NOT_INITIALIZED => "NV_ENC_ERR_ENCODER_NOT_INITIALIZED",
            NV_ENC_ERR_UNSUPPORTED_PARAM => "NV_ENC_ERR_UNSUPPORTED_PARAM",
            NV_ENC_ERR_LOCK_BUSY => "NV_ENC_ERR_LOCK_BUSY",
            NV_ENC_ERR_NOT_ENOUGH_BUFFER => "NV_ENC_ERR_NOT_ENOUGH_BUFFER",
            NV_ENC_ERR_INVALID_VERSION => "NV_ENC_ERR_INVALID_VERSION",
            NV_ENC_ERR_MAP_FAILED => "NV_ENC_ERR_MAP_FAILED",
            NV_ENC_ERR_NEED_MORE_INPUT => "NV_ENC_ERR_NEED_MORE_INPUT",
            NV_ENC_ERR_ENCODER_BUSY => "NV_ENC_ERR_ENCODER_BUSY",
            NV_ENC_ERR_EVENT_NOT_REGISTERD => "NV_ENC_ERR_EVENT_NOT_REGISTERD",
            NV_ENC_ERR_GENERIC => "NV_ENC_ERR_GENERIC",
            NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY => "NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY",
            NV_ENC_ERR_UNIMPLEMENTED => "NV_ENC_ERR_UNIMPLEMENTED",
            NV_ENC_ERR_RESOURCE_REGISTER_FAILED => "NV_ENC_ERR_RESOURCE_REGISTER_FAILED",
            NV_ENC_ERR_RESOURCE_NOT_REGISTERED => "NV_ENC_ERR_RESOURCE_NOT_REGISTERED",
            NV_ENC_ERR_RESOURCE_NOT_MAPPED => "NV_ENC_ERR_RESOURCE_NOT_MAPPED",
            _ => "NV_ENC_ERR_<unknown>",
        }
    }
}

/// `NV_ENC_DEVICE_TYPE_CUDA` (enum `_NV_ENC_DEVICE_TYPE`).
pub const NV_ENC_DEVICE_TYPE_CUDA: u32 = 0x1;
/// `NV_ENC_BUFFER_FORMAT_NV12` (enum `_NV_ENC_BUFFER_FORMAT`).
pub const NV_ENC_BUFFER_FORMAT_NV12: u32 = 0x0000_0001;
/// `NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR` (enum `_NV_ENC_INPUT_RESOURCE_TYPE`).
pub const NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR: u32 = 0x1;
/// `NV_ENC_INPUT_IMAGE` (enum `_NV_ENC_BUFFER_USAGE`).
pub const NV_ENC_INPUT_IMAGE: u32 = 0x0;
/// `NV_ENC_PARAMS_RC_CONSTQP` / `NV_ENC_PARAMS_RC_CBR` (enum `_NV_ENC_PARAMS_RC_MODE`).
pub const NV_ENC_PARAMS_RC_CONSTQP: u32 = 0x0;
pub const NV_ENC_PARAMS_RC_CBR: u32 = 0x2;
/// `NV_ENC_PIC_STRUCT_FRAME` (enum `_NV_ENC_PIC_STRUCT`).
pub const NV_ENC_PIC_STRUCT_FRAME: u32 = 0x01;
/// `NV_ENC_PIC_TYPE_*` (enum `_NV_ENC_PIC_TYPE`).
pub const NV_ENC_PIC_TYPE_P: u32 = 0x0;
pub const NV_ENC_PIC_TYPE_I: u32 = 0x02;
pub const NV_ENC_PIC_TYPE_IDR: u32 = 0x03;
pub const NV_ENC_PIC_TYPE_UNKNOWN: u32 = 0xFF;
/// `NV_ENC_PIC_FLAG_EOS` (enum `_NV_ENC_PIC_FLAGS`).
pub const NV_ENC_PIC_FLAG_EOS: u32 = 0x8;
/// `NV_ENC_TUNING_INFO_UNDEFINED` / `NV_ENC_TUNING_INFO_HIGH_QUALITY`
/// (enum `NV_ENC_TUNING_INFO`).
pub const NV_ENC_TUNING_INFO_UNDEFINED: u32 = 0;
pub const NV_ENC_TUNING_INFO_HIGH_QUALITY: u32 = 1;
/// `NV_ENC_LEVEL_AUTOSELECT` (enum `_NV_ENC_LEVEL`).
pub const NV_ENC_LEVEL_AUTOSELECT: u32 = 0;
/// `NV_ENC_H264_ENTROPY_CODING_MODE_AUTOSELECT` (enum `_NV_ENC_H264_ENTROPY_CODING_MODE`).
pub const NV_ENC_H264_ENTROPY_CODING_MODE_AUTOSELECT: u32 = 0x0;
/// `NV_ENC_MULTI_PASS_DISABLED` (enum `_NV_ENC_MULTI_PASS`).
pub const NV_ENC_MULTI_PASS_DISABLED: u32 = 0x0;
/// `NV_ENC_QP_MAP_DISABLED` (enum `_NV_ENC_QP_MAP_MODE`).
pub const NV_ENC_QP_MAP_DISABLED: u32 = 0x0;
/// `NV_ENC_BFRAME_REF_MODE_DISABLED` (enum `_NV_ENC_BFRAME_REF_MODE`).
pub const NV_ENC_BFRAME_REF_MODE_DISABLED: u32 = 0x0;
/// `NV_ENC_NUM_REF_FRAMES_AUTOSELECT` (enum `_NV_ENC_NUM_REF_FRAMES`).
pub const NV_ENC_NUM_REF_FRAMES_AUTOSELECT: u32 = 0x0;
/// `NV_ENC_BIT_DEPTH_8` (enum `_NV_ENC_BIT_DEPTH`).
pub const NV_ENC_BIT_DEPTH_8: u32 = 8;
/// `NV_ENC_TEMPORAL_FILTER_LEVEL_0` (enum `_NV_ENC_TEMPORAL_FILTER_LEVEL`).
pub const NV_ENC_TEMPORAL_FILTER_LEVEL_0: u32 = 0;
/// `NV_ENC_H264_ADAPTIVE_TRANSFORM_AUTOSELECT` (enum `_NV_ENC_H264_ADAPTIVE_TRANSFORM_MODE`).
pub const NV_ENC_H264_ADAPTIVE_TRANSFORM_AUTOSELECT: u32 = 0x0;
/// `NV_ENC_H264_FMO_AUTOSELECT` (enum `_NV_ENC_H264_FMO_MODE`).
pub const NV_ENC_H264_FMO_AUTOSELECT: u32 = 0x0;
/// `NV_ENC_H264_BDIRECT_MODE_AUTOSELECT` (enum `_NV_ENC_H264_BDIRECT_MODE`).
pub const NV_ENC_H264_BDIRECT_MODE_AUTOSELECT: u32 = 0x0;
/// `NV_ENC_STEREO_PACKING_MODE_NONE` (enum `_NV_ENC_STEREO_PACKING_MODE`).
pub const NV_ENC_STEREO_PACKING_MODE_NONE: u32 = 0x0;
/// `NV_ENC_LOOKAHEAD_LEVEL_AUTOSELECT` (enum `_NV_ENC_LOOKAHEAD_LEVEL`).
pub const NV_ENC_LOOKAHEAD_LEVEL_AUTOSELECT: u32 = 15;
/// `NV_ENC_MEMORY_HEAP_AUTOSELECT` (enum `_NV_ENC_MEMORY_HEAP`).
pub const NV_ENC_MEMORY_HEAP_AUTOSELECT: u32 = 0;
/// `NV_ENC_PARAMS_FRAME_FIELD_MODE_FRAME` (enum `_NV_ENC_PARAMS_FRAME_FIELD_MODE`).
pub const NV_ENC_PARAMS_FRAME_FIELD_MODE_FRAME: u32 = 0x01;
/// `NV_ENC_MV_PRECISION_DEFAULT` (enum `_NV_ENC_MV_PRECISION`).
pub const NV_ENC_MV_PRECISION_DEFAULT: u32 = 0x0;

// ---------------------------------------------------------------------------
// cuvid enum values (dynlink_cuviddec.h)
// ---------------------------------------------------------------------------

/// `cudaVideoCodec_H264` (enum `cudaVideoCodec_enum`).
pub const CUDA_VIDEO_CODEC_H264: i32 = 4;
/// `cudaVideoChromaFormat_420` (enum `cudaVideoChromaFormat_enum`).
pub const CUDA_VIDEO_CHROMA_FORMAT_420: i32 = 1;
/// `cudaVideoSurfaceFormat_NV12` (enum `cudaVideoSurfaceFormat_enum`).
pub const CUDA_VIDEO_SURFACE_FORMAT_NV12: i32 = 0;
/// `cudaVideoDeinterlaceMode_Weave` (enum `cudaVideoDeinterlaceMode_enum`).
pub const CUDA_VIDEO_DEINTERLACE_MODE_WEAVE: i32 = 0;
/// `cudaVideoCreate_Default` (enum `cudaVideoCreateFlags_enum`).
pub const CUDA_VIDEO_CREATE_DEFAULT: i32 = 0x00;

/// `CUVID_PKT_ENDOFPICTURE` (enum `CUvideopacketflags`).
pub const CUVID_PKT_ENDOFPICTURE: TcuUlong = 0x08;
/// `CUVID_PKT_ENDOFSTREAM` (enum `CUvideopacketflags`).
pub const CUVID_PKT_ENDOFSTREAM: TcuUlong = 0x01;

// ---------------------------------------------------------------------------
// Base types
// ---------------------------------------------------------------------------

/// `tcu_ulong`: `unsigned long` in C — 32-bit on Windows, 64-bit on LP64
/// Linux. Several cuvid struct layouts depend on this width.
#[cfg(windows)]
pub type TcuUlong = u32;
#[cfg(all(not(windows), target_os = "linux"))]
pub type TcuUlong = u64;

pub type CUresult = i32;
pub type CUdevice = i32;
pub type CUdeviceptr = u64;
pub type CUcontext = *mut c_void;
pub type CUstream = *mut c_void;
pub type CUvideodecoder = *mut c_void;
pub type CUvideoparser = *mut c_void;
/// `CUvideotimestamp` = `long long`.
pub type CUvideotimestamp = i64;

/// Win32-style GUID layout (also how nvEncodeAPI.h defines GUID on
/// non-Windows platforms; x64 Windows passes GUIDs by pointer/inline value
/// with this exact byte layout either way).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

impl Guid {
    pub const fn from_parts(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Self {
        Self {
            data1,
            data2,
            data3,
            data4,
        }
    }
}

pub const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> Guid {
    Guid::from_parts(data1, data2, data3, data4)
}

// ---------------------------------------------------------------------------
// GUIDs (values verbatim from nvEncodeAPI.h)
// ---------------------------------------------------------------------------

/// `NV_ENC_CODEC_H264_GUID` {6BC82762-4E63-4CA4-AA85-1E50F321F6BF}
pub const NV_ENC_CODEC_H264_GUID: Guid = guid(
    0x6bc8_2762,
    0x4e63,
    0x4ca4,
    [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf],
);
/// `NV_ENC_CODEC_PROFILE_AUTOSELECT_GUID` {BFD6F8E7-233C-4341-8B3E-4818523803F4}
pub const NV_ENC_CODEC_PROFILE_AUTOSELECT_GUID: Guid = guid(
    0xbfd6_f8e7,
    0x233c,
    0x4341,
    [0x8b, 0x3e, 0x48, 0x18, 0x52, 0x38, 0x03, 0xf4],
);
/// `NV_ENC_H264_PROFILE_MAIN_GUID` {60B5C1D4-67FE-4790-94D5-C4726D7B6E6D}
pub const NV_ENC_H264_PROFILE_MAIN_GUID: Guid = guid(
    0x60b5_c1d4,
    0x67fe,
    0x4790,
    [0x94, 0xd5, 0xc4, 0x72, 0x6d, 0x7b, 0x6e, 0x6d],
);
/// `NV_ENC_PRESET_P1_GUID` {FC0A8D3E-45F8-4CF8-80C7-298871590EBF}
pub const NV_ENC_PRESET_P1_GUID: Guid = guid(
    0xfc0a_8d3e,
    0x45f8,
    0x4cf8,
    [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf],
);
/// `NV_ENC_PRESET_P2_GUID` {F581CFB8-88D6-4381-93F0-DF13F9C27DAB}
pub const NV_ENC_PRESET_P2_GUID: Guid = guid(
    0xf581_cfb8,
    0x88d6,
    0x4381,
    [0x93, 0xf0, 0xdf, 0x13, 0xf9, 0xc2, 0x7d, 0xab],
);
/// `NV_ENC_PRESET_P3_GUID` {36850110-3A07-441F-94D5-3670631F91F6}
pub const NV_ENC_PRESET_P3_GUID: Guid = guid(
    0x3685_0110,
    0x3a07,
    0x441f,
    [0x94, 0xd5, 0x36, 0x70, 0x63, 0x1f, 0x91, 0xf6],
);
/// `NV_ENC_PRESET_P4_GUID` {90A7B826-DF06-4862-B9D2-CD6D73A08681}
pub const NV_ENC_PRESET_P4_GUID: Guid = guid(
    0x90a7_b826,
    0xdf06,
    0x4862,
    [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81],
);
/// `NV_ENC_PRESET_P5_GUID` {21C6E6B4-297A-4CBA-998F-B6CBDE72ADE3}
pub const NV_ENC_PRESET_P5_GUID: Guid = guid(
    0x21c6_e6b4,
    0x297a,
    0x4cba,
    [0x99, 0x8f, 0xb6, 0xcb, 0xde, 0x72, 0xad, 0xe3],
);
/// `NV_ENC_PRESET_P6_GUID` {8E75C279-6299-4AB6-8302-0B215A335CF5}
pub const NV_ENC_PRESET_P6_GUID: Guid = guid(
    0x8e75_c279,
    0x6299,
    0x4ab6,
    [0x83, 0x02, 0x0b, 0x21, 0x5a, 0x33, 0x5c, 0xf5],
);
/// `NV_ENC_PRESET_P7_GUID` {84848C12-6F71-4C13-931B-53E283F57974}
pub const NV_ENC_PRESET_P7_GUID: Guid = guid(
    0x8484_8c12,
    0x6f71,
    0x4c13,
    [0x93, 0x1b, 0x53, 0xe2, 0x83, 0xf5, 0x79, 0x74],
);

/// Map a `--video-preset p1..p7` name to its `NV_ENC_PRESET_Px_GUID`.
pub fn preset_guid(name: &str) -> Option<Guid> {
    match name {
        "p1" => Some(NV_ENC_PRESET_P1_GUID),
        "p2" => Some(NV_ENC_PRESET_P2_GUID),
        "p3" => Some(NV_ENC_PRESET_P3_GUID),
        "p4" => Some(NV_ENC_PRESET_P4_GUID),
        "p5" => Some(NV_ENC_PRESET_P5_GUID),
        "p6" => Some(NV_ENC_PRESET_P6_GUID),
        "p7" => Some(NV_ENC_PRESET_P7_GUID),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// NVENC structures (nvEncodeAPI.h, field-for-field)
// ---------------------------------------------------------------------------

/// `NV_ENC_QP`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NV_ENC_QP {
    pub qp_inter_p: u32,
    pub qp_inter_b: u32,
    pub qp_intra: u32,
}

/// `NV_ENC_RC_PARAMS` (rate-control configuration). The 15 named bitfields +
/// `aqStrength:4` + `reservedBitFields:15` pack into exactly one `u32` in the
/// C layout (probe: `minQP` sits at offset 40).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NV_ENC_RC_PARAMS {
    pub version: u32,
    pub rate_control_mode: u32,
    pub const_qp: NV_ENC_QP,
    pub average_bit_rate: u32,
    pub max_bit_rate: u32,
    pub vbv_buffer_size: u32,
    pub vbv_initial_delay: u32,
    /// `enableMinQP:1 | enableMaxQP:1 | enableInitialRCQP:1 | enableAQ:1 |
    /// reservedBitField1:1 | enableLookahead:1 | disableIadapt:1 |
    /// disableBadapt:1 | enableTemporalAQ:1 | zeroReorderDelay:1 |
    /// enableNonRefP:1 | strictGOPTarget:1 | aqStrength:4 |
    /// enableExtLookahead:1 | reservedBitFields:15`
    pub bitfields: u32,
    pub min_qp: NV_ENC_QP,
    pub max_qp: NV_ENC_QP,
    pub initial_rc_qp: NV_ENC_QP,
    pub temporal_layer_idx_mask: u32,
    pub temporal_layer_qp: [u8; 8],
    pub target_quality: u8,
    pub target_quality_lsb: u8,
    pub lookahead_depth: u16,
    pub low_delay_key_frame_scale: u8,
    pub y_dc_qp_index_offset: i8,
    pub u_dc_qp_index_offset: i8,
    pub v_dc_qp_index_offset: i8,
    pub qp_map_mode: u32,
    pub multi_pass: u32,
    pub alpha_layer_bitrate_ratio: u32,
    pub cb_qp_index_offset: i8,
    pub cr_qp_index_offset: i8,
    pub reserved2: u16,
    pub lookahead_level: u32,
    pub view_bitrate_ratios: [u8; 7],
    pub reserved3: u8,
    pub reserved1: u32,
}

/// `NV_ENC_CONFIG_H264_VUI_PARAMETERS` (also aliased as the HEVC VUI params).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NV_ENC_CONFIG_H264_VUI_PARAMETERS {
    pub overscan_info_present_flag: u32,
    pub overscan_info: u32,
    pub video_signal_type_present_flag: u32,
    pub video_format: u32,
    pub video_full_range_flag: u32,
    pub colour_description_present_flag: u32,
    pub colour_primaries: u32,
    pub transfer_characteristics: u32,
    pub colour_matrix: u32,
    pub chroma_sample_location_flag: u32,
    pub chroma_sample_location_top: u32,
    pub chroma_sample_location_bot: u32,
    pub bitstream_restriction_flag: u32,
    pub timing_info_present_flag: u32,
    pub num_unit_in_ticks: u32,
    pub time_scale: u32,
    pub reserved: [u32; 12],
}

/// `NV_ENC_CONFIG_H264`. The 22 named bitfields + `reservedBitFields:10` pack
/// into one `u32` (probe: `level` at offset 4).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_CONFIG_H264 {
    /// `enableTemporalSVC:1 | … | enableTimeCode:1 | reservedBitFields:10`
    /// (22 feature bits then 10 reserved; all zero-initialized is valid).
    pub bitfields: u32,
    pub level: u32,
    pub idr_period: u32,
    pub separate_colour_plane_flag: u32,
    pub disable_deblocking_filter_idc: u32,
    pub num_temporal_layers: u32,
    pub sps_id: u32,
    pub pps_id: u32,
    pub adaptive_transform_mode: u32,
    pub fmo_mode: u32,
    pub bdirect_mode: u32,
    pub entropy_coding_mode: u32,
    pub stereo_mode: u32,
    pub intra_refresh_period: u32,
    pub intra_refresh_cnt: u32,
    pub max_num_ref_frames: u32,
    pub slice_mode: u32,
    pub slice_mode_data: u32,
    pub h264_vui_parameters: NV_ENC_CONFIG_H264_VUI_PARAMETERS,
    pub ltr_num_frames: u32,
    pub ltr_trust_mode: u32,
    pub chroma_format_idc: u32,
    pub max_temporal_layers: u32,
    pub use_bframes_as_ref: u32,
    pub num_ref_l0: u32,
    pub num_ref_l1: u32,
    pub output_bit_depth: u32,
    pub input_bit_depth: u32,
    pub tf_level: u32,
    pub reserved1: [u32; 264],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_CONFIG_H264 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// Opaque `Debug` for repr(C) driver structs that contain unions (Rust's
/// derive(Debug) requires every member to be Debug; the union members hold
/// no extra meaning for us beyond their H264 view, which we handle manually
/// where it matters).
macro_rules! opaque_debug {
    ($($t:ty),* $(,)?) => {
        $(
            impl std::fmt::Debug for $t {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(stringify!($t))
                }
            }
        )*
    };
}

opaque_debug!(
    NV_ENC_CODEC_CONFIG,
    NV_ENC_PIC_PARAMS_H264,
    NV_ENC_PIC_PARAMS,
    CUVIDH264PICPARAMS,
    CUVIDPICPARAMS,
);

/// `_NV_ENC_CODEC_CONFIG` union. The H264 member is the largest (1792 B,
/// probe-verified; HEVC 1556 B, AV1/MEONLY smaller), so a `reserved` array
/// smaller than that cannot shrink the union below the real size.
#[repr(C)]
#[derive(Clone, Copy)]
pub union NV_ENC_CODEC_CONFIG {
    pub h264_config: NV_ENC_CONFIG_H264,
    pub reserved: [u32; 320],
}

impl Default for NV_ENC_CODEC_CONFIG {
    fn default() -> Self {
        NV_ENC_CODEC_CONFIG {
            reserved: [0u32; 320],
        }
    }
}

/// `NV_ENC_CONFIG` (encoder configuration; probe-verified 3584 B).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_CONFIG {
    pub version: u32,
    pub profile_guid: Guid,
    pub gop_length: u32,
    pub frame_interval_p: i32,
    pub monochrome_encoding: u32,
    pub frame_field_mode: u32,
    pub mv_precision: u32,
    pub rc_params: NV_ENC_RC_PARAMS,
    pub encode_codec_config: NV_ENC_CODEC_CONFIG,
    pub reserved: [u32; 278],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_CONFIG {
    fn default() -> Self {
        // repr(C) with no drop glue: zeroed memory is the driver-documented
        // initialization state (every reserved field must be 0/NULL).
        unsafe { std::mem::zeroed() }
    }
}

/// `NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE` (bitfield + reserved1[3]).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE {
    /// `numCandsPerBlk16x16:4 | numCandsPerBlk16x8:4 | numCandsPerBlk8x16:4 |
    /// numCandsPerBlk8x8:4 | numCandsPerSb:8 | reserved:8`
    pub bitfields: u32,
    pub reserved1: [u32; 3],
}

/// `NV_ENC_INITIALIZE_PARAMS` (encode session initialization; 1800 B).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_INITIALIZE_PARAMS {
    pub version: u32,
    pub encode_guid: Guid,
    pub preset_guid: Guid,
    pub encode_width: u32,
    pub encode_height: u32,
    pub dar_width: u32,
    pub dar_height: u32,
    pub frame_rate_num: u32,
    pub frame_rate_den: u32,
    pub enable_encode_async: u32,
    pub enable_ptd: u32,
    /// `reportSliceOffsets:1 | enableSubFrameWrite:1 | enableExternalMEHints:1 |
    /// enableMEOnlyMode:1 | enableWeightedPrediction:1 | splitEncodeMode:4 |
    /// enableOutputInVidmem:1 | enableReconFrameOutput:1 | enableOutputStats:1 |
    /// enableUniDirectionalB:1 | reservedBitFields:19`
    pub bitfields: u32,
    pub priv_data_size: u32,
    pub reserved: u32,
    pub priv_data: *mut c_void,
    pub encode_config: *mut NV_ENC_CONFIG,
    pub max_encode_width: u32,
    pub max_encode_height: u32,
    pub max_me_hint_counts_per_block: [NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE; 2],
    pub tuning_info: u32,
    pub buffer_format: u32,
    pub num_state_buffers: u32,
    pub output_stats_level: u32,
    pub reserved1: [u32; 284],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_INITIALIZE_PARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_PIC_PARAMS_MVC` (kept for the H264 ext union layout).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_PIC_PARAMS_MVC {
    pub version: u32,
    pub view_id: u32,
    pub temporal_id: u32,
    pub priority_id: u32,
    pub reserved1: [u32; 12],
    pub reserved2: [*mut c_void; 8],
}

impl Default for NV_ENC_PIC_PARAMS_MVC {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_PIC_PARAMS_H264_EXT` union (MVC params or 32 reserved words).
#[repr(C)]
#[derive(Clone, Copy)]
pub union NV_ENC_PIC_PARAMS_H264_EXT {
    pub mvc_pic_params: NV_ENC_PIC_PARAMS_MVC,
    pub reserved1: [u32; 32],
}

impl Default for NV_ENC_PIC_PARAMS_H264_EXT {
    fn default() -> Self {
        NV_ENC_PIC_PARAMS_H264_EXT {
            reserved1: [0u32; 32],
        }
    }
}

/// `NV_ENC_SEI_PAYLOAD`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_SEI_PAYLOAD {
    pub payload_size: u32,
    pub payload_type: u32,
    pub payload: *mut u8,
}

/// `NV_ENC_CLOCK_TIMESTAMP_SET` (bitfield + timeOffset).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NV_ENC_CLOCK_TIMESTAMP_SET {
    /// `countingTypeLSB:1 | discontinuityFlag:1 | cntDroppedFrames:1 |
    /// nFrames:8 | secondsValue:6 | minutesValue:6 | hoursValue:5 |
    /// countingTypeMSB:4`
    pub bitfields: u32,
    pub time_offset: u32,
}

/// `NV_ENC_TIME_CODE`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NV_ENC_TIME_CODE {
    pub display_pic_struct: u32,
    pub clock_timestamp: [NV_ENC_CLOCK_TIMESTAMP_SET; 3],
    pub skip_clock_timestamp_insertion: u32,
}

/// `NV_ENC_PIC_PARAMS_H264` (per-frame codec picture parameters).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NV_ENC_PIC_PARAMS_H264 {
    pub display_poc_syntax: u32,
    pub reserved3: u32,
    pub ref_pic_flag: u32,
    pub colour_plane_id: u32,
    pub force_intra_refresh_with_frame_cnt: u32,
    /// `constrainedFrame:1 | sliceModeDataUpdate:1 | ltrMarkFrame:1 |
    /// ltrUseFrames:1 | reservedBitFields:28`
    pub bitfields: u32,
    pub slice_type_data: *mut u8,
    pub slice_type_array_cnt: u32,
    pub sei_payload_array_cnt: u32,
    pub sei_payload_array: *mut NV_ENC_SEI_PAYLOAD,
    pub slice_mode: u32,
    pub slice_mode_data: u32,
    pub ltr_mark_frame_idx: u32,
    pub ltr_use_frame_bitmap: u32,
    pub ltr_usage_mode: u32,
    pub force_intra_slice_count: u32,
    pub force_intra_slice_idx: *mut u32,
    pub h264_ext_pic_params: NV_ENC_PIC_PARAMS_H264_EXT,
    pub time_code: NV_ENC_TIME_CODE,
    pub reserved: [u32; 202],
    pub reserved2: [*mut c_void; 61],
}

impl Default for NV_ENC_PIC_PARAMS_H264 {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_CODEC_PIC_PARAMS` union. The probe-verified size is 1544 B (the
/// HEVC member is the largest at 1544 B, H264 is 1536 B), so the `reserved`
/// member is sized 386 u32 = 1544 B to keep our NV_ENC_PIC_PARAMS at the true
/// driver layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub union NV_ENC_CODEC_PIC_PARAMS {
    pub h264_pic_params: NV_ENC_PIC_PARAMS_H264,
    pub reserved: [u32; 386],
}

impl Default for NV_ENC_CODEC_PIC_PARAMS {
    fn default() -> Self {
        NV_ENC_CODEC_PIC_PARAMS {
            reserved: [0u32; 386],
        }
    }
}

/// `NV_ENC_PIC_PARAMS` (per-frame encode parameters; 3360 B).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NV_ENC_PIC_PARAMS {
    pub version: u32,
    pub input_width: u32,
    pub input_height: u32,
    pub input_pitch: u32,
    pub encode_pic_flags: u32,
    pub frame_idx: u32,
    pub input_time_stamp: u64,
    pub input_duration: u64,
    pub input_buffer: *mut c_void,
    pub output_bitstream: *mut c_void,
    pub completion_event: *mut c_void,
    pub buffer_fmt: u32,
    pub picture_struct: u32,
    pub picture_type: u32,
    pub codec_pic_params: NV_ENC_CODEC_PIC_PARAMS,
    pub me_hint_counts_per_block: [NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE; 2],
    pub me_external_hints: *mut c_void,
    pub reserved2: [u32; 7],
    pub reserved5: [*mut c_void; 2],
    pub qp_delta_map: *mut i8,
    pub qp_delta_map_size: u32,
    pub reserved_bit_fields: u32,
    pub me_hint_ref_pic_dist: [u16; 2],
    pub diff_pic_num_hint: i32,
    pub alpha_buffer: *mut c_void,
    pub me_external_sb_hints: *mut c_void,
    pub me_sb_hints_count: u32,
    pub state_buffer_idx: u32,
    pub output_recon_buffer: *mut c_void,
    pub reserved3: [u32; 284],
    pub reserved6: [*mut c_void; 57],
}

impl Default for NV_ENC_PIC_PARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_LOCK_BITSTREAM` (1544 B; `bitstreamBufferPtr` is the data pointer
/// to copy the encoded frame from).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_LOCK_BITSTREAM {
    pub version: u32,
    /// `doNotWait:1 | ltrFrame:1 | getRCStats:1 | reservedBitFields:29`
    pub bitfields: u32,
    pub output_bitstream: *mut c_void,
    pub slice_offsets: *mut u32,
    pub frame_idx: u32,
    pub hw_encode_status: u32,
    pub num_slices: u32,
    pub bitstream_size_in_bytes: u32,
    pub output_time_stamp: u64,
    pub output_duration: u64,
    pub bitstream_buffer_ptr: *mut c_void,
    pub picture_type: u32,
    pub picture_struct: u32,
    pub frame_avg_qp: u32,
    pub frame_satd: u32,
    pub ltr_frame_idx: u32,
    pub ltr_frame_bitmap: u32,
    pub temporal_id: u32,
    pub intra_mb_count: u32,
    pub inter_mb_count: u32,
    pub average_mv_x: i32,
    pub average_mv_y: i32,
    pub alpha_layer_size_in_bytes: u32,
    pub output_stats_ptr_size: u32,
    pub reserved: u32,
    pub output_stats_ptr: *mut c_void,
    pub frame_idx_display: u32,
    pub reserved1: [u32; 219],
    pub reserved2: [*mut c_void; 63],
    pub reserved_internal: [u32; 8],
}

impl Default for NV_ENC_LOCK_BITSTREAM {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_LOCK_INPUT_BUFFER` (sysmem input path: the locked
/// `bufferDataPtr` is where the NV12 frame is written).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_LOCK_INPUT_BUFFER {
    pub version: u32,
    /// `doNotWait:1 | reservedBitFields:31`
    pub bitfields: u32,
    pub input_buffer: *mut c_void,
    pub buffer_data_ptr: *mut c_void,
    pub pitch: u32,
    pub reserved1: [u32; 251],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_LOCK_INPUT_BUFFER {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_CREATE_INPUT_BUFFER`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_CREATE_INPUT_BUFFER {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub memory_heap: u32,
    pub buffer_fmt: u32,
    pub reserved: u32,
    pub input_buffer: *mut c_void,
    pub p_sys_mem_buffer: *mut c_void,
    pub reserved1: [u32; 58],
    pub reserved2: [*mut c_void; 63],
}

impl Default for NV_ENC_CREATE_INPUT_BUFFER {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_CREATE_BITSTREAM_BUFFER`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_CREATE_BITSTREAM_BUFFER {
    pub version: u32,
    pub size: u32,
    pub memory_heap: u32,
    pub reserved: u32,
    pub bitstream_buffer: *mut c_void,
    pub bitstream_buffer_ptr: *mut c_void,
    pub reserved1: [u32; 58],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_CREATE_BITSTREAM_BUFFER {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_REGISTER_RESOURCE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_REGISTER_RESOURCE {
    pub version: u32,
    pub resource_type: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub sub_resource_index: u32,
    pub resource_to_register: *mut c_void,
    pub registered_resource: *mut c_void,
    pub buffer_format: u32,
    pub buffer_usage: u32,
    pub p_input_fence_point: *mut c_void,
    pub chroma_offset: [u32; 2],
    pub chroma_offset_in: [u32; 2],
    pub reserved1: [u32; 244],
    pub reserved2: [*mut c_void; 61],
}

impl Default for NV_ENC_REGISTER_RESOURCE {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_MAP_INPUT_RESOURCE`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_MAP_INPUT_RESOURCE {
    pub version: u32,
    pub sub_resource_index: u32,
    pub input_resource: *mut c_void,
    pub registered_resource: *mut c_void,
    pub mapped_resource: *mut c_void,
    pub mapped_buffer_fmt: u32,
    pub reserved1: [u32; 251],
    pub reserved2: [*mut c_void; 63],
}

impl Default for NV_ENC_MAP_INPUT_RESOURCE {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_PRESET_CONFIG` (returned by `nvEncGetEncodePresetConfigEx`;
/// embeds a full `NV_ENC_CONFIG`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_PRESET_CONFIG {
    pub version: u32,
    pub reserved: u32,
    pub preset_cfg: NV_ENC_CONFIG,
    pub reserved1: [u32; 256],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_PRESET_CONFIG {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
    pub version: u32,
    pub device_type: u32,
    pub device: *mut c_void,
    pub reserved: *mut c_void,
    pub api_version: u32,
    pub reserved1: [u32; 253],
    pub reserved2: [*mut c_void; 64],
}

impl Default for NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

// ---------------------------------------------------------------------------
// NV_ENCODE_API_FUNCTION_LIST: 46 typed entries + reserved2[275], in the
// exact header order.
// ---------------------------------------------------------------------------

pub type PnvEncOpenEncodeSession = unsafe extern "system" fn(
    device: *mut c_void,
    device_type: u32,
    encoder: *mut *mut c_void,
) -> u32;
pub type PnvEncGetEncodeGUIDCount =
    unsafe extern "system" fn(encoder: *mut c_void, count: *mut u32) -> u32;
pub type PnvEncGetEncodeProfileGUIDCount =
    unsafe extern "system" fn(encoder: *mut c_void, encode_guid: Guid, count: *mut u32) -> u32;
pub type PnvEncGetEncodeProfileGUIDs = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    profile_guids: *mut Guid,
    guid_array_size: u32,
    guid_count: *mut u32,
) -> u32;
pub type PnvEncGetEncodeGUIDs = unsafe extern "system" fn(
    encoder: *mut c_void,
    guid_array: *mut Guid,
    guid_array_size: u32,
    guid_count: *mut u32,
) -> u32;
pub type PnvEncGetInputFormatCount =
    unsafe extern "system" fn(encoder: *mut c_void, input_fmt_count: *mut u32) -> u32;
pub type PnvEncGetInputFormats = unsafe extern "system" fn(
    encoder: *mut c_void,
    buffer_fmt: u32,
    fmt_array: *mut u32,
    fmt_array_size: u32,
    fmt_count: *mut u32,
) -> u32;
pub type PnvEncGetEncodeCaps = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    caps_to_query: u32,
    out_caps_value: *mut i32,
) -> u32;
pub type PnvEncGetEncodePresetCount = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    preset_count: *mut u32,
) -> u32;
pub type PnvEncGetEncodePresetGUIDs = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    preset_array: *mut Guid,
    guid_array_size: u32,
    guid_count: *mut u32,
) -> u32;
pub type PnvEncGetEncodePresetConfig = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    preset_guid: Guid,
    preset_config: *mut NV_ENC_PRESET_CONFIG,
) -> u32;
pub type PnvEncInitializeEncoder = unsafe extern "system" fn(
    encoder: *mut c_void,
    create_encode_params: *mut NV_ENC_INITIALIZE_PARAMS,
) -> u32;
pub type PnvEncCreateInputBuffer = unsafe extern "system" fn(
    encoder: *mut c_void,
    create_input_buffer_params: *mut NV_ENC_CREATE_INPUT_BUFFER,
) -> u32;
pub type PnvEncDestroyInputBuffer =
    unsafe extern "system" fn(encoder: *mut c_void, input_buffer: *mut c_void) -> u32;
pub type PnvEncCreateBitstreamBuffer = unsafe extern "system" fn(
    encoder: *mut c_void,
    create_bitstream_buffer_params: *mut NV_ENC_CREATE_BITSTREAM_BUFFER,
) -> u32;
pub type PnvEncDestroyBitstreamBuffer =
    unsafe extern "system" fn(encoder: *mut c_void, bitstream_buffer: *mut c_void) -> u32;
pub type PnvEncEncodePicture = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_pic_params: *mut NV_ENC_PIC_PARAMS,
) -> u32;
pub type PnvEncLockBitstream = unsafe extern "system" fn(
    encoder: *mut c_void,
    lock_bitstream_buffer_params: *mut NV_ENC_LOCK_BITSTREAM,
) -> u32;
pub type PnvEncUnlockBitstream =
    unsafe extern "system" fn(encoder: *mut c_void, bitstream_buffer: *mut c_void) -> u32;
pub type PnvEncLockInputBuffer = unsafe extern "system" fn(
    encoder: *mut c_void,
    lock_input_buffer_params: *mut NV_ENC_LOCK_INPUT_BUFFER,
) -> u32;
pub type PnvEncUnlockInputBuffer =
    unsafe extern "system" fn(encoder: *mut c_void, input_buffer: *mut c_void) -> u32;
pub type PnvEncGetEncodeStats =
    unsafe extern "system" fn(encoder: *mut c_void, encode_stats: *mut c_void) -> u32;
pub type PnvEncGetSequenceParams =
    unsafe extern "system" fn(encoder: *mut c_void, sequence_param_payload: *mut c_void) -> u32;
pub type PnvEncRegisterAsyncEvent =
    unsafe extern "system" fn(encoder: *mut c_void, event_params: *mut c_void) -> u32;
pub type PnvEncUnregisterAsyncEvent =
    unsafe extern "system" fn(encoder: *mut c_void, event_params: *mut c_void) -> u32;
pub type PnvEncMapInputResource = unsafe extern "system" fn(
    encoder: *mut c_void,
    map_input_resource_params: *mut NV_ENC_MAP_INPUT_RESOURCE,
) -> u32;
pub type PnvEncUnmapInputResource =
    unsafe extern "system" fn(encoder: *mut c_void, mapped_input_buffer: *mut c_void) -> u32;
pub type PnvEncDestroyEncoder = unsafe extern "system" fn(encoder: *mut c_void) -> u32;
pub type PnvEncInvalidateRefFrames = unsafe extern "system" fn(
    encoder: *mut c_void,
    ref_frame_event: u64,
    invalid_frames_count: u32,
) -> u32;
pub type PnvEncOpenEncodeSessionEx = unsafe extern "system" fn(
    open_session_ex_params: *mut NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS,
    encoder: *mut *mut c_void,
) -> u32;
pub type PnvEncRegisterResource = unsafe extern "system" fn(
    encoder: *mut c_void,
    register_res_params: *mut NV_ENC_REGISTER_RESOURCE,
) -> u32;
pub type PnvEncUnregisterResource =
    unsafe extern "system" fn(encoder: *mut c_void, registered_res: *mut c_void) -> u32;
pub type PnvEncReconfigureEncoder =
    unsafe extern "system" fn(encoder: *mut c_void, reinit_encode_params: *mut c_void) -> u32;
pub type PnvEncCreateMVBuffer =
    unsafe extern "system" fn(encoder: *mut c_void, create_mv_buffer_params: *mut c_void) -> u32;
pub type PnvEncDestroyMVBuffer =
    unsafe extern "system" fn(encoder: *mut c_void, mv_buffer: *mut c_void) -> u32;
pub type PnvEncRunMotionEstimationOnly =
    unsafe extern "system" fn(encoder: *mut c_void, me_only_params: *mut c_void) -> u32;
pub type PnvEncGetLastError =
    unsafe extern "system" fn(encoder: *mut c_void) -> *const std::os::raw::c_char;
pub type PnvEncSetIOCudaStreams = unsafe extern "system" fn(
    encoder: *mut c_void,
    input_stream: *mut c_void,
    output_stream: *mut c_void,
) -> u32;
pub type PnvEncGetEncodePresetConfigEx = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    preset_guid: Guid,
    tuning_info: u32,
    preset_config: *mut NV_ENC_PRESET_CONFIG,
) -> u32;
pub type PnvEncGetSequenceParamEx = unsafe extern "system" fn(
    encoder: *mut c_void,
    encode_guid: Guid,
    preset_guid: Guid,
    sequence_param_payload: *mut c_void,
) -> u32;
pub type PnvEncRestoreEncoderState = unsafe extern "system" fn(
    encoder: *mut c_void,
    restore_encoder_state_params: *mut c_void,
) -> u32;
pub type PnvEncLookaheadPicture =
    unsafe extern "system" fn(encoder: *mut c_void, lookahead_pic_params: *mut c_void) -> u32;

/// `NV_ENCODE_API_FUNCTION_LIST` — 46 typed pointers + `reserved2[275]`, in
/// the exact order of `_NV_ENCODE_API_FUNCTION_LIST` in nvEncodeAPI.h.
#[repr(C)]
pub struct NV_ENCODE_API_FUNCTION_LIST {
    pub version: u32,
    pub reserved: u32,
    pub nv_enc_open_encode_session: Option<PnvEncOpenEncodeSession>,
    pub nv_enc_get_encode_guid_count: Option<PnvEncGetEncodeGUIDCount>,
    pub nv_enc_get_encode_profile_guid_count: Option<PnvEncGetEncodeProfileGUIDCount>,
    pub nv_enc_get_encode_profile_guids: Option<PnvEncGetEncodeProfileGUIDs>,
    pub nv_enc_get_encode_guids: Option<PnvEncGetEncodeGUIDs>,
    pub nv_enc_get_input_format_count: Option<PnvEncGetInputFormatCount>,
    pub nv_enc_get_input_formats: Option<PnvEncGetInputFormats>,
    pub nv_enc_get_encode_caps: Option<PnvEncGetEncodeCaps>,
    pub nv_enc_get_encode_preset_count: Option<PnvEncGetEncodePresetCount>,
    pub nv_enc_get_encode_preset_guids: Option<PnvEncGetEncodePresetGUIDs>,
    pub nv_enc_get_encode_preset_config: Option<PnvEncGetEncodePresetConfig>,
    pub nv_enc_initialize_encoder: Option<PnvEncInitializeEncoder>,
    pub nv_enc_create_input_buffer: Option<PnvEncCreateInputBuffer>,
    pub nv_enc_destroy_input_buffer: Option<PnvEncDestroyInputBuffer>,
    pub nv_enc_create_bitstream_buffer: Option<PnvEncCreateBitstreamBuffer>,
    pub nv_enc_destroy_bitstream_buffer: Option<PnvEncDestroyBitstreamBuffer>,
    pub nv_enc_encode_picture: Option<PnvEncEncodePicture>,
    pub nv_enc_lock_bitstream: Option<PnvEncLockBitstream>,
    pub nv_enc_unlock_bitstream: Option<PnvEncUnlockBitstream>,
    pub nv_enc_lock_input_buffer: Option<PnvEncLockInputBuffer>,
    pub nv_enc_unlock_input_buffer: Option<PnvEncUnlockInputBuffer>,
    pub nv_enc_get_encode_stats: Option<PnvEncGetEncodeStats>,
    pub nv_enc_get_sequence_params: Option<PnvEncGetSequenceParams>,
    pub nv_enc_register_async_event: Option<PnvEncRegisterAsyncEvent>,
    pub nv_enc_unregister_async_event: Option<PnvEncUnregisterAsyncEvent>,
    pub nv_enc_map_input_resource: Option<PnvEncMapInputResource>,
    pub nv_enc_unmap_input_resource: Option<PnvEncUnmapInputResource>,
    pub nv_enc_destroy_encoder: Option<PnvEncDestroyEncoder>,
    pub nv_enc_invalidate_ref_frames: Option<PnvEncInvalidateRefFrames>,
    pub nv_enc_open_encode_session_ex: Option<PnvEncOpenEncodeSessionEx>,
    pub nv_enc_register_resource: Option<PnvEncRegisterResource>,
    pub nv_enc_unregister_resource: Option<PnvEncUnregisterResource>,
    pub nv_enc_reconfigure_encoder: Option<PnvEncReconfigureEncoder>,
    pub reserved1: *mut c_void,
    pub nv_enc_create_mv_buffer: Option<PnvEncCreateMVBuffer>,
    pub nv_enc_destroy_mv_buffer: Option<PnvEncDestroyMVBuffer>,
    pub nv_enc_run_motion_estimation_only: Option<PnvEncRunMotionEstimationOnly>,
    pub nv_enc_get_last_error_string: Option<PnvEncGetLastError>,
    pub nv_enc_set_iocuda_streams: Option<PnvEncSetIOCudaStreams>,
    pub nv_enc_get_encode_preset_config_ex: Option<PnvEncGetEncodePresetConfigEx>,
    pub nv_enc_get_sequence_param_ex: Option<PnvEncGetSequenceParamEx>,
    pub nv_enc_restore_encoder_state: Option<PnvEncRestoreEncoderState>,
    pub nv_enc_lookahead_picture: Option<PnvEncLookaheadPicture>,
    pub reserved2: [*mut c_void; 275],
}

impl Default for NV_ENCODE_API_FUNCTION_LIST {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

// ---------------------------------------------------------------------------
// cuvid structures (dynlink_nvcuvid.h / dynlink_cuviddec.h)
// ---------------------------------------------------------------------------

/// `CUVIDEOFORMAT` (parser sequence callback payload; 64 B on both x64 ABIs).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDEOFORMAT {
    pub codec: i32,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
    pub progressive_sequence: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub min_num_decode_surfaces: u8,
    pub coded_width: u32,
    pub coded_height: u32,
    pub display_area: [i32; 4],
    pub chroma_format: i32,
    pub bitrate: u32,
    pub display_aspect_ratio_x: i32,
    pub display_aspect_ratio_y: i32,
    /// `video_format:3 | video_full_range_flag:1 | reserved_zero_bits:4` in
    /// the first byte, then color_primaries / transfer_characteristics /
    /// matrix_coefficients.
    pub video_signal_description: [u8; 4],
    pub seqhdr_data_length: u32,
}

impl Default for CUVIDEOFORMAT {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDSOURCEDATAPACKET` (layout differs by `tcu_ulong` width).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDSOURCEDATAPACKET {
    pub flags: TcuUlong,
    pub payload_size: TcuUlong,
    pub payload: *const u8,
    pub timestamp: CUvideotimestamp,
}

impl Default for CUVIDSOURCEDATAPACKET {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDPARSERDISPINFO` (display callback payload).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDPARSERDISPINFO {
    pub picture_index: i32,
    pub progressive_frame: i32,
    pub top_field_first: i32,
    pub repeat_first_field: i32,
    pub timestamp: CUvideotimestamp,
}

impl Default for CUVIDPARSERDISPINFO {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// Parser callback signatures (dynlink_nvcuvid.h PFNVID*CALLBACK).
pub type PFNVIDSEQUENCECALLBACK =
    Option<unsafe extern "system" fn(user_data: *mut c_void, format: *mut CUVIDEOFORMAT) -> i32>;
pub type PFNVIDDECODECALLBACK = Option<
    unsafe extern "system" fn(user_data: *mut c_void, pic_params: *mut CUVIDPICPARAMS) -> i32,
>;
pub type PFNVIDDISPLAYCALLBACK = Option<
    unsafe extern "system" fn(user_data: *mut c_void, disp_info: *mut CUVIDPARSERDISPINFO) -> i32,
>;
pub type PFNVIDOPPOINTCALLBACK =
    Option<unsafe extern "system" fn(user_data: *mut c_void, op_info: *mut c_void) -> i32>;
pub type PFNVIDSEIMSGCALLBACK =
    Option<unsafe extern "system" fn(user_data: *mut c_void, sei_info: *mut c_void) -> i32>;

/// `CUVIDPARSERPARAMS` (136 B on x64; the three flag bits pack one u32).
#[repr(C)]
pub struct CUVIDPARSERPARAMS {
    pub codec_type: i32,
    pub ul_max_num_decode_surfaces: u32,
    pub ul_clock_rate: u32,
    pub ul_error_threshold: u32,
    pub ul_max_display_delay: u32,
    /// `bAnnexb:1 | bMemoryOptimize:1 | uReserved:30`
    pub bitfields: u32,
    pub u_reserved1: [u32; 4],
    pub p_user_data: *mut c_void,
    pub pfn_sequence_callback: PFNVIDSEQUENCECALLBACK,
    pub pfn_decode_picture: PFNVIDDECODECALLBACK,
    pub pfn_display_picture: PFNVIDDISPLAYCALLBACK,
    pub pfn_get_operating_point: PFNVIDOPPOINTCALLBACK,
    pub pfn_get_sei_msg: PFNVIDSEIMSGCALLBACK,
    pub pv_reserved2: [*mut c_void; 5],
    pub p_ext_video_info: *mut c_void,
}

impl Default for CUVIDPARSERPARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDH264DPBENTRY`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CUVIDH264DPBENTRY {
    pub pic_idx: i32,
    pub frame_idx: i32,
    pub is_long_term: i32,
    pub not_existing: i32,
    pub used_for_reference: i32,
    pub field_order_cnt: [i32; 2],
}

/// `CUVIDH264MVCEXT` (140 B; the larger member of the SVC/MVC union).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDH264MVCEXT {
    pub num_views_minus1: i32,
    pub view_id: i32,
    pub inter_view_flag: u8,
    pub num_inter_view_refs_l0: u8,
    pub num_inter_view_refs_l1: u8,
    pub mvc_reserved8_bits: u8,
    pub inter_view_refs_l0: [i32; 16],
    pub inter_view_refs_l1: [i32; 16],
}

impl Default for CUVIDH264MVCEXT {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDH264SVCEXT` (48 B). `p_next_layer` is a self-referential pointer
/// into `CUVIDPICPARAMS`; we never construct one, the parser owns the chain.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDH264SVCEXT {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub dq_id: u8,
    pub dq_id_max: u8,
    pub disable_inter_layer_deblocking_filter_idc: u8,
    pub ref_layer_chroma_phase_y_plus1: u8,
    pub inter_layer_slice_alpha_c0_offset_div2: i8,
    pub inter_layer_slice_beta_offset_div2: i8,
    pub dpb_entry_valid_flag: u16,
    pub inter_layer_deblocking_filter_control_present_flag: u8,
    pub extended_spatial_scalability_idc: u8,
    pub adaptive_tcoeff_level_prediction_flag: u8,
    pub slice_header_restriction_flag: u8,
    pub chroma_phase_x_plus1_flag: u8,
    pub chroma_phase_y_plus1: u8,
    pub tcoeff_level_prediction_flag: u8,
    pub constrained_intra_resampling_flag: u8,
    pub ref_layer_chroma_phase_x_plus1_flag: u8,
    pub store_ref_base_pic_flag: u8,
    pub reserved8_bits_a: u8,
    pub reserved8_bits_b: u8,
    pub scaled_ref_layer_left_offset: i16,
    pub scaled_ref_layer_top_offset: i16,
    pub scaled_ref_layer_right_offset: i16,
    pub scaled_ref_layer_bottom_offset: i16,
    pub reserved16_bits: u16,
    pub p_next_layer: *mut CUVIDPICPARAMS,
    pub b_ref_base_layer: i32,
}

impl Default for CUVIDH264SVCEXT {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `fmo` union inside `CUVIDH264PICPARAMS`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union CUVIDH264FMO {
    pub slice_group_map_addr: u64,
    pub p_mb2_slice_group_map: *const u8,
}

impl Default for CUVIDH264FMO {
    fn default() -> Self {
        CUVIDH264FMO {
            slice_group_map_addr: 0,
        }
    }
}

/// MVC/SVC extension union inside `CUVIDH264PICPARAMS`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union CUVIDH264EXT {
    pub mvcext: CUVIDH264MVCEXT,
    pub svcext: CUVIDH264SVCEXT,
}

impl Default for CUVIDH264EXT {
    fn default() -> Self {
        CUVIDH264EXT {
            mvcext: CUVIDH264MVCEXT::default(),
        }
    }
}

/// `CUVIDH264PICPARAMS` (984 B on x64). The parser fills this; we only pass
/// the enclosing `CUVIDPICPARAMS` through to `cuvidDecodePicture`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CUVIDH264PICPARAMS {
    // SPS
    pub log2_max_frame_num_minus4: i32,
    pub pic_order_cnt_type: i32,
    pub log2_max_pic_order_cnt_lsb_minus4: i32,
    pub delta_pic_order_always_zero_flag: i32,
    pub frame_mbs_only_flag: i32,
    pub direct_8x8_inference_flag: i32,
    pub num_ref_frames: i32,
    pub residual_colour_transform_flag: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub qpprime_y_zero_transform_bypass_flag: u8,
    // PPS
    pub entropy_coding_mode_flag: i32,
    pub pic_order_present_flag: i32,
    pub num_ref_idx_l0_active_minus1: i32,
    pub num_ref_idx_l1_active_minus1: i32,
    pub weighted_pred_flag: i32,
    pub weighted_bipred_idc: i32,
    pub pic_init_qp_minus26: i32,
    pub deblocking_filter_control_present_flag: i32,
    pub redundant_pic_cnt_present_flag: i32,
    pub transform_8x8_mode_flag: i32,
    pub mbaff_frame_flag: i32,
    pub constrained_intra_pred_flag: i32,
    pub chroma_qp_index_offset: i32,
    pub second_chroma_qp_index_offset: i32,
    pub ref_pic_flag: i32,
    pub frame_num: i32,
    pub curr_field_order_cnt: [i32; 2],
    // DPB
    pub dpb: [CUVIDH264DPBENTRY; 16],
    // Quantization matrices (raster-order)
    pub weight_scale_4x4: [[u8; 16]; 6],
    pub weight_scale_8x8: [[u8; 64]; 2],
    // FMO/ASO
    pub fmo_aso_enable: u8,
    pub num_slice_groups_minus1: u8,
    pub slice_group_map_type: u8,
    pub pic_init_qs_minus26: i8,
    pub slice_group_change_rate_minus1: u32,
    pub fmo: CUVIDH264FMO,
    /// `mb_adaptive_frame_field_flag:2 | Reserved1:30`
    pub bitfields: u32,
    pub reserved: [u32; 11],
    // SVC/MVC
    pub ext: CUVIDH264EXT,
}

impl Default for CUVIDH264PICPARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// Codec-specific union inside `CUVIDPICPARAMS`. Only the H264 view is
/// accessed; `reserved[1024]` pins the union at the header's documented
/// 4096-byte size (the probe-verified real union is exactly that large).
#[repr(C)]
#[derive(Clone, Copy)]
pub union CUVIDCODECSPECIFIC {
    pub h264: CUVIDH264PICPARAMS,
    pub reserved: [u32; 1024],
}

impl Default for CUVIDCODECSPECIFIC {
    fn default() -> Self {
        CUVIDCODECSPECIFIC {
            reserved: [0u32; 1024],
        }
    }
}

/// `CUVIDPICPARAMS` (4280 B on x64; `CodecSpecific` union at offset 184).
/// The parser allocates and fills this struct — we never construct one, so
/// only the field offsets up to the union matter for pass-through decoding.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CUVIDPICPARAMS {
    pub pic_width_in_mbs: i32,
    pub frame_height_in_mbs: i32,
    pub curr_pic_idx: i32,
    pub field_pic_flag: i32,
    pub bottom_field_flag: i32,
    pub second_field: i32,
    pub n_bitstream_data_len: u32,
    pub p_bitstream_data: *const u8,
    pub n_num_slices: u32,
    pub p_slice_data_offsets: *const u32,
    pub ref_pic_flag: i32,
    pub intra_pic_flag: i32,
    pub timestamp: CUvideotimestamp,
    pub reserved: [u32; 28],
    pub codec_specific: CUVIDCODECSPECIFIC,
}

impl Default for CUVIDPICPARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDPROCPARAMSEXT`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDPROCPARAMSEXT {
    pub decode_stats_size: u32,
    pub reserved1: [u32; 31],
    pub decode_stats_dptr: *mut CUdeviceptr,
    pub reserved_ptr: [*mut c_void; 15],
}

impl Default for CUVIDPROCPARAMSEXT {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDPROCPARAMS` (264 B on x64).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDPROCPARAMS {
    pub progressive_frame: i32,
    pub second_field: i32,
    pub top_field_first: i32,
    pub unpaired_field: i32,
    pub reserved_flags: u32,
    pub reserved_zero: u32,
    pub raw_input_dptr: u64,
    pub raw_input_pitch: u32,
    pub raw_input_format: u32,
    pub raw_output_dptr: u64,
    pub raw_output_pitch: u32,
    pub reserved1: u32,
    pub output_stream: CUstream,
    pub reserved: [u32; 46],
    pub histogram_dptr: *mut u64,
    pub p_cuvid_proc_ext: *mut CUVIDPROCPARAMSEXT,
}

impl Default for CUVIDPROCPARAMS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDDECODECREATEINFO` (112 B on Windows x64 where `tcu_ulong` = u32;
/// 176 B on LP64 Linux where it is u64).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDDECODECREATEINFO {
    pub ul_width: TcuUlong,
    pub ul_height: TcuUlong,
    pub ul_num_decode_surfaces: TcuUlong,
    pub codec_type: i32,
    pub chroma_format: i32,
    pub ul_creation_flags: TcuUlong,
    pub bit_depth_minus8: TcuUlong,
    pub ul_intra_decode_only: TcuUlong,
    pub ul_max_width: TcuUlong,
    pub ul_max_height: TcuUlong,
    pub reserved1: TcuUlong,
    pub display_area: [i16; 4],
    pub output_format: i32,
    pub deinterlace_mode: i32,
    pub ul_target_width: TcuUlong,
    pub ul_target_height: TcuUlong,
    pub ul_num_output_surfaces: TcuUlong,
    pub vid_lock: *mut c_void,
    pub target_rect: [i16; 4],
    pub enable_histogram: TcuUlong,
    pub enable_decode_features: TcuUlong,
    pub reserved2: [TcuUlong; 3],
}

impl Default for CUVIDDECODECREATEINFO {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `CUVIDDECODECAPS`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CUVIDDECODECAPS {
    pub e_codec_type: i32,
    pub e_chroma_format: i32,
    pub n_bit_depth_minus8: u32,
    pub reserved1: [u32; 3],
    pub b_is_supported: u8,
    pub n_num_nvdecs: u8,
    pub n_output_format_mask: u16,
    pub n_max_width: u32,
    pub n_max_height: u32,
    pub n_max_mb_count: u32,
    pub n_min_width: u16,
    pub n_min_height: u16,
    pub b_is_histogram_supported: u8,
    pub n_counter_bit_depth: u8,
    pub n_max_histogram_bins: u16,
    pub b_is_decode_stats_supported: u8,
    pub reserved4: [u8; 3],
    pub reserved3: [u32; 9],
}

impl Default for CUVIDDECODECAPS {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

// ---------------------------------------------------------------------------
// cuvid function types (dynlink_nvcuvid.h / dynlink_cuviddec.h tcuvid*)
// ---------------------------------------------------------------------------

pub type TcuvidGetDecoderCaps = unsafe extern "system" fn(pdc: *mut CUVIDDECODECAPS) -> CUresult;
pub type TcuvidCreateDecoder = unsafe extern "system" fn(
    ph_decoder: *mut CUvideodecoder,
    pdci: *mut CUVIDDECODECREATEINFO,
) -> CUresult;
pub type TcuvidDestroyDecoder = unsafe extern "system" fn(h_decoder: CUvideodecoder) -> CUresult;
pub type TcuvidDecodePicture = unsafe extern "system" fn(
    h_decoder: CUvideodecoder,
    p_pic_params: *mut CUVIDPICPARAMS,
) -> CUresult;
/// The 64-bit dev-pointer variants (`cuvidMapVideoFrame64` /
/// `cuvidUnmapVideoFrame64`): ffnvcodec's loader binds these names on 64-bit
/// targets, and `cuvidMapVideoFrame` itself only takes a 32-bit `unsigned
/// int*` device pointer. There is no "MapVideoFrame2" in SDK 13.1.
pub type TcuvidMapVideoFrame64 = unsafe extern "system" fn(
    h_decoder: CUvideodecoder,
    n_pic_idx: i32,
    p_dev_ptr: *mut u64,
    p_pitch: *mut u32,
    p_vpp: *mut CUVIDPROCPARAMS,
) -> CUresult;
pub type TcuvidUnmapVideoFrame64 =
    unsafe extern "system" fn(h_decoder: CUvideodecoder, dev_ptr: u64) -> CUresult;
pub type TcuvidCreateVideoParser = unsafe extern "system" fn(
    p_obj: *mut CUvideoparser,
    p_params: *mut CUVIDPARSERPARAMS,
) -> CUresult;
pub type TcuvidParseVideoData =
    unsafe extern "system" fn(obj: CUvideoparser, p_packet: *mut CUVIDSOURCEDATAPACKET) -> CUresult;
pub type TcuvidDestroyVideoParser = unsafe extern "system" fn(obj: CUvideoparser) -> CUresult;

// ---------------------------------------------------------------------------
// CUDA driver API subset (own context; independent of cudarc)
// ---------------------------------------------------------------------------

pub const CU_SUCCESS: CUresult = 0;

pub type TcuInit = unsafe extern "system" fn(flags: u32) -> CUresult;
pub type TcuDeviceGet = unsafe extern "system" fn(device: *mut CUdevice, ordinal: i32) -> CUresult;
pub type TcuDeviceGetCount = unsafe extern "system" fn(count: *mut i32) -> CUresult;
pub type TcuDeviceGetAttribute =
    unsafe extern "system" fn(pi: *mut i32, attrib: i32, dev: CUdevice) -> CUresult;
pub type TcuDeviceGetName =
    unsafe extern "system" fn(name: *mut u8, len: i32, dev: CUdevice) -> CUresult;
pub type TcuDevicePrimaryCtxRetain =
    unsafe extern "system" fn(pctx: *mut CUcontext, dev: CUdevice) -> CUresult;
pub type TcuDevicePrimaryCtxRelease = unsafe extern "system" fn(dev: CUdevice) -> CUresult;
pub type TcuCtxSetCurrent = unsafe extern "system" fn(ctx: CUcontext) -> CUresult;
pub type TcuCtxGetCurrent = unsafe extern "system" fn(pctx: *mut CUcontext) -> CUresult;
pub type TcuMemcpyDtoH = unsafe extern "system" fn(
    dst_host: *mut c_void,
    src_device: CUdeviceptr,
    byte_count: usize,
) -> CUresult;

/// Entry points exported by the NVENC library (dynlink_loader.h binds these
/// exact undecorated names; on Windows x64 there is no stdcall decoration).
pub type TNvEncodeAPICreateInstance =
    unsafe extern "system" fn(function_list: *mut NV_ENCODE_API_FUNCTION_LIST) -> u32;
pub type TNvEncodeAPIGetMaxSupportedVersion = unsafe extern "system" fn(version: *mut u32) -> u32;

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    // Everything below is pinned against `nvlayout_probe.c` compiled with
    // MinGW x64 against the real ffnvcodec SDK 13.1 headers
    // (nvEncodeAPI.h / dynlink_nvcuvid.h / dynlink_cuviddec.h). A driver
    // ABI mismatch here means the encoder writes past our structs.

    #[test]
    fn version_macros_match_the_header_arithmetic() {
        assert_eq!(NVENCAPI_VERSION, 0x0100_000D);
        assert_eq!(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER, 0x7101_000D);
        assert_eq!(NV_ENCODE_API_FUNCTION_LIST_VER, 0x7102_000D);
        assert_eq!(NV_ENC_CREATE_INPUT_BUFFER_VER, 0x7102_000D);
        assert_eq!(NV_ENC_CREATE_BITSTREAM_BUFFER_VER, 0x7101_000D);
        assert_eq!(NV_ENC_RC_PARAMS_VER, 0x7101_000D);
        assert_eq!(NV_ENC_LOCK_INPUT_BUFFER_VER, 0x7101_000D);
        assert_eq!(NV_ENC_CONFIG_VER, 0xF109_000D);
        assert_eq!(NV_ENC_INITIALIZE_PARAMS_VER, 0xF107_000D);
        assert_eq!(NV_ENC_PRESET_CONFIG_VER, 0xF105_000D);
        assert_eq!(NV_ENC_PIC_PARAMS_VER, 0xF107_000D);
        assert_eq!(NV_ENC_LOCK_BITSTREAM_VER, 0xF102_000D);
        assert_eq!(NV_ENC_REGISTER_RESOURCE_VER, 0x7105_000D);
        assert_eq!(NV_ENC_MAP_INPUT_RESOURCE_VER, 0x7104_000D);
    }

    #[test]
    fn guid_values_are_verbatim_from_the_header() {
        assert_eq!(NV_ENC_CODEC_H264_GUID.data1, 0x6bc8_2762);
        assert_eq!(
            NV_ENC_H264_PROFILE_MAIN_GUID.data4,
            [0x94, 0xd5, 0xc4, 0x72, 0x6d, 0x7b, 0x6e, 0x6d]
        );
        assert_eq!(NV_ENC_PRESET_P3_GUID.data1, 0x3685_0110);
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(preset_guid("p7"), Some(NV_ENC_PRESET_P7_GUID));
        assert_eq!(preset_guid("p8"), None);
    }

    #[test]
    fn nvenc_struct_sizes_match_the_c_probe() {
        assert_eq!(size_of::<NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS>(), 1552);
        assert_eq!(size_of::<NV_ENC_QP>(), 12);
        assert_eq!(size_of::<NV_ENC_RC_PARAMS>(), 128);
        assert_eq!(size_of::<NV_ENC_CONFIG_H264_VUI_PARAMETERS>(), 112);
        assert_eq!(size_of::<NV_ENC_CONFIG_H264>(), 1792);
        assert_eq!(size_of::<NV_ENC_CODEC_CONFIG>(), 1792);
        assert_eq!(size_of::<NV_ENC_CONFIG>(), 3584);
        assert_eq!(size_of::<NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE>(), 16);
        assert_eq!(size_of::<NV_ENC_INITIALIZE_PARAMS>(), 1800);
        assert_eq!(size_of::<NV_ENC_PRESET_CONFIG>(), 5128);
        assert_eq!(size_of::<NV_ENC_PIC_PARAMS>(), 3360);
        assert_eq!(size_of::<NV_ENC_CODEC_PIC_PARAMS>(), 1544);
        assert_eq!(size_of::<NV_ENC_LOCK_BITSTREAM>(), 1544);
        assert_eq!(size_of::<NV_ENC_LOCK_INPUT_BUFFER>(), 1544);
        assert_eq!(size_of::<NV_ENC_CREATE_INPUT_BUFFER>(), 776);
        assert_eq!(size_of::<NV_ENC_CREATE_BITSTREAM_BUFFER>(), 776);
        assert_eq!(size_of::<NV_ENC_REGISTER_RESOURCE>(), 1536);
        assert_eq!(size_of::<NV_ENC_MAP_INPUT_RESOURCE>(), 1544);
        assert_eq!(size_of::<NV_ENCODE_API_FUNCTION_LIST>(), 2552);
    }

    #[test]
    fn nvenc_struct_offsets_match_the_c_probe() {
        assert_eq!(
            offset_of!(NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS, api_version),
            24
        );
        assert_eq!(offset_of!(NV_ENC_RC_PARAMS, min_qp), 40);
        assert_eq!(offset_of!(NV_ENC_RC_PARAMS, temporal_layer_idx_mask), 76);
        assert_eq!(offset_of!(NV_ENC_RC_PARAMS, multi_pass), 100);
        assert_eq!(offset_of!(NV_ENC_CONFIG_H264, h264_vui_parameters), 72);
        assert_eq!(offset_of!(NV_ENC_CONFIG_H264, chroma_format_idc), 192);
        assert_eq!(offset_of!(NV_ENC_CONFIG_H264, reserved1), 224);
        assert_eq!(offset_of!(NV_ENC_CONFIG, gop_length), 20);
        assert_eq!(offset_of!(NV_ENC_CONFIG, rc_params), 40);
        assert_eq!(offset_of!(NV_ENC_CONFIG, encode_codec_config), 168);
        assert_eq!(offset_of!(NV_ENC_INITIALIZE_PARAMS, encode_width), 36);
        assert_eq!(
            offset_of!(NV_ENC_INITIALIZE_PARAMS, max_me_hint_counts_per_block),
            104
        );
        assert_eq!(offset_of!(NV_ENC_INITIALIZE_PARAMS, tuning_info), 136);
        assert_eq!(offset_of!(NV_ENC_INITIALIZE_PARAMS, reserved1), 152);
        assert_eq!(offset_of!(NV_ENC_PRESET_CONFIG, preset_cfg), 8);
        assert_eq!(offset_of!(NV_ENC_PIC_PARAMS, input_time_stamp), 24);
        assert_eq!(offset_of!(NV_ENC_PIC_PARAMS, input_buffer), 40);
        assert_eq!(offset_of!(NV_ENC_PIC_PARAMS, codec_pic_params), 80);
        assert_eq!(
            offset_of!(NV_ENC_PIC_PARAMS, me_hint_counts_per_block),
            1624
        );
        assert_eq!(offset_of!(NV_ENC_PIC_PARAMS, qp_delta_map), 1712);
        assert_eq!(offset_of!(NV_ENC_LOCK_BITSTREAM, output_bitstream), 8);
        assert_eq!(offset_of!(NV_ENC_LOCK_BITSTREAM, frame_idx), 24);
        assert_eq!(
            offset_of!(NV_ENC_LOCK_BITSTREAM, bitstream_size_in_bytes),
            36
        );
        assert_eq!(offset_of!(NV_ENC_LOCK_BITSTREAM, bitstream_buffer_ptr), 56);
        assert_eq!(offset_of!(NV_ENC_LOCK_BITSTREAM, picture_type), 64);
        assert_eq!(offset_of!(NV_ENC_LOCK_INPUT_BUFFER, buffer_data_ptr), 16);
        assert_eq!(offset_of!(NV_ENC_LOCK_INPUT_BUFFER, pitch), 24);
        assert_eq!(offset_of!(NV_ENC_CREATE_INPUT_BUFFER, input_buffer), 24);
        assert_eq!(
            offset_of!(NV_ENC_CREATE_BITSTREAM_BUFFER, bitstream_buffer),
            16
        );
        assert_eq!(
            offset_of!(NV_ENC_REGISTER_RESOURCE, resource_to_register),
            24
        );
        assert_eq!(
            offset_of!(NV_ENC_REGISTER_RESOURCE, registered_resource),
            32
        );
        assert_eq!(offset_of!(NV_ENC_REGISTER_RESOURCE, buffer_format), 40);
        assert_eq!(
            offset_of!(NV_ENC_MAP_INPUT_RESOURCE, registered_resource),
            16
        );
        assert_eq!(offset_of!(NV_ENC_MAP_INPUT_RESOURCE, mapped_resource), 24);
        assert_eq!(offset_of!(NV_ENC_MAP_INPUT_RESOURCE, mapped_buffer_fmt), 32);
        assert_eq!(
            offset_of!(NV_ENCODE_API_FUNCTION_LIST, nv_enc_open_encode_session),
            8
        );
        assert_eq!(
            offset_of!(NV_ENCODE_API_FUNCTION_LIST, nv_enc_initialize_encoder),
            96
        );
        assert_eq!(
            offset_of!(NV_ENCODE_API_FUNCTION_LIST, nv_enc_open_encode_session_ex),
            240
        );
        assert_eq!(
            offset_of!(NV_ENCODE_API_FUNCTION_LIST, nv_enc_get_last_error_string),
            304
        );
        assert_eq!(offset_of!(NV_ENCODE_API_FUNCTION_LIST, reserved2), 352);
    }

    #[test]
    fn nvenc_status_names_cover_the_enum() {
        assert_eq!(nvenc_status::name(0), "NV_ENC_SUCCESS");
        assert_eq!(nvenc_status::name(9), "NV_ENC_ERR_INVALID_CALL");
        assert!(nvenc_status::name(999).starts_with("NV_ENC_ERR_<unknown>"));
    }

    #[test]
    fn cuvid_struct_sizes_match_the_c_probe() {
        // Windows x64 (tcu_ulong = 4 bytes):
        #[cfg(windows)]
        {
            assert_eq!(size_of::<CUVIDSOURCEDATAPACKET>(), 24);
            assert_eq!(size_of::<CUVIDDECODECREATEINFO>(), 112);
            assert_eq!(offset_of!(CUVIDSOURCEDATAPACKET, payload_size), 4);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, codec_type), 12);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, vid_lock), 72);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, reserved2), 96);
        }
        // LP64 Linux (tcu_ulong = 8 bytes): only the tcu_ulong-bearing
        // structs change width (8 ulWidth/ulHeight/... fields widen).
        #[cfg(all(not(windows), target_os = "linux"))]
        {
            assert_eq!(size_of::<CUVIDSOURCEDATAPACKET>(), 32);
            assert_eq!(size_of::<CUVIDDECODECREATEINFO>(), 176);
            assert_eq!(offset_of!(CUVIDSOURCEDATAPACKET, payload_size), 8);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, codec_type), 24);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, vid_lock), 120);
            assert_eq!(offset_of!(CUVIDDECODECREATEINFO, reserved2), 152);
        }
        // Platform-independent layouts:
        assert_eq!(size_of::<CUVIDEOFORMAT>(), 64);
        assert_eq!(offset_of!(CUVIDEOFORMAT, coded_width), 16);
        assert_eq!(offset_of!(CUVIDEOFORMAT, chroma_format), 40);
        assert_eq!(offset_of!(CUVIDEOFORMAT, seqhdr_data_length), 60);
        assert_eq!(size_of::<CUVIDPARSERDISPINFO>(), 24);
        assert_eq!(size_of::<CUVIDPARSERPARAMS>(), 136);
        assert_eq!(offset_of!(CUVIDPARSERPARAMS, p_user_data), 40);
        assert_eq!(offset_of!(CUVIDPARSERPARAMS, pfn_sequence_callback), 48);
        assert_eq!(offset_of!(CUVIDPARSERPARAMS, pfn_decode_picture), 56);
        assert_eq!(offset_of!(CUVIDPARSERPARAMS, pfn_display_picture), 64);
        assert_eq!(offset_of!(CUVIDPARSERPARAMS, p_ext_video_info), 128);
        assert_eq!(size_of::<CUVIDH264DPBENTRY>(), 28);
        assert_eq!(size_of::<CUVIDH264MVCEXT>(), 140);
        assert_eq!(size_of::<CUVIDH264SVCEXT>(), 48);
        assert_eq!(size_of::<CUVIDH264PICPARAMS>(), 984);
        assert_eq!(offset_of!(CUVIDH264PICPARAMS, dpb), 104);
        assert_eq!(offset_of!(CUVIDH264PICPARAMS, weight_scale_4x4), 552);
        assert_eq!(offset_of!(CUVIDH264PICPARAMS, fmo_aso_enable), 776);
        assert_eq!(offset_of!(CUVIDH264PICPARAMS, reserved), 796);
        assert_eq!(size_of::<CUVIDPICPARAMS>(), 4280);
        assert_eq!(offset_of!(CUVIDPICPARAMS, n_bitstream_data_len), 24);
        assert_eq!(offset_of!(CUVIDPICPARAMS, reserved), 72);
        assert_eq!(offset_of!(CUVIDPICPARAMS, codec_specific), 184);
        assert_eq!(size_of::<CUVIDPROCPARAMSEXT>(), 256);
        assert_eq!(size_of::<CUVIDPROCPARAMS>(), 264);
        assert_eq!(offset_of!(CUVIDPROCPARAMS, raw_input_dptr), 24);
        assert_eq!(offset_of!(CUVIDPROCPARAMS, output_stream), 56);
        assert_eq!(offset_of!(CUVIDPROCPARAMS, histogram_dptr), 248);
        assert_eq!(size_of::<CUVIDDECODECAPS>(), 88);
    }
}
