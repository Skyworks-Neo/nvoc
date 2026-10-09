use crate::runner::style::stylize;
use crate::vulkan_render::run_render_loop;
use anstream::eprintln;
use ash::{Instance, vk};
use cli_stressor_cuda_rs::PciBusAddress;
use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VulkanDeviceSelection {
    pub cuda_uuid: [u8; 16],
    pub cuda_pci_bus: Option<PciBusAddress>,
}

/// FurMark-style heavy render parameters. Defined here (rather than in the
/// `vulkan_render` module) so the config type exists on every platform; the
/// offscreen render loop runs on all of them and only windowed presentation
/// stays Windows-only.
#[derive(Clone, Copy, Debug)]
pub struct VulkanRenderConfig {
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
    /// Compute->graphics particle pool size (Lumen/TSR-style SSBO ping-pong).
    /// 0 disables the stage.
    pub particles: u32,
}

pub struct VulkanGraphicsEngine {
    is_running: Arc<AtomicBool>,
    has_error: Arc<AtomicBool>,
    selection: Option<VulkanDeviceSelection>,
    render_config: VulkanRenderConfig,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl VulkanGraphicsEngine {
    pub fn new(render_config: VulkanRenderConfig) -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            has_error: Arc::new(AtomicBool::new(false)),
            selection: None,
            render_config,
            thread_handle: None,
        }
    }

    #[cfg(feature = "cuda")]
    pub fn with_selection(
        selection: VulkanDeviceSelection,
        render_config: VulkanRenderConfig,
    ) -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            has_error: Arc::new(AtomicBool::new(false)),
            selection: Some(selection),
            render_config,
            thread_handle: None,
        }
    }

    pub fn start_stress_thread(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let is_running = self.is_running.clone();
        let has_error = self.has_error.clone();
        let selection = self.selection;
        let render_config = self.render_config;

        is_running.store(true, Ordering::SeqCst);
        has_error.store(false, Ordering::SeqCst);

        let handle = thread::spawn(move || {
            if let Err(e) = run_render_loop(is_running, selection, render_config) {
                eprintln!(
                    "{}",
                    stylize(&format!("[VulkanGfx] Thread crashed: {:?}", e), true)
                );
                has_error.store(true, Ordering::SeqCst);
            }
        });

        self.thread_handle = Some(handle);
        Ok(())
    }

    /// Return a clone of the internal error flag Arc so callers can monitor it
    /// without taking ownership of the engine itself.
    pub fn get_error_flag_arc(&self) -> Arc<AtomicBool> {
        self.has_error.clone()
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.is_running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take()
            && handle.join().is_err()
        {
            self.has_error.store(true, Ordering::SeqCst);
            return Err(std::io::Error::other("Vulkan stress thread panicked").into());
        }
        Ok(())
    }
}

pub fn select_gpu_by_cuda_uuid(
    instance: &Instance,
    target_cuda_uuid: [u8; 16],
) -> Result<vk::PhysicalDevice, String> {
    select_gpu_by_cuda_identity(instance, target_cuda_uuid, None)
}

