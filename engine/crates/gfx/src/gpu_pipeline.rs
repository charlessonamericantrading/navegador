pub struct WebGpuPipeline {
    pub adapter_name: String,
    pub backend_name: String,
}

impl WebGpuPipeline {
    pub fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }));

        if let Some(adapter) = adapter {
            let info = adapter.get_info();
            Self {
                adapter_name: info.name,
                backend_name: format!("{:?}", info.backend),
            }
        } else {
            Self {
                adapter_name: "Software Engine Rasterizer".to_string(),
                backend_name: "CPU Fallback".to_string(),
            }
        }
    }
}
