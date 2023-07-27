use std::{collections::hash_map::Entry, fs::File, path::PathBuf, sync::Arc};

use anyhow::{Error, Result};
use petgraph::{
    dot::Dot,
    prelude::*,
    visit::{Control, DfsEvent},
};
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::watch::Receiver;
use wgpu::{
    include_wgsl,
    util::{DeviceExt, StagingBelt},
};

use crate::{
    bit::{ABitBehavior, Bit, SpawnResult},
    gates::{BinaryGate, OwnedBinaryGate, OwnedUnaryGate, UnaryGate},
    gpu::GpuContext,
    parser::{self, Binding},
    CLOCK_FULL_INTERVAL, NUM_DISPLAYS,
};

#[derive(Clone)]
pub struct BinaryOpNode {
    id: NodeIndex,
    op: BinaryGate,
    name: Option<Binding>,
}

impl std::fmt::Debug for BinaryOpNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ", self.id.index())?;
        if let Some(name) = &self.name {
            write!(f, "{}", name)?;
        } else {
            write!(f, "???")?;
        }
        write!(f, " ({:?})", self.op)
    }
}

pub struct Circuit {
    pub name: String,
    graph: DiGraph<BinaryOpNode, UnaryGate>,
    pub(crate) owned_unaries: FxHashMap<EdgeIndex, OwnedUnaryGate>,
    pub(crate) owned_binaries: FxHashMap<NodeIndex, OwnedBinaryGate>,
    pub node_bindings: FxHashMap<NodeIndex, Binding>,
    input_nodes: Vec<NodeIndex>,
    output_nodes: Vec<NodeIndex>,
}

