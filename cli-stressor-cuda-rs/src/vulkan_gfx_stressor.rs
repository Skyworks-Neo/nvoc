use ash::vk;
use rand::Rng;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct VulkanGraphicsEngine {
	is_running: Arc<AtomicBool>,
	has_error: Arc<AtomicBool>,
	thread_handle: Option<thread::JoinHandle<()>>,
}

impl VulkanGraphicsEngine {
	pub fn new() -> Self {
		Self {
			is_running: Arc::new(AtomicBool::new(false)),
			has_error: Arc::new(AtomicBool::new(false)),
			thread_handle: None,
		}
	}

	pub fn start_stress_thread(&mut self) -> Result<(), Box<dyn std::error::Error>> {
		let is_running = self.is_running.clone();
		let has_error = self.has_error.clone();

		is_running.store(true, Ordering::SeqCst);
		has_error.store(false, Ordering::SeqCst);

		let handle = thread::spawn(move || {
			if let Err(e) = run_vulkan_stress_loop(is_running) {
				eprintln!("[VulkanGfx] Thread crashed: {:?}", e);
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

	pub fn stop(&mut self) {
		self.is_running.store(false, Ordering::SeqCst);
		if let Some(handle) = self.thread_handle.take() {
			let _ = handle.join();
		}
	}
}

fn run_vulkan_stress_loop(is_running: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
	unsafe {
		let entry = ash::Entry::load()?;
		let app_name = CStr::from_bytes_with_nul_unchecked(b"HeadlessStressor\0");
		let app_info = vk::ApplicationInfo::default()
			.application_name(app_name)
			.api_version(vk::API_VERSION_1_2);

		let instance_create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
		let instance = entry.create_instance(&instance_create_info, None)?;

		let pdevices = instance.enumerate_physical_devices()?;
		if pdevices.is_empty() {
			return Err("No Vulkan physical devices found".into());
		}
		let pdevice = pdevices[0];

		let queue_family_properties = instance.get_physical_device_queue_family_properties(pdevice);
		let graphics_queue_index = queue_family_properties
			.iter()
			.position(|info| info.queue_flags.contains(vk::QueueFlags::GRAPHICS))
			.expect("No Graphics queue family found") as u32;

		let queue_priorities = [1.0];
		let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
			.queue_family_index(graphics_queue_index)
			.queue_priorities(&queue_priorities)];

		let device_create_info = vk::DeviceCreateInfo::default()
			.queue_create_infos(&queue_create_infos);
		let device = instance.create_device(pdevice, &device_create_info, None)?;
		let queue = device.get_device_queue(graphics_queue_index, 0);

		let pool_create_info = vk::CommandPoolCreateInfo::default()
			.queue_family_index(graphics_queue_index)
			.flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
		let command_pool = device.create_command_pool(&pool_create_info, None)?;

		let cmd_buf_alloc_info = vk::CommandBufferAllocateInfo::default()
			.command_pool(command_pool)
			.level(vk::CommandBufferLevel::PRIMARY)
			.command_buffer_count(1);
		let cmd_buffer = device.allocate_command_buffers(&cmd_buf_alloc_info)?[0];

		let fence_create_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
		let fence = device.create_fence(&fence_create_info, None)?;

		// ==========================================
		// 模块：极高压内存与 ROP 占据
		let image_extent = vk::Extent3D { width: 8192, height: 8192, depth: 1 };
		let image_create_info = vk::ImageCreateInfo::default()
			.image_type(vk::ImageType::TYPE_2D)
			.format(vk::Format::R8G8B8A8_UNORM)
			.extent(image_extent)
			.mip_levels(1)
			.array_layers(1)
			.samples(vk::SampleCountFlags::TYPE_1)
			.tiling(vk::ImageTiling::OPTIMAL)
			.usage(
				vk::ImageUsageFlags::TRANSFER_SRC |
				vk::ImageUsageFlags::TRANSFER_DST |
				vk::ImageUsageFlags::COLOR_ATTACHMENT
			)
			.sharing_mode(vk::SharingMode::EXCLUSIVE);

		let image_count = 6; // ~1.5GB region
		let mut images = Vec::new();
		let mut memories = Vec::new();

		let mem_properties = instance.get_physical_device_memory_properties(pdevice);

		for _ in 0..image_count {
			let img = device.create_image(&image_create_info, None)?;
			let mem_req = device.get_image_memory_requirements(img);

			let mut mem_type_idx = 0;
			for i in 0..mem_properties.memory_type_count {
				if (mem_req.memory_type_bits & (1 << i)) != 0 {
					if mem_properties.memory_types[i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL) {
						mem_type_idx = i;
						break;
					}
				}
			}

			let alloc_info = vk::MemoryAllocateInfo::default()
				.allocation_size(mem_req.size)
				.memory_type_index(mem_type_idx);
			let mem = device.allocate_memory(&alloc_info, None)?;
			device.bind_image_memory(img, mem, 0)?;
			images.push(img);
			memories.push(mem);
		}

		// Convert layout to GENERAL
		{
			device.reset_command_buffer(cmd_buffer, vk::CommandBufferResetFlags::empty())?;
			let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
			device.begin_command_buffer(cmd_buffer, &begin_info)?;

			let subresource_range = vk::ImageSubresourceRange::default()
				.aspect_mask(vk::ImageAspectFlags::COLOR)
				.level_count(1)
				.layer_count(1);
			let mut barriers = Vec::new();
			for &img in &images {
				barriers.push(vk::ImageMemoryBarrier::default()
					.old_layout(vk::ImageLayout::UNDEFINED)
					.new_layout(vk::ImageLayout::GENERAL)
					.src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
					.dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
					.image(img)
					.subresource_range(subresource_range)
					.src_access_mask(vk::AccessFlags::empty())
					.dst_access_mask(vk::AccessFlags::TRANSFER_WRITE));
			}
			device.cmd_pipeline_barrier(
				cmd_buffer,
				vk::PipelineStageFlags::TOP_OF_PIPE,
				vk::PipelineStageFlags::TRANSFER,
				vk::DependencyFlags::empty(),
				&[], &[], &barriers
			);
			device.end_command_buffer(cmd_buffer)?;
			let cmd_buffers = [cmd_buffer];
			let submits = [vk::SubmitInfo::default().command_buffers(&cmd_buffers)];
			device.queue_submit(queue, &submits, vk::Fence::null())?;
			device.queue_wait_idle(queue)?;
		}

		let subresource_range = vk::ImageSubresourceRange::default()
			.aspect_mask(vk::ImageAspectFlags::COLOR)
			.level_count(1)
			.layer_count(1);

		let mut rng = rand::rng();
		let stress_start = std::time::Instant::now();
		let mut last_log = stress_start;
		let mut window_submits: u64 = 0;
		let mut pipeline_flushes: u64 = 0;

		while is_running.load(Ordering::SeqCst) {
			device.wait_for_fences(&[fence], true, u64::MAX)?;
			device.reset_fences(&[fence])?;
			device.reset_command_buffer(cmd_buffer, vk::CommandBufferResetFlags::empty())?;

			let begin_info = vk::CommandBufferBeginInfo::default()
				.flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
			device.begin_command_buffer(cmd_buffer, &begin_info)?;

			// Heavy work
			for _ in 0..20 {
				let c_val: f32 = rng.random_range(0.0..1.0);
				let clear_color = vk::ClearColorValue { float32: [c_val, 1.0 - c_val, c_val * 0.5, 1.0] };
				let target_img_idx = rng.random_range(0..image_count);
				device.cmd_clear_color_image(
					cmd_buffer,
					images[target_img_idx],
					vk::ImageLayout::GENERAL,
					&clear_color,
					&[subresource_range]
				);

				let src_idx = (target_img_idx + 1) % image_count;
				let dst_idx = (target_img_idx + 2) % image_count;
				let blit = vk::ImageBlit::default()
					.src_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
					.src_offsets([
						vk::Offset3D { x: 0, y: 0, z: 0 },
						vk::Offset3D { x: 8192, y: 8192, z: 1 }
					])
					.dst_subresource(vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
					.dst_offsets([
						vk::Offset3D { x: 0, y: 0, z: 0 },
						vk::Offset3D { x: 8192, y: 8192, z: 1 }
					]);

				device.cmd_blit_image(
					cmd_buffer,
					images[src_idx],
					vk::ImageLayout::GENERAL,
					images[dst_idx],
					vk::ImageLayout::GENERAL,
					&[blit],
					vk::Filter::NEAREST
				);

				let memory_barrier = vk::MemoryBarrier::default()
					.src_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
					.dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);

				device.cmd_pipeline_barrier(
					cmd_buffer,
					vk::PipelineStageFlags::ALL_COMMANDS,
					vk::PipelineStageFlags::ALL_COMMANDS,
					vk::DependencyFlags::empty(),
					&[memory_barrier], &[], &[],
				);
				pipeline_flushes += 1;
			}

			device.end_command_buffer(cmd_buffer)?;

			let cmd_buffers = [cmd_buffer];
			let submit_info = vk::SubmitInfo::default()
				.command_buffers(&cmd_buffers);

			device.queue_submit(queue, &[submit_info], fence)?;
			window_submits += 1;

			let now = std::time::Instant::now();
			let log_elapsed = now.duration_since(last_log);
			if log_elapsed >= Duration::from_secs(3) {
				let elapsed_s = stress_start.elapsed().as_secs_f64();
				let submits_per_s = window_submits as f64 / log_elapsed.as_secs_f64().max(1e-6);
				println!(
					"[Vulkan GFX] {:>6.1}s | {:>5.1} submits/s (randomized interval) | Active DWM preemption stress | Pipeline Flushes: {}",
					elapsed_s,
					submits_per_s,
					pipeline_flushes
				);
				window_submits = 0;
				last_log = now;
			}

			let r = rng.random_range(0..100);
			let sleep_time = if r < 2 {
				Duration::from_millis(20)
			} else {
				Duration::from_millis(rng.random_range(4..16))
			};
			thread::sleep(sleep_time);
		}

		device.device_wait_idle()?;

		for &img in &images {
			device.destroy_image(img, None);
		}
		for &mem in &memories {
			device.free_memory(mem, None);
		}

		device.destroy_fence(fence, None);
		device.destroy_command_pool(command_pool, None);
		device.destroy_device(None);
		instance.destroy_instance(None);

		Ok(())
	}
}

