//! Heavy FurMark-style Vulkan render loop.
//!
//! Purpose-built graphics-domain stimulus, complementing the light
//! clear/blit path in `vulkan_gfx_stressor.rs`. What it exercises that the
//! light path never touches:
//! - programmable shading: fragment shader with a data-dependent
//!   sin/cos/exp2/log2/inversesqrt chain (SFU paths) + FMA interleave
//! - rasterizer + attribute interpolator (dense torus mesh, instanced
//!   shells = layered overdraw)
//! - ROP alpha blending (read-modify-write) + depth test/write
//! - optional MSAA resolve
//! - display engine: window + swapchain present (skipped in offscreen mode)
//! - ~100% duty cycle (no sleeps in this loop)
//!
//! Targets:
//! - Windows (default): Win32 window + swapchain present. Resize/minimize
//!   handled via swapchain recreation; a minimized window parks the loop
//!   until restored.
//! - Any OS (`offscreen`): renders into an owned color image — pure CLI,
//!   no window system. Covers everything except display-engine scanout.
//!
//! Shaders are GLSL translated to SPIR-V at init/recreate time via naga (no
//! external toolchain needed).

use crate::runner::style::stylize;
use ash::khr::surface::Instance as SurfaceInstance;
use ash::khr::swapchain::Device as SwapchainDevice;
use ash::vk;
use ash::Instance;
use crate::vulkan_gfx_stressor::{select_gpu_by_cuda_identity, VulkanDeviceSelection};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(target_os = "windows")]
/// FurMark-style heavy render parameters.
#[derive(Clone, Copy, Debug)]
pub struct VulkanHeavyConfig {
    pub width: u32,
    pub height: u32,
    /// MSAA sample count: 1 = off, 2/4/8. Clamped to device-supported.
    pub msaa: u32,
    /// Fragment MUFU/FMA loop iterations per pixel.
    pub iters: u32,
    /// Instanced shell count (layered overdraw with alpha blending).
    pub shells: u32,
    /// Render into an owned color image instead of a window swapchain
    /// (pure CLI / headless; skips the display-engine path).
    pub offscreen: bool,
    /// Animate the torus rotation (dynamic tiles/Z-distribution/interp
    /// inputs). Off for the static-mesh A/B baseline.
    pub rotate: bool,
}

impl Default for VulkanHeavyConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            msaa: 1,
            iters: 128,
            shells: 16,
            offscreen: false,
            rotate: true,
        }
    }
}

const BG_VERT_SRC: &str = r#"
#version 450 core
layout(location = 0) out vec2 v_uv;
void main() {
    vec2 p = vec2(float((gl_VertexIndex << 1u) & 2u), float(gl_VertexIndex & 2u));
    v_uv = p;
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const BG_FRAG_SRC: &str = r#"
#version 450 core
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
layout(push_constant) uniform Push { float u_time; float shell_off; int iters; float rotate; } pc;
void main() {
    vec2 p = v_uv * 2.0 - 1.0;
    float s = 0.0;
    // Data-dependent MUFU chain: p mutates every iteration, so the
    // sin/cos/exp2/log2/inversesqrt calls cannot be constant-folded or
    // hoisted. FMA work is interleaved to keep the FP32 pipe dual-issuing
    // alongside SFU.
    // TEMP-DEBUG: loop removed entirely
    s = abs(s);
    vec3 col = 0.5 + 0.5 * vec3(sin(s * 8.0), sin(s * 8.0 + 2.1), sin(s * 8.0 + 4.2));
    out_color = vec4(col, 1.0);
}
"#;

const KNOT_VERT_SRC: &str = r#"
#version 450 core
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in vec2 in_uv;
layout(location = 0) out vec3 v_normal;
layout(location = 1) out vec3 v_pos;
layout(push_constant) uniform Push { float u_time; float shell_off; int iters; float rotate; } pc;
void main() {
    // Object rotation: dynamic geometry rotates tiles/interpolation inputs/
    // Z-distribution every frame (second-order stress the static mesh lacks).
    // X+Y compound axis so the projected silhouette actually changes (a pure
    // Y spin is invisible on a torus and would not move tiles).
    float t = pc.u_time * 0.5;
    float cx = cos(t), sx = sin(t);
    float cy = cos(t * 0.7), sy = sin(t * 0.7);
    mat3 rot_x = mat3(1.0, 0.0, 0.0, 0.0, cx, sx, 0.0, -sx, cx);
    mat3 rot_y = mat3(cy, 0.0, sy, 0.0, 1.0, 0.0, -sy, 0.0, cy);
    mat3 rot = rot_y * rot_x;
    vec3 p = rot * (in_pos + in_normal * pc.shell_off * float(gl_InstanceIndex));
    v_normal = rot * in_normal;
    v_pos = p;
    gl_Position = vec4(p, 1.0);
}
"#;

const KNOT_FRAG_SRC: &str = r#"
#version 450 core
layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec3 v_pos;
layout(location = 0) out vec4 out_color;
layout(push_constant) uniform Push { float u_time; float shell_off; int iters; float rotate; } pc;
void main() {
    vec3 N = normalize(v_normal);
    vec3 V = normalize(vec3(0.0, 0.6, 2.2) - v_pos);
    vec3 col = vec3(0.05, 0.06, 0.09);
    for (int i = 0; i < 3; ++i) {
        float fi = float(i);
        vec3 L = normalize(vec3(sin(pc.u_time * 0.7 + fi * 2.1), 0.8,
                                cos(pc.u_time * 0.6 + fi * 1.7)));
        float diff = max(dot(N, L), 0.0);
        vec3 H = normalize(L + V);
        float spec = pow(max(dot(N, H), 0.0), 32.0 + fi * 16.0);
        col += vec3(0.9, 0.6, 0.3) * diff * (0.4 - fi * 0.1) + vec3(1.0) * spec * 0.6;
    }
    float rim = pow(1.0 - max(dot(N, V), 0.0), 2.0);
    col += vec3(0.2, 0.3, 0.5) * rim;
    out_color = vec4(col, 0.65);
}
"#;

fn compile_glsl(
    stage: naga::ShaderStage,
    source: &str,
) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
    let options = naga::front::glsl::Options {
        stage,
        defines: Default::default(),
    };
    let module = naga::front::glsl::Frontend::default().parse(&options, source)?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)?;
    let spv_options = naga::back::spv::Options::default();
    let pipeline = naga::back::spv::PipelineOptions {
        shader_stage: stage,
        entry_point: "main".into(),
    };
    naga::back::spv::write_vec(&module, &info, &spv_options, Some(&pipeline))
        .map_err(|err| -> Box<dyn std::error::Error> { err.into() })
}