pub fn select_gpu_by_cuda_identity(
    instance: &Instance,
    target_cuda_uuid: [u8; 16],
    target_cuda_pci: Option<PciBusAddress>,
) -> Result<vk::PhysicalDevice, String> {
    let pdevices = unsafe {
        instance
            .enumerate_physical_devices()
            .map_err(|err| format!("failed to enumerate Vulkan physical devices: {err}"))?
    };
    if pdevices.is_empty() {
        return Err("no Vulkan physical devices found".to_string());
    }

    let target_uuid_hex = format_uuid_hex(&target_cuda_uuid);
    let target_uuid_valid = !is_zero_uuid(&target_cuda_uuid);
    let target_pci_hex = target_cuda_pci.as_ref().map(format_pci_address);
    println!(
        "{}",
        stylize(
            &format!(
                "[VulkanGfx] Target CUDA UUID: {}{}",
                target_uuid_hex,
                if let Some(pci) = &target_pci_hex {
                    format!(" | target PCI: {pci}")
                } else {
                    String::new()
                }
            ),
            false
        )
    );

    let mut pci_fallback_candidate: Option<vk::PhysicalDevice> = None;

    for pdevice in pdevices {
        let props = unsafe { query_vulkan_device_identity(instance, pdevice) };
        let (device_name, device_uuid, device_uuid_hex, device_pci) = match props {
            Ok(value) => value,
            Err(err) => {
                println!(
                    "{}",
                    stylize(
                        &format!("[VulkanGfx] Failed to query device identity: {err}"),
                        false
                    )
                );
                continue;
            }
        };

        println!(
            "{}",
            stylize(
                &format!(
                    "[VulkanGfx] Checking device: {} | uuid={} | pci={}",
                    device_name,
                    device_uuid_hex,
                    device_pci
                        .as_ref()
                        .map(format_pci_address)
                        .unwrap_or_else(|| "<none>".to_string())
                ),
                false
            )
        );

        if target_uuid_valid && !is_zero_uuid(&device_uuid) && device_uuid == target_cuda_uuid {
            println!(
                "{}",
                stylize(
                    &format!("[VulkanGfx] Selected Vulkan device by UUID match: {device_name}"),
                    false
                )
            );
            return Ok(pdevice);
        }

        if pci_fallback_candidate.is_none()
            && let (Some(target_pci), Some(device_pci)) =
                (target_cuda_pci.as_ref(), device_pci.as_ref())
            && target_pci == device_pci
        {
            pci_fallback_candidate = Some(pdevice);
        }
    }

    if let Some(pdevice) = pci_fallback_candidate {
        return Ok(pdevice);
    }

    if target_uuid_valid {
        Err(format!(
            "no Vulkan physical device matched CUDA UUID {target_uuid_hex}"
        ))
    } else if target_cuda_pci.is_some() {
        Err(
            "CUDA UUID was empty and no Vulkan device matched the fallback PCIe bus address"
                .to_string(),
        )
    } else {
        Err("CUDA UUID was empty and no PCIe fallback information was provided".to_string())
    }
}

fn is_zero_uuid(uuid: &[u8; 16]) -> bool {
    uuid.iter().all(|b| *b == 0)
}

fn format_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

fn format_pci_address(pci: &PciBusAddress) -> String {
    format!(
        "{:04X}:{:02X}:{:02X}.{}",
        pci.domain, pci.bus, pci.device, pci.function
    )
}

unsafe fn query_vulkan_device_identity(
    instance: &Instance,
    pdevice: vk::PhysicalDevice,
) -> Result<(String, [u8; 16], String, Option<PciBusAddress>), String> {
    let mut id_properties = vk::PhysicalDeviceIDProperties::default();
    let mut pci_properties = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default()
        // push_next wires extension-owned property structs into the query chain so the
        // driver writes them directly into Rust-owned memory during get_physical_device_properties2().
        .push_next(&mut id_properties)
        .push_next(&mut pci_properties);
    unsafe {
        instance.get_physical_device_properties2(pdevice, &mut properties2);
    }

    let device_name = unsafe {
        CStr::from_ptr(properties2.properties.device_name.as_ptr())
            .to_string_lossy()
            .into_owned()
    };
    let device_uuid = id_properties.device_uuid;
    let device_uuid_hex = format_uuid_hex(&device_uuid);
    let pci = if pci_properties.pci_domain == 0
        && pci_properties.pci_bus == 0
        && pci_properties.pci_device == 0
        && pci_properties.pci_function == 0
    {
        None
    } else {
        Some(PciBusAddress {
            domain: pci_properties.pci_domain,
            bus: pci_properties.pci_bus,
            device: pci_properties.pci_device,
            function: pci_properties.pci_function,
        })
    };

    Ok((device_name, device_uuid, device_uuid_hex, pci))
}
