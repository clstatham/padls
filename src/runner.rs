use std::sync::{Arc, RwLock};

use petgraph::visit::{IntoEdgeReferences, IntoNodeReferences};
use wgpu::util::DeviceExt;

use crate::{
    bit::Bit,
    gates::Gate,
    gpu::GpuContext,
    parser::{Circuit, Wire},
};

pub struct CircuitRunner {
    gpu: Arc<GpuContext>,
    pub state: Arc<RwLock<Vec<Bit>>>,
}

impl CircuitRunner {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let gpu = Arc::new(pollster::block_on(GpuContext::new()));
        let state = Arc::new(RwLock::new(Vec::new()));
        Self { gpu, state }
    }

    pub fn set_bit(&self, idx: usize, bit: Bit) {
        let mut state = self.state.write().unwrap();
        state[idx] = bit;
    }

    pub fn get_bit(&self, idx: usize) -> Bit {
        let state = self.state.read().unwrap();
        state[idx]
    }

    pub fn run(&self, circ: &Circuit) -> anyhow::Result<()> {
        let node_count = circ.node_count();
        let input_idxs = circ
            .inputs
            .iter()
            .map(|binding| circ.bindings[binding].index())
            .collect::<Vec<_>>();

        let mut state = self.state.write().unwrap();
        *state = vec![Bit::LO; node_count];
        drop(state);

        let state_gpu = vec![0u32; node_count];

        let mut modes = vec![0u32; node_count];

        for (idx, gate) in circ.graph.node_references() {
            modes[idx.index()] = match gate {
                Gate::Identity => 0,
                Gate::Not => 1,
                Gate::And => 2,
                Gate::Or => 3,
                Gate::Xor => 4,
            };
        }

        let mut connections = vec![[u32::MAX, u32::MAX]; circ.node_count()];

        for (src, dst, wire) in circ.graph.edge_references() {
            let Wire {
                source_output,
                target_input,
            } = wire;
            assert_eq!(source_output, &0);
            connections[dst.index()][*target_input as usize] = src.index() as u32;
        }

        let modes_buffer = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("modes"),
                contents: bytemuck::cast_slice(&modes),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let connections_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("connections"),
                    contents: bytemuck::cast_slice(&connections),
                    usage: wgpu::BufferUsages::STORAGE,
                });

        let state_buffer = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("state"),
                contents: bytemuck::cast_slice(&state_gpu),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            });

        let state_out_buffer =
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("state_out"),
                    contents: bytemuck::cast_slice(&state_gpu),
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST,
                });

        let state_staging_buffer = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("state_staging"),
            size: (node_count * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            self.gpu
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("bind_group_layout"),
                    entries: &[
                        // state
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // state_out
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: false },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // connections
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // modes
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::COMPUTE,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Storage { read_only: true },
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                    ],
                });

        let bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bind_group"),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: state_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: state_out_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: connections_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: modes_buffer.as_entire_binding(),
                    },
                ],
            });

        let gpu = self.gpu.clone();

        let shader = wgpu::include_wgsl!("circuit.wgsl");
        let module = gpu.device.create_shader_module(shader);

        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let pipeline = gpu
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("pipeline"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: "main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            });

        let state = self.state.clone();

        std::thread::Builder::new()
            .name("PADLS GPU Thread".to_string())
            .spawn(move || loop {
                let mut encoder =
                    gpu.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("circuit command encoder"),
                        });

                {
                    let state = state.read().unwrap();
                    let state_gpu = state.iter().map(|&b| b.as_u8() as u32).collect::<Vec<_>>();
                    gpu.queue
                        .write_buffer(&state_buffer, 0, bytemuck::cast_slice(&state_gpu));
                }

                gpu.device.poll(wgpu::Maintain::Wait);

                {
                    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("circuit compute pass"),
                        timestamp_writes: None,
                    });
                    cpass.set_pipeline(&pipeline);
                    cpass.set_bind_group(0, &bind_group, &[]);
                    cpass.dispatch_workgroups(node_count as u32, 1, 1);
                }

                encoder.copy_buffer_to_buffer(
                    &state_out_buffer,
                    0,
                    &state_staging_buffer,
                    0,
                    (node_count * std::mem::size_of::<u32>()) as u64,
                );

                gpu.queue.submit(std::iter::once(encoder.finish()));

                let buffer_slice = state_staging_buffer.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
                    tx.send(result).unwrap();
                });
                gpu.device.poll(wgpu::Maintain::Wait);
                let _ = rx.recv().unwrap();

                let data = buffer_slice.get_mapped_range();
                let data_slice: &[u32] = bytemuck::cast_slice(&data);

                {
                    let mut state = state.write().unwrap();
                    for (idx, &val) in data_slice.iter().enumerate() {
                        if input_idxs.contains(&idx) {
                            continue;
                        }
                        state[idx] = match val {
                            0 => Bit::LO,
                            1 => Bit::HI,
                            _ => unreachable!("invalid state value"),
                        };
                    }
                }

                drop(data);

                state_staging_buffer.unmap();
            })?;

        Ok(())
    }
}