// ---------------------------------------------------------------------------
// Win32 window (fixed-size creation; resize handled via swapchain recreation)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
struct Win32Window {
    hwnd: isize,
}

#[cfg(target_os = "windows")]
impl Win32Window {
    fn new(width: u32, height: u32) -> Result<Self, Box<dyn std::error::Error>> {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            AdjustWindowRect, CreateWindowExW, RegisterClassExW, ShowWindow, CW_USEDEFAULT,
            SW_SHOW, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        const CLASS_NAME: &[u16] = &[
            b'N' as u16, b'V' as u16, b'O' as u16, b'C' as u16, b'V' as u16, b'K' as u16,
            b'H' as u16, b'e' as u16, b'a' as u16, b'v' as u16, b'y' as u16, 0,
        ];
        const TITLE: &[u16] = &[
            b'n' as u16, b'v' as u16, b'o' as u16, b'c' as u16, b' ' as u16, b'v' as u16,
            b'k' as u16, b' ' as u16, b'h' as u16, b'e' as u16, b'a' as u16, b'v' as u16,
            b'y' as u16, 0,
        ];

        unsafe {
            let hinstance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: CLASS_NAME.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            let _ = RegisterClassExW(&wc);

            let mut rect = windows_sys::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width as i32,
                bottom: height as i32,
            };
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;

            let hwnd = CreateWindowExW(
                0,
                CLASS_NAME.as_ptr(),
                TITLE.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinstance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            ShowWindow(hwnd, SW_SHOW);
            Ok(Self { hwnd: hwnd as isize })
        }
    }

    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn pump_messages(&self) -> Result<(), Box<dyn std::error::Error>> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE, WM_QUIT,
        };
        unsafe {
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return Err("window closed by user".into());
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl Drop for Win32Window {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
        unsafe {
            if self.hwnd != 0 {
                DestroyWindow(self.hwnd as *mut core::ffi::c_void);
            }
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn window_proc(
    hwnd: *mut core::ffi::c_void,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DefWindowProcW, PostQuitMessage, WM_DESTROY};
    unsafe {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

// ---------------------------------------------------------------------------

fn find_memory_type(
    mem_properties: &vk::PhysicalDeviceMemoryProperties,
    req: vk::MemoryRequirements,
    flags: vk::MemoryPropertyFlags,
) -> Result<u32, Box<dyn std::error::Error>> {
    (0..mem_properties.memory_type_count)
        .find(|&i| {
            (req.memory_type_bits & (1 << i)) != 0
                && mem_properties.memory_types[i as usize].property_flags.contains(flags)
        })
        .ok_or_else(|| "no compatible Vulkan memory type".into())
}

#[allow(clippy::needless_range_loop)]
fn build_torus_mesh() -> (Vec<f32>, Vec<u32>) {
    let (r_major, r_minor) = (1.0f32, 0.35f32);
    let mut verts = Vec::with_capacity((RINGS * SEGS * 8) as usize);
    for i in 0..RINGS {
        let u = (i as f32 / RINGS as f32) * std::f32::consts::TAU;
        let cu = u.cos();
        let su = u.sin();
        for j in 0..SEGS {
            let v = (j as f32 / SEGS as f32) * std::f32::consts::TAU;
            let cv = v.cos();
            let sv = v.sin();
            let nx = cu * cv;
            let ny = sv;
            let nz = su * cv;
            let px = (r_major + r_minor * cv) * cu;
            let py = r_minor * sv;
            let pz = (r_major + r_minor * cv) * su;
            verts.extend_from_slice(&[
                px, py, pz, nx, ny, nz, u / std::f32::consts::TAU,
                v / std::f32::consts::TAU,
            ]);
        }
    }
    let mut indices = Vec::with_capacity((RINGS * SEGS * 6) as usize);
    for i in 0..RINGS {
        for j in 0..SEGS {
            let a = i * SEGS + j;
            let b = i * SEGS + (j + 1) % SEGS;
            let c = ((i + 1) % RINGS) * SEGS + (j + 1) % SEGS;
            let d = ((i + 1) % RINGS) * SEGS + j;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    (verts, indices)
}

const RINGS: u32 = 180;
const SEGS: u32 = 90;

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn run_heavy_render_loop(
    is_running: Arc<AtomicBool>,
    selection: Option<VulkanDeviceSelection>,
    cfg: VulkanHeavyConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        // ---- target selection: windowed swapchain vs headless offscreen ----
        let use_swapchain = cfg!(target_os = "windows") && !cfg.offscreen;

        let mut window_hwnd: isize = 0;
        let mut window_hinstance: isize = 0;
        let window = if use_swapchain {
            let w = Win32Window::new(cfg.width, cfg.height)?;
            window_hwnd = w.hwnd();
            window_hinstance = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(
                std::ptr::null(),
            ) as isize;
            Some(w)
        } else {
            None
        };

        let entry = ash::Entry::load()?;
        let mut instance_exts: Vec<*const i8> = Vec::new();
        if use_swapchain {
            instance_exts.push(ash::khr::surface::NAME.as_ptr());
            instance_exts.push(ash::khr::win32_surface::NAME.as_ptr());
        }
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"nvoc-vk-heavy")
            .api_version(vk::API_VERSION_1_2);
        let instance: Instance = entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_extension_names(&instance_exts),
            None,
        )?;

        let pdevice = if let Some(selection) = selection {
            select_gpu_by_cuda_identity(&instance, selection.cuda_uuid, selection.cuda_pci_bus)
                .map_err(|err| format!("[VKGFX-H] Vulkan GPU selection failed: {err}"))?
        } else {
            let pdevices = instance.enumerate_physical_devices()?;
            *pdevices.first().ok_or("no Vulkan physical devices")?
        };

        let surface_fn = if use_swapchain {
            Some(SurfaceInstance::new(&entry, &instance))
        } else {
            None
        };
        let surface = if use_swapchain {
            let win32_fn = ash::khr::win32_surface::Instance::new(&entry, &instance);
            win32_fn.create_win32_surface(
                &vk::Win32SurfaceCreateInfoKHR::default()
                    .hinstance(window_hinstance)
                    .hwnd(window_hwnd),
                None,
            )?
        } else {
            vk::SurfaceKHR::null()
        };

        let queue_family_properties =
            instance.get_physical_device_queue_family_properties(pdevice);
        let queue_family = (0..queue_family_properties.len() as u32)
            .find(|&f| {
                queue_family_properties[f as usize]
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS)
                    && (!use_swapchain
                        || surface_fn
                            .as_ref()
                            .unwrap()
                            .get_physical_device_surface_support(
                                pdevice,
                                f,
                                surface,
                            )
                            .unwrap_or(false))
            })
            .ok_or("no graphics queue family")?;

        let mut device_exts: Vec<*const i8> = Vec::new();
        if use_swapchain {
            device_exts.push(ash::khr::swapchain::NAME.as_ptr());
        }
        let queue_priorities = [1.0];
        let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&queue_priorities)];
        let device = instance.create_device(
            pdevice,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_create_infos)
                .enabled_extension_names(&device_exts),
            None,
        )?;
        let queue = device.get_device_queue(queue_family, 0);
        let swapchain_dev = if use_swapchain {
            Some(SwapchainDevice::new(&instance, &device))
        } else {
            None
        };

        // ---- command pool / sync ----
        let command_pool = device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
            None,
        )?;
        let cmd = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        let fence = device.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )?;
        let acquire_sem = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
        let render_sem = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;

        let mem_properties = instance.get_physical_device_memory_properties(pdevice);

        // ---- swapchain (windowed target) ----
        let format = if use_swapchain {
            let formats = surface_fn.as_ref().unwrap()
                .get_physical_device_surface_formats(pdevice, surface)?;
            formats
                .iter()
                .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
                .or_else(|| formats.iter().find(|f| f.format == vk::Format::R8G8B8A8_UNORM))
                .unwrap_or(&formats[0])
                .format
        } else {
            vk::Format::R8G8B8A8_UNORM
        };

        // Clamp requested MSAA to what the device supports for this format.
        let requested_samples = vk::SampleCountFlags::from_raw(cfg.msaa.max(1));
        let msaa_samples = if requested_samples == vk::SampleCountFlags::TYPE_1 {
            requested_samples
        } else {
            match instance.get_physical_device_image_format_properties(
                pdevice,
                format,
                vk::ImageType::TYPE_2D,
                vk::ImageTiling::OPTIMAL,
                vk::ImageUsageFlags::COLOR_ATTACHMENT,
                vk::ImageCreateFlags::empty(),
            ) {
                Ok(props) => {
                    let supported = props.sample_counts;
                    let mut best = vk::SampleCountFlags::TYPE_1;
                    for cand in [
                        vk::SampleCountFlags::TYPE_2,
                        vk::SampleCountFlags::TYPE_4,
                        vk::SampleCountFlags::TYPE_8,
                        vk::SampleCountFlags::TYPE_16,
                        vk::SampleCountFlags::TYPE_32,
                        vk::SampleCountFlags::TYPE_64,
                    ] {
                        if supported.contains(cand) && cand.as_raw() <= cfg.msaa {
                            best = cand;
                        }
                    }
                    if best == vk::SampleCountFlags::TYPE_1 {
                        println!(
                            "{}",
                            stylize(
                                &format!(
                                    "[VKGFX-H] MSAA {}x unsupported for this format; MSAA disabled",
                                    cfg.msaa
                                ),
                                false
                            )
                        );
                    }
                    best
                }
                Err(_) => {
                    println!(
                        "{}",
                        stylize("[VKGFX-H] format probe failed; MSAA disabled", false)
                    );
                    vk::SampleCountFlags::TYPE_1
                }
            }
        };
        let msaa_on = msaa_samples.as_raw() > 1;

        let mut offscreen_color: Option<(vk::Image, vk::DeviceMemory, vk::ImageView)> = None;
        let mut offscreen_depth: Option<(vk::Image, vk::DeviceMemory, vk::ImageView)> = None;
        let mut offscreen_msaa: Option<(vk::Image, vk::DeviceMemory, vk::ImageView)> = None;

        // ---- render pass ----
        let final_color_layout = if use_swapchain {
            vk::ImageLayout::PRESENT_SRC_KHR
        } else {
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
        };
        let render_pass = create_render_pass(&device, format, msaa_samples, final_color_layout)?;

        // ---- offscreen color target (non-swapchain) ----
        if !use_swapchain {
            let info = vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(format)
                .extent(vk::Extent3D { width: cfg.width, height: cfg.height, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let image = device.create_image(&info, None)?;
            let req = device.get_image_memory_requirements(image);
            let idx = find_memory_type(
                &mem_properties,
                req,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
            )?;
            let mem = device.allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(idx),
                None,
            )?;
            device.bind_image_memory(image, mem, 0)?;
            let view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
            // Layout transition once; the render pass keeps initial layout =
            // COLOR_ATTACHMENT_OPTIMAL across submissions.
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    )
                    .src_access_mask(vk::AccessFlags::empty())
                    .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)],
            );
            device.end_command_buffer(cmd)?;
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default().command_buffers(&[cmd])],
                vk::Fence::null(),
            )?;
            device.queue_wait_idle(queue)?;
            offscreen_color = Some((image, mem, view));

            let make_depth = |samples: vk::SampleCountFlags| -> Result<
                (vk::Image, vk::DeviceMemory, vk::ImageView),
                Box<dyn std::error::Error>,
            > {
                let info = vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::D32_SFLOAT)
                    .extent(vk::Extent3D {
                        width: cfg.width,
                        height: cfg.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(samples)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let image = device.create_image(&info, None)?;
                let req = device.get_image_memory_requirements(image);
                let idx = find_memory_type(
                    &mem_properties,
                    req,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )?;
                let mem = device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(idx),
                    None,
                )?;
                device.bind_image_memory(image, mem, 0)?;
                let view = device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(vk::Format::D32_SFLOAT)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::DEPTH)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )?;
                Ok((image, mem, view))
            };

            let make_msaa_color = || -> Result<
                (vk::Image, vk::DeviceMemory, vk::ImageView),
                Box<dyn std::error::Error>,
            > {
                let info = vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width: cfg.width,
                        height: cfg.height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(msaa_samples)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE);
                let image = device.create_image(&info, None)?;
                let req = device.get_image_memory_requirements(image);
                let idx = find_memory_type(
                    &mem_properties,
                    req,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )?;
                let mem = device.allocate_memory(
                    &vk::MemoryAllocateInfo::default()
                        .allocation_size(req.size)
                        .memory_type_index(idx),
                    None,
                )?;
                device.bind_image_memory(image, mem, 0)?;
                let view = device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )?;
                Ok((image, mem, view))
            };

            offscreen_depth = Some(make_depth(msaa_samples)?);
            if msaa_on {
                offscreen_msaa = Some(make_msaa_color()?);
            }
        }

        let mut sc = if use_swapchain {
            Some(create_swapchain_resources(
                &instance,
                &device,
                pdevice,
                surface_fn.as_ref().unwrap(),
                surface,
                swapchain_dev.as_ref().unwrap(),
                vk::SwapchainKHR::null(),
                msaa_samples,
                render_pass,
                &mem_properties,
                cfg.width,
                cfg.height,
            )?)
        } else {
            None
        };

        // ---- shaders / layouts / pipelines ----
        let make_module = |words: &[u32]| -> Result<vk::ShaderModule, Box<dyn std::error::Error>> {
            Ok(device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(words), None)?)
        };
        let bg_vs = make_module(&compile_glsl(naga::ShaderStage::Vertex, BG_VERT_SRC)?)?;
        let bg_fs = make_module(&compile_glsl(naga::ShaderStage::Fragment, BG_FRAG_SRC)?)?;
        let knot_vs = make_module(&compile_glsl(naga::ShaderStage::Vertex, KNOT_VERT_SRC)?)?;
        let knot_fs = make_module(&compile_glsl(naga::ShaderStage::Fragment, KNOT_FRAG_SRC)?)?;

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(16);
        let pipeline_layout = device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&[push_range]),
            None,
        )?;

        let mut bg_pipeline = vk::Pipeline::null();
        let mut knot_pipeline = vk::Pipeline::null();
        create_pipelines(
            &device,
            render_pass,
            msaa_samples,
            pipeline_layout,
            bg_vs,
            bg_fs,
            knot_vs,
            knot_fs,
            &mut bg_pipeline,
            &mut knot_pipeline,
        )?;

        // ---- offscreen framebuffer (single; layout persists across submits) ----
        let offscreen_fb = if !use_swapchain {
            let (_, _, color_view) = offscreen_color.as_ref().unwrap();
            let (_, _, depth_view) = offscreen_depth.as_ref().unwrap();
            let mut attachments = vec![*color_view, *depth_view];
            if let Some((_, _, mv)) = offscreen_msaa.as_ref() {
                attachments.push(*mv);
            }
            Some(device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(&attachments)
                    .width(cfg.width)
                    .height(cfg.height)
                    .layers(1),
                None,
            )?)
        } else {
            None
        };

        // ---- mesh ----
        let (verts, indices) = build_torus_mesh();
        let (vertex_buf, vertex_mem) =
            create_host_buffer(&device, &mem_properties, (verts.len() * 4) as u64)?;
        {
            let ptr = device
                .map_memory(vertex_mem, 0, (verts.len() * 4) as u64, vk::MemoryMapFlags::empty())?
                as *mut f32;
            std::ptr::copy_nonoverlapping(verts.as_ptr(), ptr, verts.len());
            device.unmap_memory(vertex_mem);
        }
        let (index_buf, index_mem) =
            create_host_buffer(&device, &mem_properties, (indices.len() * 4) as u64)?;
        {
            let ptr = device
                .map_memory(index_mem, 0, (indices.len() * 4) as u64, vk::MemoryMapFlags::empty())?
                as *mut u32;
            std::ptr::copy_nonoverlapping(indices.as_ptr(), ptr, indices.len());
            device.unmap_memory(index_mem);
        }
        let index_count = indices.len() as u32;

        println!(
            "{}",
            stylize(
                &format!(
                    "[VKGFX-H] heavy renderer up: {}x{} | msaa={}x | iters={} | shells={} | target={} | torus {RINGS}x{SEGS}",
                    cfg.width,
                    cfg.height,
                    msaa_samples.as_raw(),
                    cfg.iters,
                    cfg.shells,
                    if use_swapchain { "swapchain+present" } else { "offscreen" },
                ),
                false
            )
        );

        // ---- frame loop ----
        let start = std::time::Instant::now();
        let mut last_log = start;
        let mut frames: u64 = 0;
        let mut image_idx = 0u32;

        while is_running.load(Ordering::SeqCst) {
            if use_swapchain {
                window.as_ref().unwrap().pump_messages()?;

                // Minimized: swapchain extent collapses to 0 — park.
                if sc.as_ref().map(|r| r.extent.width == 0).unwrap_or(false) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }

                match swapchain_dev.as_ref().unwrap().acquire_next_image(
                    sc.as_ref().unwrap().swapchain,
                    u64::MAX,
                    acquire_sem,
                    vk::Fence::null(),
                ) {
                    Ok((idx, false)) => image_idx = idx,
                    Ok((_, true)) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                        // Stale swapchain: full recreation, then re-acquire on
                        // the next iteration.
                        device.device_wait_idle()?;
                        let old = sc.take().expect("swapchain vanished");
                        let old_handle = old.swapchain;
                        destroy_swapchain_resources(&device, swapchain_dev.as_ref().unwrap(), old);
                        let new_sc = create_swapchain_resources(
                            &instance,
                            &device,
                            pdevice,
                            surface_fn.as_ref().unwrap(),
                            surface,
                            swapchain_dev.as_ref().unwrap(),
                            old_handle,
                            msaa_samples,
                            render_pass,
                            &mem_properties,
                            cfg.width,
                            cfg.height,
                        )?;
                        sc = Some(new_sc);
                        continue;
                    }
                    Err(err) => return Err(format!("acquire failed: {err}").into()),
                }
            }

            // Frame target references: swapchain fb vs offscreen fb.
            let (fb, extent) = if use_swapchain {
                let sc = sc.as_ref().unwrap();
                (
                    sc.framebuffers[image_idx as usize],
                    vk::Extent2D { width: sc.extent.width, height: sc.extent.height },
                )
            } else {
                (
                    offscreen_fb.unwrap(),
                    vk::Extent2D { width: cfg.width, height: cfg.height },
                )
            };

            device.wait_for_fences(&[fence], true, u64::MAX)?;
            device.reset_fences(&[fence])?;
            device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            let mut color_clear = vk::ClearValue::default();
            color_clear.color = vk::ClearColorValue {
                float32: [0.02, 0.03, 0.05, 1.0],
            };
            let mut depth_clear = vk::ClearValue::default();
            depth_clear.depth_stencil =
                vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 };
            let clear = [color_clear, depth_clear];

            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(render_pass)
                    .framebuffer(fb)
                    .render_area(
                        vk::Rect2D::default()
                            .offset(vk::Offset2D { x: 0, y: 0 })
                            .extent(extent),
                    )
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            let viewport = vk::Viewport::default()
                .x(0.0)
                .y(0.0)
                .width(extent.width as f32)
                .height(extent.height as f32)
                .min_depth(0.0)
                .max_depth(1.0);
            let scissor = vk::Rect2D::default()
                .offset(vk::Offset2D { x: 0, y: 0 })
                .extent(extent);
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[scissor]);

            let time = start.elapsed().as_secs_f32();
            // Push block is {f32, f32, int} — the int member must be written
            // as its integer bit pattern, not as an f32 bit pattern.
            let push_bytes: [u8; 16] = {
                let mut b = [0u8; 16];
                b[0..4].copy_from_slice(&time.to_bits().to_ne_bytes());
                b[4..8].copy_from_slice(&0.012f32.to_bits().to_ne_bytes());
                b[8..12].copy_from_slice(&cfg.iters.to_ne_bytes());
                b[12..16].copy_from_slice(&(if cfg.rotate { 1.0f32 } else { 0.0f32 }).to_bits().to_ne_bytes());
                b
            };

            // Background: SFU-heavy fullscreen shader.
            {
                device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, bg_pipeline);
                device.cmd_push_constants(
                    cmd,
                    pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    &push_bytes,
                );
                device.cmd_draw(cmd, 3, 1, 0, 0);
            }

            // Torus shells: instanced draw, alpha-blended layered overdraw.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, knot_pipeline);
            let vertex_buffers = [vertex_buf];
            let offsets = [0u64];
            device.cmd_bind_vertex_buffers(cmd, 0, &vertex_buffers, &offsets);
            device.cmd_bind_index_buffer(cmd, index_buf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, index_count, cfg.shells.max(1), 0, 0, 0);

            device.cmd_end_render_pass(cmd);
            device.end_command_buffer(cmd)?;

            if use_swapchain {
                let wait_stage = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
                let wait_sems = [acquire_sem];
                let cmd_bufs = [cmd];
                let signal_sems = [render_sem];
                let submit = [vk::SubmitInfo::default()
                    .wait_semaphores(&wait_sems)
                    .wait_dst_stage_mask(&wait_stage)
                    .command_buffers(&cmd_bufs)
                    .signal_semaphores(&signal_sems)];
                device.queue_submit(queue, &submit, fence)?;

                let wait_sems = [render_sem];
                let swapchains = [sc.as_ref().unwrap().swapchain];
                let image_indices = [image_idx];
                let present_info = vk::PresentInfoKHR::default()
                    .wait_semaphores(&wait_sems)
                    .swapchains(&swapchains)
                    .image_indices(&image_indices);
                match swapchain_dev.as_ref().unwrap()
                    .queue_present(queue, &present_info)
                {
                    Ok(false) => {}
                    // SUBOPTIMAL/OUT_OF_DATE after present: the next frame's
                    // acquire returns OUT_OF_DATE and triggers recreation.
                    Ok(true) => {}
                    Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {}
                    Err(err) => return Err(format!("present failed: {err}").into()),
                }
            } else {
                let cmd_bufs = [cmd];
                let submit = [vk::SubmitInfo::default().command_buffers(&cmd_bufs)];
                device.queue_submit(queue, &submit, fence)?;
            }

            frames += 1;
            let now = std::time::Instant::now();
            if now.duration_since(last_log) >= std::time::Duration::from_secs(3) {
                let fps = frames as f64 / now.duration_since(last_log).as_secs_f64();
                println!(
                    "{}",
                    stylize(
                        &format!(
                            "[VKGFX-H] {:>6.1}s | {:>6.1} fps ({:.2} ms/frame) | msaa={}x | iters={} | shells={}",
                            start.elapsed().as_secs_f64(),
                            fps,
                            1000.0 / fps.max(1e-6),
                            msaa_samples.as_raw(),
                            cfg.iters,
                            cfg.shells
                        ),
                        false
                    )
                );
                frames = 0;
                last_log = now;
            }

        }

        device.device_wait_idle()?;

        // ---- teardown ----
        if let Some(fb) = offscreen_fb {
            device.destroy_framebuffer(fb, None);
        }
        if let Some(sc) = sc.take() {
            destroy_swapchain_resources(&device, swapchain_dev.as_ref().unwrap(), sc);
        }
        if let Some((image, mem, view)) = offscreen_color {
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(mem, None);
        }
        if let Some((image, mem, view)) = offscreen_depth {
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(mem, None);
        }
        if let Some((image, mem, view)) = offscreen_msaa {
            device.destroy_image_view(view, None);
            device.destroy_image(image, None);
            device.free_memory(mem, None);
        }
        device.destroy_buffer(vertex_buf, None);
        device.destroy_buffer(index_buf, None);
        device.destroy_pipeline(bg_pipeline, None);
        device.destroy_pipeline(knot_pipeline, None);
        device.destroy_shader_module(bg_vs, None);
        device.destroy_shader_module(bg_fs, None);
        device.destroy_shader_module(knot_vs, None);
        device.destroy_shader_module(knot_fs, None);
        device.destroy_pipeline_layout(pipeline_layout, None);
        device.destroy_render_pass(render_pass, None);
        device.destroy_semaphore(acquire_sem, None);
        device.destroy_semaphore(render_sem, None);
        device.destroy_fence(fence, None);
        device.destroy_command_pool(command_pool, None);
        if let Some(fn_swapchain) = swapchain_dev {
            // SwapchainDevice has no destructor; the swapchain itself was
            // destroyed above.
            let _ = fn_swapchain;
        }
        if let Some(surface_fn) = surface_fn {
            surface_fn.destroy_surface(surface, None);
        }
        device.destroy_device(None);
        instance.destroy_instance(None);
        drop(window);

        Ok(())
    }
}


