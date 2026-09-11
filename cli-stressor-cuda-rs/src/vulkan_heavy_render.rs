//! Heavy FurMark-style Vulkan render loop (Windows).
//!
//! Purpose-built graphics-domain stimulus, complementing the light
//! clear/blit path in `vulkan_gfx_stressor.rs`. What it exercises that the
//! light path never touches:
//! - programmable shading: fragment shader with dense sin/cos chains (SFU
//!   paths) + vertex shell displacement (instanced)
//! - rasterizer + attribute interpolator (dense torus mesh, instanced
//!   shells = layered overdraw)
//! - ROP alpha blending (read-modify-write) + depth test/write
//! - display engine: real Win32 window + swapchain present (no offscreen)
//! - ~100% duty cycle (no sleeps in this loop)
//!
//! Shaders are GLSL translated to SPIR-V at init time via naga (no external
//! toolchain needed).

use super::style::stylize;
use super::vulkan_gfx_stressor::{select_gpu_by_cuda_identity, VulkanDeviceSelection, VulkanImageConfig};
use ash::khr::surface::Instance as SurfaceInstance;
use ash::khr::swapchain::Device as SwapchainDevice;
use ash::khr::win32_surface::Instance as Win32SurfaceInstance;
use ash::vk;
use ash::Instance;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const HEAVY_WIDTH: u32 = 1280;
const HEAVY_HEIGHT: u32 = 720;
const SHELLS: u32 = 8;
const RINGS: u32 = 180;
const SEGS: u32 = 90;

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
layout(push_constant) uniform Push { float u_time; float shell_off; } pc;
void main() {
    vec2 p = v_uv * 2.0 - 1.0;
    float s = 0.0;
    for (int i = 0; i < 24; ++i) {
        float fi = float(i);
        float a = 0.7 + fi * 0.05;
        mat2 rot = mat2(cos(a), -sin(a), sin(a), cos(a));
        p = rot * p;
        s += 0.012 * sin(p.x * (10.0 + fi) + pc.u_time * (1.0 + fi * 0.13)) *
             cos(p.y * (9.0 + fi * 0.5) - pc.u_time * 0.7);
    }
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
layout(push_constant) uniform Push { float u_time; float shell_off; } pc;
void main() {
    vec3 p = in_pos + in_normal * pc.shell_off * float(gl_InstanceIndex);
    v_normal = in_normal;
    v_pos = p;
    gl_Position = vec4(p, 1.0);
}
"#;

const KNOT_FRAG_SRC: &str = r#"
#version 450 core
layout(location = 0) in vec3 v_normal;
layout(location = 1) in vec3 v_pos;
layout(location = 0) out vec4 out_color;
layout(push_constant) uniform Push { float u_time; float shell_off; } pc;
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

fn compile_glsl(stage: naga::ShaderStage, source: &str) -> Result<Vec<u32>, Box<dyn std::error::Error>> {
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
    let words = naga::back::spv::write_vec(&module, &info, &spv_options, Some(&pipeline))?;
    Ok(words)
}

// ---------------------------------------------------------------------------
// Win32 window (fixed size, no resize handling — swapchain is static)
// ---------------------------------------------------------------------------

struct Win32Window {
    hwnd: isize,
}

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

            let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
            AdjustWindowRect(&mut rect, WS_OVERLAPPEDWINDOW, 0);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;

            let hwnd = CreateWindowExW(
                0,
                CLASS_NAME.as_ptr(),
                {
                    const TITLE: &[u16] = &[
                        b'n' as u16, b'v' as u16, b'o' as u16, b'c' as u16, b' ' as u16,
                        b'v' as u16, b'k' as u16, b' ' as u16, b'h' as u16, b'e' as u16,
                        b'a' as u16, b'v' as u16, b'y' as u16, 0,
                    ];
                    TITLE.as_ptr()
                },
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

impl Drop for Win32Window {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
        unsafe {
            if self.hwnd != 0 {
                DestroyWindow(self.hwnd as _);
            }
        }
    }
}

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



#[allow(dead_code)]
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

fn create_host_buffer(
    device: &ash::Device,
    mem_properties: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory), Box<dyn std::error::Error>> {
    let info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::VERTEX_BUFFER | vk::BufferUsageFlags::INDEX_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buf = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buf) };
    let idx = find_memory_type(
        mem_properties,
        req,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(idx);
    let mem = unsafe { device.allocate_memory(&alloc, None)? };
    unsafe { device.bind_buffer_memory(buf, mem, 0)? };
    Ok((buf, mem))
}

