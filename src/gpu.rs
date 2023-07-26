pub struct GpuContext {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuContext {
    pub async fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    features: Default::default(),
                    limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .unwrap();
        // let shader = include_wgsl!("kernels/circuit.wgsl");
        Self {
            adapter,
            device,
            queue,
        }
    }
}