impl<'b, 'a: 'b> Circuit {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            graph: DiGraph::new(),
            owned_unaries: FxHashMap::default(),
            owned_binaries: FxHashMap::default(),
            node_bindings: FxHashMap::default(),
            input_nodes: Vec::default(),
            output_nodes: Vec::default(),
        }
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn input_nodes(&self) -> &Vec<NodeIndex> {
        &self.input_nodes
    }

    pub fn output_nodes(&self) -> &Vec<NodeIndex> {
        &self.output_nodes
    }

    pub fn add_default_node(&mut self, name: Option<Binding>) -> NodeIndex {
        let id = self.graph.add_node(BinaryOpNode {
            id: 0.into(),
            op: BinaryGate::AlwaysA,
            name,
        });
        self.graph[id].id = id;
        id
    }

    pub fn add_node(&mut self, op: BinaryGate, name: Option<Binding>) -> NodeIndex {
        let id = self.graph.add_node(BinaryOpNode {
            id: 0.into(),
            op,
            name,
        });
        self.graph[id].id = id;
        id
    }

    pub fn add_input(&mut self, name: Option<Binding>) -> NodeIndex {
        let idx = self.add_default_node(name);
        self.input_nodes.push(idx);
        idx
    }

    pub fn add_output(&mut self, name: Option<Binding>) -> NodeIndex {
        let idx = self.add_default_node(name);
        self.output_nodes.push(idx);
        idx
    }

    pub fn set_node_op(&mut self, idx: NodeIndex, op: BinaryGate) {
        self.graph[idx].op = op;
    }

    pub fn node_by_binding(&self, name: &Binding) -> Option<NodeIndex> {
        self.node_bindings
            .iter()
            .find_map(|(k, v)| if v == name { Some(k.to_owned()) } else { None })
    }

    pub fn connect_with(
        &mut self,
        op: BinaryGate,
        input_a: NodeIndex,
        input_op_a: UnaryGate,
        input_b: NodeIndex,
        input_op_b: UnaryGate,
        is_output: bool,
    ) -> Result<NodeIndex> {
        let idx = {
            self.graph.add_node(BinaryOpNode {
                id: 0.into(),
                op,
                name: None,
            })
        };
        self.graph[idx].id = idx;

        let _edge_a = { self.connect(input_a, idx, input_op_a) };

        let _edge_b = { self.connect(input_b, idx, input_op_b) };

        if is_output {
            self.output_nodes.push(idx);
        }
        Ok(idx)
    }

    pub fn connect(&mut self, source: NodeIndex, sink: NodeIndex, op: UnaryGate) -> EdgeIndex {
        self.graph.add_edge(source, sink, op)
    }

    fn flesh_out_graph(&mut self) {
        let inputs = self
            .graph
            .node_indices()
            .filter(|node| self.graph.edges(*node).count() > 0);
        #[allow(clippy::iter_nth_zero)]
        #[allow(clippy::map_entry)] // stfu clippy, that's way less readable
        petgraph::visit::depth_first_search(&self.graph, inputs, |event| {
            match event {
                DfsEvent::Discover(node_id, _t) => {
                    let op = &self.graph[node_id];
                    if !self.owned_binaries.contains_key(&node_id) {
                        self.owned_binaries
                            .insert(node_id, OwnedBinaryGate::new(node_id, op.op));
                    }
                }
                DfsEvent::TreeEdge(source, target) => {
                    let mut edges = self.graph.edges_connecting(source, target);
                    let mut edge_id = edges.next().unwrap().id();
                    if self.owned_unaries.contains_key(&edge_id) {
                        if let Some(edge) = edges.next() {
                            if self.owned_unaries.contains_key(&edge.id()) {
                                // ??? alright then
                                return Control::Continue::<()>;
                            } else {
                                edge_id = edge.id();
                            }
                        } else {
                            return Control::Continue::<()>;
                        }
                    }
                    // println!("TreeEdge {:?} => {:?} => {:?}", source, edge_id, target);
                    let edge_op = self.graph[edge_id];
                    let src_op = self.owned_binaries.get_mut(&source).unwrap();
                    let mut edge = OwnedUnaryGate::new(edge_id, edge_op);
                    edge.set_input(source, src_op.subscribe());
                    self.owned_unaries.insert(edge_id, edge);
                }
                DfsEvent::BackEdge(source, target) => {
                    assert_eq!(self.graph.edges_connecting(source, target).count(), 1);
                    let mut edges = self.graph.edges_connecting(source, target);
                    let edge_id = edges.next().unwrap().id();

                    if !self.owned_unaries.contains_key(&edge_id) {
                        let edge_op = self.graph[edge_id];
                        // println!("BackEdge {:?} => {:?} => {:?}", source, edge_id, target);
                        let src_op = self.owned_binaries.get_mut(&source).unwrap();
                        let mut edge = OwnedUnaryGate::new(edge_id, edge_op);
                        edge.set_input(source, src_op.subscribe());
                        self.owned_unaries.insert(edge_id, edge);
                    }
                }

                DfsEvent::CrossForwardEdge(source, target) => {
                    // connect B
                    let mut edges = self.graph.edges_connecting(source, target);
                    let edge = edges.next().unwrap();
                    let edge_id = edge.id();
                    if !self.owned_unaries.contains_key(&edge_id) {
                        let edge_op = self.graph[edge_id];
                        // println!("XFEdge {:?} => {:?} => {:?}", source, edge_id, target);
                        let mut edge = OwnedUnaryGate::new(edge_id, edge_op);
                        let src_op = self.owned_binaries.get_mut(&source).unwrap();
                        edge.set_input(source, src_op.subscribe());
                        self.owned_unaries.insert(edge_id, edge);
                    }
                }
                DfsEvent::Finish(node_id, _t) => {
                    let binary = self.owned_binaries.get_mut(&node_id).unwrap();
                    let mut edges = self.graph.edges_directed(node_id, Direction::Incoming);
                    if let Some(a) = edges.next() {
                        if let Some(a_op) = self.owned_unaries.get(&a.id()) {
                            let a_out = a_op.subscribe();
                            binary.set_input_a(Some(a.id()), a_out);
                        }
                        if let Some(b) = edges.next() {
                            if let Some(b_op) = self.owned_unaries.get(&b.id()) {
                                let b_out = b_op.subscribe();
                                binary.set_input_b(Some(b.id()), b_out);
                            }
                        } else if let Some(a_op) = self.owned_unaries.get(&a.id()) {
                            let a_out = a_op.subscribe();
                            binary.set_input_b(Some(a.id()), a_out);
                        }
                    }
                }
            }
            Control::Continue::<()>
        });
    }

    pub async fn spawn_gpu(
        &mut self,
        gpu: Arc<GpuContext>,
        inputs: FxHashMap<NodeIndex, (Receiver<Bit>, Receiver<Bit>)>,
    ) -> Option<FxHashMap<NodeIndex, Receiver<Bit>>> {
        assert_eq!(inputs.len(), self.input_nodes.len());
        assert!(self.input_nodes.iter().all(|val| inputs.contains_key(val)));

        let clk = self.node_by_binding(&Binding::Clk).unwrap();
        self.input_nodes.push(clk);
        self.owned_binaries
            .insert(clk, OwnedBinaryGate::new(clk, self.graph[clk].op));

        let always_hi = self.node_by_binding(&Binding::Hi).unwrap();
        self.input_nodes.push(always_hi);
        self.owned_binaries.insert(
            always_hi,
            OwnedBinaryGate::new(always_hi, self.graph[always_hi].op),
        );
        let always_lo = self.node_by_binding(&Binding::Lo).unwrap();
        self.input_nodes.push(always_lo);
        self.owned_binaries.insert(
            always_lo,
            OwnedBinaryGate::new(always_lo, self.graph[always_lo].op),
        );
        self.flesh_out_graph();
        for (inp_idx, (inp_a, inp_b)) in inputs.iter() {
            let binary = self.owned_binaries.get_mut(inp_idx).unwrap();
            binary.set_input_a(None, inp_a.clone());
            binary.set_input_b(None, inp_b.clone());
        }
        self.flesh_out_graph();

        let mut outputs_tx = FxHashMap::default();
        let mut outputs_rx = FxHashMap::default();

        let output_nodes = self.output_nodes().clone();
        for (b_idx, _binary) in self.owned_binaries.iter_mut() {
            let (tx, rx) = tokio::sync::watch::channel(Bit::LO);
            if output_nodes.contains(b_idx) {
                if let Entry::Vacant(e) = outputs_tx.entry(*b_idx) {
                    e.insert(tx);
                }
                if let Entry::Vacant(e) = outputs_rx.entry(*b_idx) {
                    e.insert(rx);
                }
            }
        }

        let mut binaries = std::mem::take(&mut self.owned_binaries);
        let unaries = std::mem::take(&mut self.owned_unaries);

        let mut clk = binaries.remove(&clk).unwrap();
        // todo
        // clk.spawn_eager();

        let shader = gpu
            .device
            .create_shader_module(include_wgsl!("kernels/circuit.wgsl"));
        let compute_pipeline =
            gpu.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &shader,
                    entry_point: "main",
                });

        const ERROR: u32 = 987654321;
        const NONE: u32 = 123456789;
        const MODE_IDENTITY: u32 = 0;
        const MODE_NOT: u32 = 1;
        const MODE_AND: u32 = 2;
        const MODE_OR: u32 = 3;
        const MODE_XOR: u32 = 4;
        const MODE_ALWAYSA: u32 = 5;
        const MODE_ALWAYSB: u32 = 6;
        const MODE_ALWAYSLO: u32 = 7;
        const MODE_ALWAYSHI: u32 = 8;

        let state_len = binaries.len() + unaries.len();
        let state_size = state_len * std::mem::size_of::<u32>();
        let state_size = state_size as wgpu::BufferAddress;
        let state_vec = vec![0u32; state_len];
        let mut modes_vec = vec![0u32; state_len];
        let mut inputs_vec = vec![NONE; state_len * 2];
        let mut binary_indices = FxHashMap::default();
        let mut unary_indices = FxHashMap::default();
        let mut i = 0;
        for (idx, binary) in binaries.iter() {
            binary_indices.insert(*idx, i);
            modes_vec[i] = match binary.gate {
                BinaryGate::AlwaysA => MODE_ALWAYSA,
                BinaryGate::And => MODE_AND,
                BinaryGate::Or => MODE_OR,
                BinaryGate::Xor => MODE_XOR,
                BinaryGate::IgnoreInput(bh) => match bh {
                    ABitBehavior::AlwaysHi => MODE_ALWAYSHI,
                    ABitBehavior::AlwaysLo => MODE_ALWAYSLO,
                    ABitBehavior::Clock { half_period } => todo!(),
                    ABitBehavior::Normal { value } => todo!(),
                },
            };
            i += 1;
        }
        for (idx, unary) in unaries.iter() {
            unary_indices.insert(*idx, i);
            modes_vec[i] = match unary.gate {
                UnaryGate::Identity => MODE_IDENTITY,
                UnaryGate::Not => MODE_NOT,
            };
            i += 1;
        }
        // now the inputs, since we know what everything's index is
        for (idx, binary) in binaries.iter() {
            let shader_idx = binary_indices[idx];
            let in_a_idx = if let Some(id) = binary.inp_a_id {
                unary_indices[&id] as u32
            } else {
                NONE
            };
            let in_b_idx = if let Some(id) = binary.inp_b_id {
                unary_indices[&id] as u32
            } else {
                NONE
            };
            inputs_vec[shader_idx * 2] = in_a_idx;
            inputs_vec[shader_idx * 2 + 1] = in_b_idx;
        }
        for (idx, unary) in unaries.iter() {
            let shader_idx = unary_indices[idx];
            let in_idx = if let Some(id) = unary.inp_idx {
                binary_indices[&id] as u32
            } else {
                NONE
            };
            inputs_vec[shader_idx * 2] = in_idx;
            inputs_vec[shader_idx * 2 + 1] = NONE;
        }

        let state_buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("State"),
                contents: bytemuck::cast_slice(&state_vec),
                usage: wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::STORAGE,
            });
        let inputs_buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Inputs"),
                contents: bytemuck::cast_slice(&inputs_vec),
                usage: wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::STORAGE,
            });
        let modes_buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Modes"),
                contents: bytemuck::cast_slice(&modes_vec),
                usage: wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::STORAGE,
            });
        let state_readable_buffer =
            gpu.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("State Staging Readable"),
                    contents: bytemuck::cast_slice(&state_vec),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                });

        tokio::spawn(async move {
            loop {
                let bind_group_layout = compute_pipeline.get_bind_group_layout(0);
                let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: state_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: inputs_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: modes_buffer.as_entire_binding(),
                        },
                    ],
                });

                let mut encoder = gpu
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

                {
                    let mut cpass =
                        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None });
                    cpass.set_pipeline(&compute_pipeline);
                    cpass.set_bind_group(0, &bind_group, &[]);
                    cpass.insert_debug_marker("begin circuit compute pass");
                    cpass.dispatch_workgroups(state_len as u32, 1, 1);
                }
                encoder.copy_buffer_to_buffer(
                    &state_buffer,
                    0,
                    &state_readable_buffer,
                    0,
                    state_size,
                );
                gpu.queue.submit(Some(encoder.finish()));

                let state_readable_slice = state_readable_buffer.slice(..);
                let (tx, read_rx) = tokio::sync::oneshot::channel();
                state_readable_slice
                    .map_async(wgpu::MapMode::Read, move |state| tx.send(state).unwrap());

                gpu.device.poll(wgpu::Maintain::Wait);

                if let Ok(Ok(())) = read_rx.await {
                    let state_readable_data = state_readable_slice.get_mapped_range();
                    let mut state: Vec<u32> = bytemuck::cast_slice(&state_readable_data).to_vec();
                    drop(state_readable_data);
                    state_readable_buffer.unmap();
                    for (idx, tx) in outputs_tx.iter() {
                        if let Some(id) = binary_indices.get(idx) {
                            let bit = state[*id];
                            let bit = if bit == 0 { Bit::LO } else { Bit::HI };
                            tx.send(bit).unwrap();
                        }
                    }

                    for (idx, (inp_a, _inp_b)) in inputs.iter() {
                        let bit = *inp_a.borrow();
                        if let Some(id) = binary_indices.get(idx) {
                            state[*id] = if bit == Bit::LO { 0 } else { 1 };
                        }
                    }

                    gpu.queue
                        .write_buffer(&state_buffer, 0, bytemuck::cast_slice(&state));
                }

                tokio::task::yield_now().await;
            }
        });

        Some(outputs_rx)
    }

    pub fn spawn_cpu(
        &mut self,
        inputs: FxHashMap<NodeIndex, (Receiver<Bit>, Receiver<Bit>)>,
    ) -> Option<FxHashMap<NodeIndex, Receiver<Bit>>> {
        assert_eq!(inputs.len(), self.input_nodes.len());
        assert!(self.input_nodes.iter().all(|val| inputs.contains_key(val)));

        let clk = self.node_by_binding(&Binding::Clk).unwrap();
        self.input_nodes.push(clk);
        self.owned_binaries
            .insert(clk, OwnedBinaryGate::new(clk, self.graph[clk].op));

        let always_hi = self.node_by_binding(&Binding::Hi).unwrap();
        self.input_nodes.push(always_hi);
        self.owned_binaries.insert(
            always_hi,
            OwnedBinaryGate::new(always_hi, self.graph[always_hi].op),
        );
        let always_lo = self.node_by_binding(&Binding::Lo).unwrap();
        self.input_nodes.push(always_lo);
        self.owned_binaries.insert(
            always_lo,
            OwnedBinaryGate::new(always_lo, self.graph[always_lo].op),
        );
        self.flesh_out_graph();
        for (inp_idx, (inp_a, inp_b)) in inputs.into_iter() {
            let binary = self.owned_binaries.get_mut(&inp_idx).unwrap();
            binary.set_input_a(None, inp_a);
            binary.set_input_b(None, inp_b);
        }
        self.flesh_out_graph();

        let mut outputs = FxHashMap::default();

        let output_nodes = self.output_nodes().clone();
        for (b_idx, binary) in self.owned_binaries.iter_mut() {
            let rx = binary.subscribe();
            if output_nodes.contains(b_idx) {
                if let Entry::Vacant(e) = outputs.entry(*b_idx) {
                    e.insert(rx);
                }
            }
        }

        let mut binaries = std::mem::take(&mut self.owned_binaries);
        let unaries = std::mem::take(&mut self.owned_unaries);
        let mut clk = binaries.remove(&clk).unwrap();
        clk.spawn_eager();
        // tokio::spawn(async move {
        for (_, mut bin) in binaries.into_iter() {
            // tokio::spawn(async move {
            //     loop {
            //         bin.spawn_eager().await;
            //         // crate::beat().await;
            //         // tokio::task::yield_now().await;
            //     }
            // });
            if let SpawnResult::NotConnected = bin.spawn_eager() {
                eprintln!("ERROR: {:?} couldn't spawn", bin.idx);
                return None;
            }
        }
        for (_, mut un) in unaries.into_iter() {
            // tokio::spawn(async move {
            // loop {
            //     un.lazy_update().await;
            //     // crate::beat().await;
            //     // tokio::task::yield_now().await;
            // }
            if let SpawnResult::NotConnected = un.spawn_eager() {
                eprintln!("ERROR: {:?} couldn't spawn", un.idx);
                return None;
            }
            // });
        }
        // });

        Some(outputs)
    }

    pub fn dump_ops(&self) {
        let bind = |idx| {
            self.node_bindings
                .get(&idx)
                .map(|bind| format!("{}", bind))
                .unwrap_or("???".to_owned())
        };
        let edge_inp_bind = |idx| {
            self.owned_unaries
                .get(&idx)
                .and_then(|edge| edge.inp_idx)
                .map(bind)
                .unwrap_or("???".to_owned())
        };
        petgraph::visit::depth_first_search(
            &self.graph,
            self.input_nodes().iter().copied(),
            |event| {
                #[allow(clippy::single_match)]
                match event {
                    DfsEvent::Discover(node, _) => {
                        let node = &self.owned_binaries[&node];
                        println!("Node ID {:?} ({}):", node.idx, bind(node.idx),);
                        println!("    Op: {:?}", node.gate);
                        println!(
                            "    Input A: {:?}",
                            node.inp_a_id.map(|idx| (idx, edge_inp_bind(idx)))
                        );
                        println!(
                            "    Input B: {:?}",
                            node.inp_b_id.map(|idx| (idx, edge_inp_bind(idx)))
                        );
                        println!(
                            "    PBit: {:?}",
                            node.bit.as_ref().map(|handle| handle.id())
                        )
                    }
                    DfsEvent::TreeEdge(src, target) | DfsEvent::CrossForwardEdge(src, target) => {
                        let edges = self.graph.edges_connecting(src, target);
                        for edge in edges {
                            let edge = &self.owned_unaries[&edge.id()];
                            println!(
                                "Edge ID {:?}:\n    Op: {:?}\n    {:?} ({:?}) -> {:?}\n    PBit: {:?}",
                                edge.idx,
                                edge.gate,
                                edge.inp_idx,
                                edge.inp_idx.map(bind),
                                target,
                                edge.bit.as_ref().map(|handle| handle.id())
                            );
                        }
                    }
                    _ => {}
                }
                Control::Continue::<()>
            },
        );
    }

    fn parse_expr(
        &mut self,
        expr: &parser::Expr,
        parsed_circs: &'b FxHashMap<String, Circuit>,
    ) -> Result<Option<Vec<NodeIndex>>> {
        let idxs = match expr {
            parser::Expr::Binding(bind) => Some(vec![self.node_by_binding(bind).unwrap()]),
            parser::Expr::Not(expr) => {
                let not_expr = self.parse_expr(&expr.expr, parsed_circs)?;
                let not_expr = if let Some(not_expr) = not_expr {
                    not_expr
                } else {
                    return Ok(None);
                };
                assert_eq!(not_expr.len(), 1);
                let connection = self.graph.add_node(BinaryOpNode {
                    id: 0.into(),
                    op: BinaryGate::AlwaysA,
                    name: None,
                });
                self.graph[connection].id = connection;
                self.connect(not_expr[0], connection, UnaryGate::Not);
                Some(vec![connection])
            }
            parser::Expr::BinaryExpr(expr) => {
                let a_expr = self.parse_expr(&expr.a, parsed_circs)?;
                let a_expr = if let Some(a_expr) = a_expr {
                    a_expr
                } else {
                    return Ok(None);
                };
                let b_expr = self.parse_expr(&expr.b, parsed_circs)?;
                let b_expr = if let Some(b_expr) = b_expr {
                    b_expr
                } else {
                    return Ok(None);
                };
                assert_eq!(a_expr.len(), 1);
                assert_eq!(b_expr.len(), 1);
                let op = match expr.op {
                    parser::BinaryOperator::And => BinaryGate::And,
                    parser::BinaryOperator::Or => BinaryGate::Or,
                    parser::BinaryOperator::Xor => BinaryGate::Xor,
                };
                let connection = self.connect_with(
                    op,
                    a_expr[0],
                    UnaryGate::Identity,
                    b_expr[0],
                    UnaryGate::Identity,
                    false,
                )?;
                Some(vec![connection])
            }
            parser::Expr::CircuitCall(call) => {
                // merge
                let other = if let Some(other) = parsed_circs.get(&call.name) {
                    other
                } else {
                    return Ok(None);
                };

                let other_graph = &other.graph;
                let mut other_nodes_to_my_nodes = FxHashMap::default();

                for node_id in other_graph.node_indices() {
                    if let Some(Binding::Named(_)) = other.node_bindings.get(&node_id) {
                        let my_node_id = self.add_node(
                            other_graph[node_id].op,
                            other.node_bindings.get(&node_id).cloned(),
                        );
                        other_nodes_to_my_nodes.insert(node_id, my_node_id);
                    } else if let Some(bind) = other.node_bindings.get(&node_id) {
                        other_nodes_to_my_nodes
                            .insert(node_id, self.node_by_binding(bind).unwrap());
                    } else {
                        let my_node_id = self.add_node(other_graph[node_id].op, None);
                        other_nodes_to_my_nodes.insert(node_id, my_node_id);
                    }
                }

                for (input_idx, input_expr) in call.inputs.iter().enumerate() {
                    let parsed_expr = self.parse_expr(input_expr, parsed_circs)?;
                    let parsed_expr = if let Some(parsed_expr) = parsed_expr {
                        parsed_expr
                    } else {
                        return Ok(None);
                    };
                    assert_eq!(parsed_expr.len(), 1);
                    other_nodes_to_my_nodes.insert(other.input_nodes[input_idx], parsed_expr[0]);
                }
                for edge_id in other_graph.edge_indices() {
                    let edge = other_graph[edge_id];
                    let (src, target) = other_graph.edge_endpoints(edge_id).unwrap();
                    let (my_src, my_target) = (
                        other_nodes_to_my_nodes[&src],
                        other_nodes_to_my_nodes[&target],
                    );
                    self.connect(my_src, my_target, edge);
                }

                Some(
                    other
                        .output_nodes
                        .iter()
                        .filter(|node| matches!(other.node_bindings[node], Binding::Named(_)))
                        .map(|node| other_nodes_to_my_nodes[node])
                        .collect(),
                )
            }
        };
        Ok(idxs)
    }

    pub fn parse(script: &'a str) -> Result<Circuit> {
        let script = script.lines().collect::<Vec<_>>().join(" ");
        let (_junk, circs) = parser::parse_circuits(&script)
            .map_err(|e| Error::msg(format!("Parsing error: {}", e)))?;
        let mut parsed_circs = FxHashMap::default();
        let mut ready_circs = FxHashSet::default();

        for circ in circs.iter() {
            let parsed = parsed_circs
                .entry(circ.name.to_owned())
                .or_insert(Self::new(&circ.name));
            let mut bindings_to_nodes = FxHashMap::default();

            let clk = parsed.add_node(
                BinaryGate::IgnoreInput(ABitBehavior::Clock {
                    half_period: CLOCK_FULL_INTERVAL / 2,
                }),
                Some(Binding::Clk),
            );
            parsed.output_nodes.push(clk);
            bindings_to_nodes.insert(Binding::Clk, clk);
            parsed.node_bindings.insert(clk, Binding::Clk);

            let always_hi = parsed.add_node(
                BinaryGate::IgnoreInput(ABitBehavior::AlwaysHi),
                Some(Binding::Hi),
            );
            bindings_to_nodes.insert(Binding::Hi, always_hi);
            parsed.node_bindings.insert(always_hi, Binding::Hi);

            let always_lo = parsed.add_node(
                BinaryGate::IgnoreInput(ABitBehavior::AlwaysLo),
                Some(Binding::Lo),
            );
            bindings_to_nodes.insert(Binding::Lo, always_lo);
            parsed.node_bindings.insert(always_lo, Binding::Lo);

            for idx in 0..NUM_DISPLAYS {
                for shift in 0..8 {
                    let binding = Binding::Num8Bit {
                        display_idx: idx,
                        bit_shift: shift,
                    };
                    let node = parsed.add_node(BinaryGate::AlwaysA, Some(binding.clone()));
                    bindings_to_nodes.insert(binding.clone(), node);
                    parsed.node_bindings.insert(node, binding);
                    parsed.output_nodes.push(node);
                }
            }

            for input in circ.inputs.iter() {
                let id = parsed.add_default_node(Some(input.to_owned()));
                parsed.input_nodes.push(id);
                bindings_to_nodes.insert(input.to_owned(), id);
                parsed.node_bindings.insert(id, input.to_owned());
            }
            for output in circ.outputs.iter() {
                let id = parsed.add_default_node(Some(output.to_owned()));
                parsed.output_nodes.push(id);
                bindings_to_nodes.insert(output.to_owned(), id);
                parsed.node_bindings.insert(id, output.to_owned());
            }
            for connection in circ.logic.iter() {
                let refs = connection.all_refs();
                for refr in
                    refs.difference(&FxHashSet::from_iter(bindings_to_nodes.keys().cloned()))
                {
                    let id = parsed.add_default_node(Some(refr.to_owned()));
                    bindings_to_nodes.insert(refr.to_owned(), id);
                    parsed.node_bindings.insert(id, refr.to_owned());
                }
            }

            parsed
                .node_bindings
                .extend(bindings_to_nodes.iter().map(|(k, v)| (*v, k.to_owned())));
        }

        while ready_circs.len() < circs.len() {
            for circ in circs.iter() {
                let mut all_parsed = true;
                let mut parsed = parsed_circs.remove(&circ.name).unwrap();
                for connection in circ.logic.iter() {
                    let outputs = parsed.parse_expr(&connection.expr, &parsed_circs)?;

                    if let Some(outputs) = outputs {
                        assert_eq!(connection.targets.len(), outputs.len());
                        for (target, output) in connection.targets.iter().zip(outputs.iter()) {
                            let target = parsed.node_by_binding(target).unwrap();
                            parsed.connect(*output, target, UnaryGate::Identity);
                        }
                    } else {
                        all_parsed = false;
                    }
                }

                if all_parsed {
                    ready_circs.insert(parsed.name.to_owned());
                }
                parsed_circs.insert(circ.name.to_owned(), parsed);
            }
        }

        let mut main = parsed_circs.remove("main").unwrap();
        main.graph.retain_nodes(|graph, node| {
            (graph.edges_directed(node, Direction::Incoming).count() > 0
                || graph.edges_directed(node, Direction::Outgoing).count() > 0)
                || main.node_bindings.get(&node).is_some()
        });
        Ok(main)
    }

    pub fn write_dot(&self, filename: PathBuf) -> Result<()> {
        use std::io::Write;
        write!(
            &mut File::create(filename.with_extension("dot"))?,
            "{:?}",
            Dot::with_config(
                &self.graph,
                &[] // &[Config::EdgeIndexLabel, Config::NodeIndexLabel]
            )
        )?;
        // let svg = Command::new("dot")
        //     .args([
        //         "-Tsvg",
        //         filename.with_extension("dot").as_os_str().to_str().unwrap(),
        //     ])
        //     .output()?;
        // File::create(filename.with_extension("svg"))?.write_all(&svg.stdout)?;
        Ok(())
    }
}