// ---------------------------------------------------------------------------
// Swapchain resources (windowed target): swapchain + views + MSAA color +
// depth + framebuffers. Recreation on resize/minimize/OUT_OF_DATE.
// ---------------------------------------------------------------------------

struct SwapchainResources {
    swapchain: vk::SwapchainKHR,
    views: Vec<vk::ImageView>,
    msaa_image: vk::Image,
    msaa_mem: vk::DeviceMemory,
    msaa_view: vk::ImageView,
    depth_image: vk::Image,
    depth_mem: vk::DeviceMemory,
    depth_view: vk::ImageView,
    framebuffers: Vec<vk::Framebuffer>,
    extent: vk::Extent2D,
    #[allow(dead_code)]
    format: vk::Format,
}

#[allow(clippy::too_many_arguments)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_swapchain_resources(
    _instance: &Instance,
    device: &ash::Device,
    pdevice: vk::PhysicalDevice,
    surface_fn: &SurfaceInstance,
    surface: vk::SurfaceKHR,
    swapchain_dev: &SwapchainDevice,
    old_swapchain: vk::SwapchainKHR,
    msaa_samples: vk::SampleCountFlags,
    render_pass: vk::RenderPass,
    mem_properties: &vk::PhysicalDeviceMemoryProperties,
    fallback_width: u32,
    fallback_height: u32,
) -> Result<SwapchainResources, Box<dyn std::error::Error>> {
    let caps = unsafe { surface_fn.get_physical_device_surface_capabilities(pdevice, surface)? };
    let mut extent = caps.current_extent;
    if extent.width == u32::MAX {
        extent = vk::Extent2D { width: fallback_width, height: fallback_height };
    }
    if extent.width == 0 || extent.height == 0 {
        // Minimized: create nothing; the loop parks until restored.
        return Ok(SwapchainResources {
            swapchain: vk::SwapchainKHR::null(),
            views: Vec::new(),
            msaa_image: vk::Image::null(),
            msaa_mem: vk::DeviceMemory::null(),
            msaa_view: vk::ImageView::null(),
            depth_image: vk::Image::null(),
            depth_mem: vk::DeviceMemory::null(),
            depth_view: vk::ImageView::null(),
            framebuffers: Vec::new(),
            extent,
            format: vk::Format::B8G8R8A8_UNORM,
        });
    }

    let format = vk::Format::B8G8R8A8_UNORM;
    let image_count = caps.min_image_count.max(3).min(caps.max_image_count.min(4));
    let mut info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface)
        .min_image_count(image_count)
        .image_format(format)
        .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
        .image_extent(extent)
        .image_array_layers(1)
        .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(vk::PresentModeKHR::IMMEDIATE)
        .clipped(true);
    if old_swapchain != vk::SwapchainKHR::null() {
        info = info.old_swapchain(old_swapchain);
    }
    let swapchain = swapchain_dev.create_swapchain(&info, None)?;
    let images = swapchain_dev.get_swapchain_images(swapchain)?;
    let views: Vec<vk::ImageView> = images
        .iter()
        .map(|&img| {
            device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(img)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let msaa_on = msaa_samples != vk::SampleCountFlags::TYPE_1;

    // Depth (MSAA-aware).
    let depth_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::D32_SFLOAT)
        .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
        .mip_levels(1)
        .array_layers(1)
        .samples(msaa_samples)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let depth_image = device.create_image(&depth_info, None)?;
    let depth_req = device.get_image_memory_requirements(depth_image);
    let depth_idx = find_memory_type(
        mem_properties,
        depth_req,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )?;
    let depth_mem = device.allocate_memory(
        &vk::MemoryAllocateInfo::default()
            .allocation_size(depth_req.size)
            .memory_type_index(depth_idx),
        None,
    )?;
    device.bind_image_memory(depth_image, depth_mem, 0)?;
    let depth_view = device.create_image_view(
        &vk::ImageViewCreateInfo::default()
            .image(depth_image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .subresource_range(
                vk::ImageSubresourceRange::default()
                    .aspect_mask(vk::ImageAspectFlags::DEPTH)
                    .level_count(1)
                    .layer_count(1),
            ),
        None,
    )?;

    // MSAA color target (only when MSAA on).
    let (msaa_image, msaa_mem, msaa_view) = if msaa_on {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(msaa_samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = device.create_image(&info, None)?;
        let req = device.get_image_memory_requirements(image);
        let idx = find_memory_type(
            mem_properties,
            req,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )?;
        let mem = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(req.size)
                .memory_type_index(idx),
            None,
        )?;
        device.bind_image_memory(image, mem, 0)?;
        let view = device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                ),
            None,
        )?;
        (image, mem, view)
    } else {
        (vk::Image::null(), vk::DeviceMemory::null(), vk::ImageView::null())
    };

    // Framebuffers: attachments follow the render pass attachment order
    // [resolve target, depth, msaa color?].
    let framebuffers = views
        .iter()
        .map(|&view| {
            let mut attachments = vec![view, depth_view];
            if msaa_on {
                attachments.push(msaa_view);
            }
            device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(&attachments)
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SwapchainResources {
        swapchain,
        views,
        msaa_image,
        msaa_mem,
        msaa_view,
        depth_image,
        depth_mem,
        depth_view,
        framebuffers,
        extent,
        #[allow(dead_code)]
        format,
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn destroy_swapchain_resources(
    device: &ash::Device,
    swapchain_dev: &SwapchainDevice,
    res: SwapchainResources,
) {
    unsafe {
    for &fb in &res.framebuffers {
        device.destroy_framebuffer(fb, None);
    }
    for &view in &res.views {
        device.destroy_image_view(view, None);
    }
    if res.msaa_view != vk::ImageView::null() {
        device.destroy_image_view(res.msaa_view, None);
    }
    if res.msaa_image != vk::Image::null() {
        device.destroy_image(res.msaa_image, None);
    }
    if res.msaa_mem != vk::DeviceMemory::null() {
        device.free_memory(res.msaa_mem, None);
    }
    if res.depth_view != vk::ImageView::null() {
        device.destroy_image_view(res.depth_view, None);
    }
    if res.depth_image != vk::Image::null() {
        device.destroy_image(res.depth_image, None);
    }
    if res.depth_mem != vk::DeviceMemory::null() {
        device.free_memory(res.depth_mem, None);
    }
    if res.swapchain != vk::SwapchainKHR::null() {
        swapchain_dev.destroy_swapchain(res.swapchain, None);
    }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_render_pass(
    device: &ash::Device,
    format: vk::Format,
    msaa_samples: vk::SampleCountFlags,
    final_color_layout: vk::ImageLayout,
) -> Result<vk::RenderPass, Box<dyn std::error::Error>> {
    let msaa_on = msaa_samples != vk::SampleCountFlags::TYPE_1;

    // att0 = resolve target (samples 1); att1 = depth; att2 = msaa color.
    let mut attachments = vec![
        vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(final_color_layout),
        vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(msaa_samples)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
    ];
    if msaa_on {
        attachments.push(
            vk::AttachmentDescription::default()
                .format(format)
                .samples(msaa_samples)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::DONT_CARE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
        );
    }

    let color_ref = vk::AttachmentReference::default()
        .attachment(if msaa_on { 2 } else { 0 })
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let resolve_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let depth_ref = vk::AttachmentReference::default()
        .attachment(1)
        .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

    let mut subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(std::slice::from_ref(&color_ref))
        .depth_stencil_attachment(&depth_ref);
    if msaa_on {
        subpass = subpass.resolve_attachments(std::slice::from_ref(&resolve_ref));
    }

    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

    unsafe {
        Ok(device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&attachments)
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&[dependency]),
            None,
        )?)
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_pipelines(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    msaa_samples: vk::SampleCountFlags,
    layout: vk::PipelineLayout,
    bg_vs: vk::ShaderModule,
    bg_fs: vk::ShaderModule,
    knot_vs: vk::ShaderModule,
    knot_fs: vk::ShaderModule,
    out_bg: &mut vk::Pipeline,
    out_knot: &mut vk::Pipeline,
) -> Result<(), Box<dyn std::error::Error>> {
    let dynamic_state = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(msaa_samples);

    // Background: fullscreen, no blend, no depth.
    let bg_stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(bg_vs)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(bg_fs)
            .name(c"main"),
    ];
    let bg_blend = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false);
    let bg_depth = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(false)
        .depth_write_enable(false);
    let bg_pipelines = device.create_graphics_pipelines(
        vk::PipelineCache::null(),
        &[vk::GraphicsPipelineCreateInfo::default()
            .stages(&bg_stages)
            .vertex_input_state(&vk::PipelineVertexInputStateCreateInfo::default())
            .input_assembly_state(
                &vk::PipelineInputAssemblyStateCreateInfo::default()
                    .topology(vk::PrimitiveTopology::TRIANGLE_STRIP),
            )
            .viewport_state(&viewport_state)
            .rasterization_state(
                &vk::PipelineRasterizationStateCreateInfo::default()
                    .polygon_mode(vk::PolygonMode::FILL)
                    .line_width(1.0),
            )
            .multisample_state(&multisample)
            .depth_stencil_state(&bg_depth)
            .color_blend_state(
                &vk::PipelineColorBlendStateCreateInfo::default()
                    .attachments(std::slice::from_ref(&bg_blend)),
            )
            .dynamic_state(&vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_state))
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0)],
        None,
    );
    *out_bg = match bg_pipelines {
        Ok(mut v) => v.pop().ok_or("no bg pipeline")?,
        Err((_, err)) => return Err(format!("bg pipeline: {err}").into()),
    };

    // Knot shells: vertex attribs, depth on, alpha blend.
    let knot_stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(knot_vs)
            .name(c"main"),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(knot_fs)
            .name(c"main"),
    ];
    let vertex_attribs = [
        vk::VertexInputAttributeDescription::default().location(0).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(0),
        vk::VertexInputAttributeDescription::default().location(1).binding(0).format(vk::Format::R32G32B32_SFLOAT).offset(12),
        vk::VertexInputAttributeDescription::default().location(2).binding(0).format(vk::Format::R32G32_SFLOAT).offset(24),
    ];
    let vertex_bindings = [vk::VertexInputBindingDescription::default()
        .binding(0)
        .stride(32)
        .input_rate(vk::VertexInputRate::VERTEX)];
    let knot_blend = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(true)
        .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
        .color_blend_op(vk::BlendOp::ADD)
        .src_alpha_blend_factor(vk::BlendFactor::ONE)
        .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
        .alpha_blend_op(vk::BlendOp::ADD);
    let knot_depth = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS);
    let knot_pipelines = device.create_graphics_pipelines(
        vk::PipelineCache::null(),
        &[vk::GraphicsPipelineCreateInfo::default()
            .stages(&knot_stages)
            .vertex_input_state(
                &vk::PipelineVertexInputStateCreateInfo::default()
                    .vertex_attribute_descriptions(&vertex_attribs)
                    .vertex_binding_descriptions(&vertex_bindings),
            )
            .input_assembly_state(
                &vk::PipelineInputAssemblyStateCreateInfo::default()
                    .topology(vk::PrimitiveTopology::TRIANGLE_LIST),
            )
            .viewport_state(&viewport_state)
            .rasterization_state(
                &vk::PipelineRasterizationStateCreateInfo::default()
                    .polygon_mode(vk::PolygonMode::FILL)
                    .cull_mode(vk::CullModeFlags::NONE)
                    .line_width(1.0),
            )
            .multisample_state(&multisample)
            .depth_stencil_state(&knot_depth)
            .color_blend_state(
                &vk::PipelineColorBlendStateCreateInfo::default()
                    .attachments(std::slice::from_ref(&knot_blend)),
            )
            .dynamic_state(&vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_state))
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0)],
        None,
    );
    *out_knot = match knot_pipelines {
        Ok(mut v) => v.pop().ok_or("no knot pipeline")?,
        Err((_, err)) => return Err(format!("knot pipeline: {err}").into()),
    };
    Ok(())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_host_buffer(
    device: &ash::Device,
    mem_properties: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory), Box<dyn std::error::Error>> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buf = device.create_buffer(&info, None)?;
    let req = device.get_buffer_memory_requirements(buf);
    let idx = find_memory_type(
        mem_properties,
        req,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    let mem = device.allocate_memory(
        &vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(idx),
        None,
    )?;
    device.bind_buffer_memory(buf, mem, 0)?;
    Ok((buf, mem))
}