#[allow(clippy::needless_range_loop)]
fn build_torus_mesh() -> (Vec<f32>, Vec<u32>) {
    let rings = RINGS;
    let segs = SEGS;
    let (r_major, r_minor) = (1.0f32, 0.35f32);
    let mut verts = Vec::with_capacity((rings * segs * 8) as usize);
    for i in 0..rings {
        let u = (i as f32 / rings as f32) * std::f32::consts::TAU;
        let cu = u.cos();
        let su = u.sin();
        for j in 0..segs {
            let v = (j as f32 / segs as f32) * std::f32::consts::TAU;
            let cv = v.cos();
            let sv = v.sin();
            let nx = cu * cv;
            let ny = sv;
            let nz = su * cv;
            let px = (r_major + r_minor * cv) * cu;
            let py = r_minor * sv;
            let pz = (r_major + r_minor * cv) * su;
            verts.extend_from_slice(&[px, py, pz, nx, ny, nz, u / std::f32::consts::TAU, v / std::f32::consts::TAU]);
        }
    }
    let mut indices = Vec::with_capacity((rings * segs * 6) as usize);
    for i in 0..rings {
        for j in 0..segs {
            let a = i * segs + j;
            let b = i * segs + (j + 1) % segs;
            let c = ((i + 1) % rings) * segs + (j + 1) % segs;
            let d = ((i + 1) % rings) * segs + j;
            indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    (verts, indices)
}

#[allow(clippy::too_many_arguments)]
pub fn run_heavy_render_loop(
    is_running: Arc<AtomicBool>,
    selection: Option<VulkanDeviceSelection>,
    _image_config: VulkanImageConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let window = Win32Window::new(HEAVY_WIDTH, HEAVY_HEIGHT)?;
        let hwnd = window.hwnd();
        let hinstance = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(
            std::ptr::null(),
        ) as isize;

        let entry = ash::Entry::load()?;
        let surface_exts = [
            ash::khr::surface::NAME.as_ptr(),
            ash::khr::win32_surface::NAME.as_ptr(),
        ];
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"nvoc-vk-heavy")
            .api_version(vk::API_VERSION_1_2);
        let instance_create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&surface_exts);
        let instance: Instance = entry.create_instance(&instance_create_info, None)?;

        let pdevice = if let Some(selection) = selection {
            select_gpu_by_cuda_identity(
                &instance,
                selection.cuda_uuid,
                selection.cuda_pci_bus,
            )
            .map_err(|err| format!("[VKGFX-H] Vulkan GPU selection failed: {err}"))?
        } else {
            let pdevices = instance.enumerate_physical_devices()?;
            *pdevices.first().ok_or("no Vulkan physical devices")?
        };

        let surface_fn = SurfaceInstance::new(&entry, &instance);
        let surface_info = vk::Win32SurfaceCreateInfoKHR::default()
            .hinstance(hinstance)
            .hwnd(hwnd);
        let win32_fn = Win32SurfaceInstance::new(&entry, &instance);
        let surface = win32_fn.create_win32_surface(&surface_info, None)?;

        let families = instance.get_physical_device_queue_family_properties(pdevice);
        let queue_family = (0..families.len() as u32)
            .find(|&f| {
                families[f as usize]
                    .queue_flags
                    .contains(vk::QueueFlags::GRAPHICS)
                    && surface_fn
                        .get_physical_device_surface_support(pdevice, f, surface)
                        .unwrap_or(false)
            })
            .ok_or("no graphics+present queue family")?;

        let queue_priorities = [1.0];
        let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&queue_priorities)];
        let device_exts = [ash::khr::swapchain::NAME.as_ptr()];
        let device_create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .enabled_extension_names(&device_exts);
        let device = instance.create_device(pdevice, &device_create_info, None)?;
        let queue = device.get_device_queue(queue_family, 0);
        let swapchain_dev = SwapchainDevice::new(&instance, &device);

        // ---- surface formats / present modes ----
        let formats = surface_fn.get_physical_device_surface_formats(pdevice, surface)?;
        let format = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .or_else(|| {
                formats
                    .iter()
                    .find(|f| f.format == vk::Format::R8G8B8A8_UNORM)
            })
            .unwrap_or(&formats[0])
            .format;
        let present_modes =
            surface_fn.get_physical_device_surface_present_modes(pdevice, surface)?;
        let present_mode = present_modes
            .iter()
            .find(|m| **m == vk::PresentModeKHR::IMMEDIATE)
            .copied()
            .unwrap_or(vk::PresentModeKHR::FIFO);

        let caps = surface_fn.get_physical_device_surface_capabilities(pdevice, surface)?;
        let extent = if caps.current_extent.width == u32::MAX {
            vk::Extent2D { width: HEAVY_WIDTH, height: HEAVY_HEIGHT }
        } else {
            caps.current_extent
        };
        let image_count = caps.min_image_count.max(3).min(caps.max_image_count.min(4));

        let swapchain_info = vk::SwapchainCreateInfoKHR::default()
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
            .present_mode(present_mode)
            .clipped(true);
        let swapchain = swapchain_dev.create_swapchain(&swapchain_info, None)?;
        let swapchain_images = swapchain_dev.get_swapchain_images(swapchain)?;
        let image_count = swapchain_images.len();

        // ---- command pool / sync ----
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = device.create_command_pool(&pool_info, None)?;
        let cmd = device.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1))?[0];
        let fence = device.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )?;
        let acquire_sem = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
        let render_sem = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;

        // ---- depth image ----
        let mem_properties = instance.get_physical_device_memory_properties(pdevice);
        let depth_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::D32_SFLOAT)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let depth_image = device.create_image(&depth_info, None)?;
        let depth_req = device.get_image_memory_requirements(depth_image);
        let depth_mem_type =
            find_memory_type(&mem_properties, depth_req, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        let depth_mem = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(depth_req.size)
                .memory_type_index(depth_mem_type),
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

        // ---- render pass (color + depth) ----
        let color_attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let depth_attachment = vk::AttachmentDescription::default()
            .format(vk::Format::D32_SFLOAT)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let color_ref = vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let depth_ref = vk::AttachmentReference::default()
            .attachment(1)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(std::slice::from_ref(&color_ref))
            .depth_stencil_attachment(&depth_ref);
        let dependency = vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
        let render_pass = device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&[color_attachment, depth_attachment])
                .subpasses(std::slice::from_ref(&subpass))
                .dependencies(&[dependency]),
            None,
        )?;

        // ---- shaders ----
        let make_module = |words: &[u32]| -> Result<vk::ShaderModule, Box<dyn std::error::Error>> {
            let info = vk::ShaderModuleCreateInfo::default().code(words);
            Ok(device.create_shader_module(&info, None)?)
        };
        let bg_vs = make_module(&compile_glsl(naga::ShaderStage::Vertex, BG_VERT_SRC)?)?;
        let bg_fs = make_module(&compile_glsl(naga::ShaderStage::Fragment, BG_FRAG_SRC)?)?;
        let knot_vs = make_module(&compile_glsl(naga::ShaderStage::Vertex, KNOT_VERT_SRC)?)?;
        let knot_fs = make_module(&compile_glsl(naga::ShaderStage::Fragment, KNOT_FRAG_SRC)?)?;

        let push_range = vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(8);
        let pipeline_layout = device.create_pipeline_layout(
            &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&[push_range]),
            None,
        )?;

        let dynamic_state = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let viewport_state = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);

        // Background pipeline: fullscreen triangle, no blend, no depth.
        let bg_stage_create = [
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
                .stages(&bg_stage_create)
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
                .multisample_state(
                    &vk::PipelineMultisampleStateCreateInfo::default()
                        .rasterization_samples(vk::SampleCountFlags::TYPE_1),
                )
                .depth_stencil_state(&bg_depth)
                .color_blend_state(
                    &vk::PipelineColorBlendStateCreateInfo::default()
                        .attachments(std::slice::from_ref(&bg_blend)),
                )
                .dynamic_state(&vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_state))
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0)],
            None,
        );
        let bg_pipeline = match bg_pipelines {
            Ok(mut v) => v.pop().ok_or("no bg pipeline")?,
            Err((_, err)) => return Err(format!("bg pipeline: {err}").into()),
        };

        // Knot pipeline: vertex attribs, instanced shells, depth on, alpha blend.
        let knot_stage_create = [
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
                .stages(&knot_stage_create)
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
                .multisample_state(
                    &vk::PipelineMultisampleStateCreateInfo::default()
                        .rasterization_samples(vk::SampleCountFlags::TYPE_1),
                )
                .depth_stencil_state(&knot_depth)
                .color_blend_state(
                    &vk::PipelineColorBlendStateCreateInfo::default()
                        .attachments(std::slice::from_ref(&knot_blend)),
                )
                .dynamic_state(&vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_state))
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0)],
            None,
        );
        let knot_pipeline = match knot_pipelines {
            Ok(mut v) => v.pop().ok_or("no knot pipeline")?,
            Err((_, err)) => return Err(format!("knot pipeline: {err}").into()),
        };

        // ---- framebuffers + mesh ----
        let swapchain_views: Vec<vk::ImageView> = swapchain_images
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
        let framebuffers: Vec<vk::Framebuffer> = swapchain_views
            .iter()
            .map(|&view| {
                device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(render_pass)
                        .attachments(&[view, depth_view])
                        .width(extent.width)
                        .height(extent.height)
                        .layers(1),
                    None,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        let (verts, indices) = build_torus_mesh();
        let (vertex_buf, vertex_mem) = create_host_buffer(
            &device,
            &mem_properties,
            (verts.len() * 4) as u64,
        )?;
        {
            let ptr = device
                .map_memory(vertex_mem, 0, (verts.len() * 4) as u64, vk::MemoryMapFlags::empty())?
                as *mut f32;
            std::ptr::copy_nonoverlapping(verts.as_ptr(), ptr, verts.len());
            device.unmap_memory(vertex_mem);
        }
        let (index_buf, index_mem) = create_host_buffer(
            &device,
            &mem_properties,
            (indices.len() * 4) as u64,
        )?;
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
                    "[VKGFX-H] heavy renderer up: {}x{} | swapchain images={image_count} | present={present_mode:?} | torus {RINGS}x{SEGS} x {SHELLS} shells",
                    extent.width, extent.height
                ),
                false
            )
        );

        // ---- frame loop: acquire → bg + instanced shells → submit → present ----
        let start = std::time::Instant::now();
        let mut last_log = start;
        let mut frames: u64 = 0;
        let viewport = vk::Viewport::default()
            .x(0.0)
            .y(0.0)
            .width(extent.width as f32)
            .height(extent.height as f32)
            .min_depth(0.0)
            .max_depth(1.0);
        let scissor = vk::Rect2D::default().offset(vk::Offset2D { x: 0, y: 0 }).extent(extent);

        while is_running.load(Ordering::SeqCst) {
            window.pump_messages()?;
            device.wait_for_fences(&[fence], true, u64::MAX)?;
            device.reset_fences(&[fence])?;

            let (image_idx, _) = swapchain_dev
                .acquire_next_image(swapchain, u64::MAX, acquire_sem, vk::Fence::null())
                .map_err(|err| format!("acquire failed: {err}"))?;

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
            depth_clear.depth_stencil = vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 };
            let clear = [color_clear, depth_clear];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(render_pass)
                    .framebuffer(framebuffers[image_idx as usize])
                    .render_area(vk::Rect2D::default().offset(vk::Offset2D { x: 0, y: 0 }).extent(extent))
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            device.cmd_set_viewport(cmd, 0, &[viewport]);
            device.cmd_set_scissor(cmd, 0, &[scissor]);

            let time = start.elapsed().as_secs_f32();
            let push = [time, 0.012f32];

            // Background: SFU-heavy fullscreen shader.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, bg_pipeline);
            device.cmd_push_constants(
                cmd,
                pipeline_layout,
                vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                0,
                std::slice::from_raw_parts(push.as_ptr().cast::<u8>(), 8),
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);

            // Torus shells: instanced draw, alpha-blended layered overdraw.
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, knot_pipeline);
            let vertex_buffers = [vertex_buf];
            let offsets = [0u64];
            device.cmd_bind_vertex_buffers(cmd, 0, &vertex_buffers, &offsets);
            device.cmd_bind_index_buffer(cmd, index_buf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, index_count, SHELLS, 0, 0, 0);

            device.cmd_end_render_pass(cmd);
            device.end_command_buffer(cmd)?;

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
            let swapchains = [swapchain];
            let image_indices = [image_idx];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&wait_sems)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            swapchain_dev
                .queue_present(queue, &present_info)
                .map_err(|err| format!("present failed: {err}"))?;
            frames += 1;

            let now = std::time::Instant::now();
            if now.duration_since(last_log) >= std::time::Duration::from_secs(3) {
                let fps = frames as f64 / now.duration_since(last_log).as_secs_f64();
                println!(
                    "{}",
                    stylize(
                        &format!("[VKGFX-H] {:>6.1}s | {:>6.1} fps | present={present_mode:?}", start.elapsed().as_secs_f64(), fps),
                        false
                    )
                );
                frames = 0;
                last_log = now;
            }

        }

        device.device_wait_idle()?;

        // ---- teardown ----
        for &fb in &framebuffers {
            device.destroy_framebuffer(fb, None);
        }
        for &view in &swapchain_views {
            device.destroy_image_view(view, None);
        }
        device.destroy_image_view(depth_view, None);
        device.destroy_image(depth_image, None);
        device.free_memory(depth_mem, None);
        device.destroy_buffer(vertex_buf, None);
        device.free_memory(vertex_mem, None);
        device.destroy_buffer(index_buf, None);
        device.free_memory(index_mem, None);
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
        swapchain_dev.destroy_swapchain(swapchain, None);
        surface_fn.destroy_surface(surface, None);
        device.destroy_device(None);
        instance.destroy_instance(None);
        drop(window);

        Ok(())
    }
}


