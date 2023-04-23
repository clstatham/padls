use std::{collections::hash_map::Entry, fs::File, path::PathBuf, process::Command};

use anyhow::{Error, Result};
use async_channel::Receiver;
use petgraph::{
    dot::{Config, Dot},
    prelude::*,
    visit::{Control, DfsEvent},
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    bit::Bit,
    ops::{BinaryOp, OwnedBinaryOp, OwnedUnaryOp, UnaryOp},
    parser,
};

pub struct Circuit {
    pub name: String,
    graph: DiGraph<BinaryOp, UnaryOp>,
    pub(crate) owned_unaries: FxHashMap<EdgeIndex, OwnedUnaryOp>,
    pub(crate) owned_binaries: FxHashMap<NodeIndex, OwnedBinaryOp>,
    pub node_names: FxHashMap<NodeIndex, String>,
    input_nodes: Vec<NodeIndex>,
    output_nodes: Vec<NodeIndex>,
}

impl Circuit {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            graph: DiGraph::new(),
            owned_unaries: FxHashMap::default(),
            owned_binaries: FxHashMap::default(),
            node_names: FxHashMap::default(),
            input_nodes: Vec::default(),
            output_nodes: Vec::default(),
        }
    }

    pub fn input_nodes(&self) -> &Vec<NodeIndex> {
        &self.input_nodes
    }

    pub fn output_nodes(&self) -> &Vec<NodeIndex> {
        &self.output_nodes
    }

    pub fn add_default_node(&mut self) -> NodeIndex {
        self.graph.add_node(BinaryOp::A)
    }

    pub fn add_node(&mut self, op: BinaryOp) -> NodeIndex {
        self.graph.add_node(op)
    }

    pub fn add_input(&mut self) -> NodeIndex {
        let idx = self.add_default_node();
        self.input_nodes.push(idx);
        idx
    }

    pub fn add_output(&mut self) -> NodeIndex {
        let idx = self.add_default_node();
        self.output_nodes.push(idx);
        idx
    }

    pub fn set_node_op(&mut self, idx: NodeIndex, op: BinaryOp) {
        self.graph[idx] = op;
    }

    pub fn node_by_name(&self, name: &str) -> Option<NodeIndex> {
        self.node_names
            .iter()
            .find_map(|(k, v)| if v == name { Some(k.to_owned()) } else { None })
    }

    pub fn connect_with(
        &mut self,
        op: BinaryOp,
        input_a: NodeIndex,
        input_op_a: UnaryOp,
        input_b: NodeIndex,
        input_op_b: UnaryOp,
        is_output: bool,
    ) -> Result<NodeIndex> {
        let idx = { self.graph.add_node(op) };

        let edge_a = { self.connect(input_a, idx, input_op_a) };

        let edge_b = { self.connect(input_b, idx, input_op_b) };

        // let mut binary = OwnedBinaryOp::new(idx, op);
        // binary.set_input_a(Some(edge_a), self.owned_unaries[&edge_a].get_output());
        // binary.set_input_b(Some(edge_b), self.owned_unaries[&edge_b].get_output());
        // self.owned_binaries.insert(idx, binary);
        if is_output {
            self.output_nodes.push(idx);
        }
        Ok(idx)
    }

    pub fn connect(&mut self, source: NodeIndex, sink: NodeIndex, op: UnaryOp) -> EdgeIndex {
        let edge_idx = self.graph.add_edge(source, sink, op);
        // let mut unary = OwnedUnaryOp::new(edge_idx, op);
        // unary.set_input(source, self.owned_binaries[&source].get_output());
        // self.owned_unaries.insert(edge_idx, unary);
        edge_idx
    }

    fn flesh_out_graph(&mut self) {
        let inputs = self.input_nodes().clone();
        #[allow(clippy::iter_nth_zero)]
        petgraph::visit::depth_first_search(&self.graph, inputs, |event| {
            match event {
                DfsEvent::Discover(node_id, _t) => {
                    let op = self.graph[node_id];
                    self.owned_binaries
                        .insert(node_id, OwnedBinaryOp::new(node_id, op));
                }
                DfsEvent::TreeEdge(source, target) => {
                    let mut edges = self.graph.edges_connecting(source, target);
                    let mut edge_id = edges.next().unwrap().id();
                    if self.owned_unaries.contains_key(&edge_id) {
                        edge_id = edges.next().unwrap().id();
                    }
                    println!("TreeEdge {:?} => {:?} => {:?}", source, edge_id, target);
                    let edge_op = self.graph[edge_id];
                    let src_op = self.owned_binaries.get(&source).unwrap();
                    let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
                    edge.set_input(source, src_op.get_output());
                    self.owned_unaries.insert(edge_id, edge);
                }
                DfsEvent::BackEdge(source, target) => {
                    assert_eq!(self.graph.edges_connecting(source, target).count(), 1);
                    let mut edges = self.graph.edges_connecting(source, target);
                    let edge_id = edges.next().unwrap().id();
                    #[allow(clippy::map_entry)] // stfu clippy, that's way less readable
                    if !self.owned_unaries.contains_key(&edge_id) {
                        let edge_op = self.graph[edge_id];
                        println!("BackEdge {:?} => {:?} => {:?}", source, edge_id, target);
                        let src_op = self.owned_binaries.get(&source).unwrap();
                        let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
                        edge.set_input(source, src_op.get_output());
                        self.owned_unaries.insert(edge_id, edge);
                    }
                }

                DfsEvent::CrossForwardEdge(source, target) => {
                    // connect B
                    let mut edges = self.graph.edges_connecting(source, target);
                    let edge = edges.next().unwrap();
                    let edge_id = edge.id();
                    let edge_op = self.graph[edge_id];
                    println!("XFEdge {:?} => {:?} => {:?}", source, edge_id, target);
                    let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
                    let src_op = self.owned_binaries.get(&source).unwrap();
                    edge.set_input(source, src_op.get_output());
                    self.owned_unaries.insert(edge_id, edge);
                }
                DfsEvent::Finish(node_id, _t) => {
                    let binary = self.owned_binaries.get_mut(&node_id).unwrap();
                    let mut edges = self.graph.edges_directed(node_id, Direction::Incoming);
                    if let Some(a) = edges.next() {
                        let a_out = self.owned_unaries[&a.id()].get_output();
                        binary.set_input_a(Some(a.id()), a_out.clone());
                        if let Some(b) = edges.next() {
                            if let Some(b_op) = self.owned_unaries.get(&b.id()) {
                                let b_out = b_op.get_output();
                                binary.set_input_b(Some(b.id()), b_out);
                            }
                        } else {
                            binary.set_input_b(Some(a.id()), a_out);
                        }
                    }
                }
            }
            Control::Continue::<()>
        });
    }

    pub fn spawn_eval(
        &mut self,
        inputs: FxHashMap<NodeIndex, Receiver<Bit>>,
    ) -> Option<FxHashMap<NodeIndex, Receiver<Bit>>> // output rx
    {
        assert_eq!(inputs.len(), self.input_nodes.len());
        assert!(self.input_nodes.iter().all(|val| inputs.contains_key(val)));
        self.flesh_out_graph();
        self.dump_ops();
        for (inp_idx, inp) in inputs.iter() {
            let binary = self.owned_binaries.get_mut(inp_idx).unwrap();
            binary.set_input_a(None, inp.clone());
            binary.set_input_b(None, inp.clone());
        }

        let mut outputs = FxHashMap::default();

        let mut all_spawned = true;
        let output_nodes = self.output_nodes().clone();
        for (_u_idx, unary) in self.owned_unaries.iter_mut() {
            if unary.try_spawn().is_err() {
                println!("{:?} failed to spawn", unary.idx);
                all_spawned = false;
            }
        }
        for (b_idx, binary) in self.owned_binaries.iter_mut() {
            if binary.try_spawn().is_err() {
                println!("{:?} failed to spawn", binary.idx);
                all_spawned = false;
            } else if output_nodes.contains(b_idx) {
                if let Entry::Vacant(e) = outputs.entry(*b_idx) {
                    e.insert(binary.get_output());
                }
            }
        }
        if !all_spawned {
            self.dump_ops();
            return None;
        }
        Some(outputs)
    }

    pub fn dump_ops(&self) {
        petgraph::visit::depth_first_search(
            &self.graph,
            self.input_nodes().iter().copied(),
            |event| {
                #[allow(clippy::single_match)]
                match event {
                    DfsEvent::Discover(node, _) => {
                        let node = &self.owned_binaries[&node];
                        println!("Node ID {}:", node.idx.index());
                        println!("    Op: {:?}", node.op);
                        println!("    Input A: {:?}", node.inp_a_id.map(|idx| idx.index()));
                        println!("    Input B: {:?}", node.inp_b_id.map(|idx| idx.index()));
                        println!(
                            "    PBit: {:?}",
                            node.handle.as_ref().and_then(|handle| handle.id())
                        )
                    }
                    DfsEvent::TreeEdge(src, target) | DfsEvent::CrossForwardEdge(src, target) => {
                        let edges = self.graph.edges_connecting(src, target);
                        for edge in edges {
                            let edge = &self.owned_unaries[&edge.id()];
                            println!(
                                "Edge ID {}:\n    Op: {:?}\n    {:?} -> {:?}\n    PBit: {:?}",
                                edge.idx.index(),
                                edge.op,
                                edge.inp_idx,
                                target,
                                edge.handle.as_ref().and_then(|handle| handle.id())
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
        parsed_circs: &FxHashMap<String, Circuit>,
    ) -> Result<Option<Vec<NodeIndex>>> {
        let idxs = match expr {
            parser::Expr::Binding(bind) => Some(vec![self.node_by_name(&bind.0).unwrap()]),
            parser::Expr::Not(expr) => {
                let not_expr = self.parse_expr(&expr.expr, parsed_circs)?;
                let not_expr = if let Some(not_expr) = not_expr {
                    not_expr
                } else {
                    return Ok(None);
                };
                assert_eq!(not_expr.len(), 1);
                let connection = self.graph.add_node(BinaryOp::A);
                self.connect(not_expr[0], connection, UnaryOp::Not);
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
                    parser::BinaryOperator::And => BinaryOp::And,
                    parser::BinaryOperator::Or => BinaryOp::Or,
                    parser::BinaryOperator::Xor => BinaryOp::Xor,
                };
                let connection = self.connect_with(
                    op,
                    a_expr[0],
                    UnaryOp::Identity,
                    b_expr[0],
                    UnaryOp::Identity,
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
                    // let node = other_graph[node_id];
                    // let tmp = AsyncBit::spawn(None);
                    let my_node_id = self.add_default_node();
                    other_nodes_to_my_nodes.insert(node_id, my_node_id);
                }

                for (input_idx, input_expr) in call.inputs.iter().enumerate() {
                    let parsed_expr: Option<Vec<NodeIndex>> =
                        self.parse_expr(input_expr, parsed_circs)?;
                    let parsed_expr = if let Some(parsed_expr) = parsed_expr {
                        parsed_expr
                    } else {
                        return Ok(None);
                    };
                    assert_eq!(parsed_expr.len(), 1);
                    other_nodes_to_my_nodes.insert(other.input_nodes[input_idx], parsed_expr[0]);
                }
                for node_id in other_graph.node_indices() {
                    let node = other_graph[node_id];
                    self.graph[node_id] = node;
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
                        .map(|node| other_nodes_to_my_nodes[node])
                        .collect(),
                )
            }
        };
        Ok(idxs)
    }

    pub fn parse(script: &str) -> Result<Self> {
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
            for input in circ.inputs.iter() {
                let id = parsed.add_default_node();
                parsed.input_nodes.push(id);
                bindings_to_nodes.insert(input.to_owned(), id);
                parsed.node_names.insert(id, input.0.to_owned());
            }
            for output in circ.outputs.iter() {
                let id = parsed.add_default_node();
                parsed.output_nodes.push(id);
                bindings_to_nodes.insert(output.to_owned(), id);
                parsed.node_names.insert(id, output.0.to_owned());
            }
            for assignment in circ.logic.iter() {
                let refs = assignment.all_refs();
                for refr in
                    refs.difference(&FxHashSet::from_iter(bindings_to_nodes.keys().cloned()))
                {
                    let id = parsed.add_default_node();
                    // let id = parsed.graph.lock().add_node(BinaryOp::B);
                    bindings_to_nodes.insert(refr.to_owned(), id);
                    parsed.node_names.insert(id, refr.0.to_owned());
                }
            }

            parsed
                .node_names
                .extend(bindings_to_nodes.iter().map(|(k, v)| (*v, k.0.to_owned())));
        }

        while ready_circs.len() < circs.len() {
            for circ in circs.iter() {
                let mut all_parsed = true;
                let mut parsed = parsed_circs.remove(&circ.name).unwrap();
                for assignment in circ.logic.iter() {
                    let outputs = parsed.parse_expr(&assignment.expr, &parsed_circs)?;

                    if let Some(outputs) = outputs {
                        assert_eq!(assignment.targets.len(), outputs.len());
                        for (target, output) in assignment.targets.iter().zip(outputs.iter()) {
                            let target = parsed.node_by_name(&target.0).unwrap();
                            parsed.connect(*output, target, UnaryOp::Identity);
                        }
                    } else {
                        all_parsed = false;
                    }
                }

                if all_parsed {
                    parsed.graph.retain_nodes(|graph, node| {
                        graph.edges(node).count() > 0
                            && !graph.edges(node).all(|edge| edge.source() == edge.target())
                    });
                    // parsed.graph.retain_edges(|graph, edge| {
                    //     let (a, b) = graph.edge_endpoints(edge).unzip();
                    //     if let (Some(a), Some(b)) = (a, b) {
                    //         graph.node_indices().any(|i| i == a)
                    //             && graph.node_indices().any(|i| i == b)
                    //     } else {
                    //         false
                    //     }
                    // });

                    let all_nodes = parsed.graph.node_indices().collect::<Vec<_>>();
                    let all_edges = parsed.graph.edge_indices().collect::<Vec<_>>();
                    // parsed.input_nodes.retain(|node| {
                    //     parsed
                    //         .graph
                    //         .edges_directed(*node, Direction::Outgoing)
                    //         .count()
                    //         > 0
                    // });
                    // parsed.output_nodes.retain(|node| {
                    //     parsed
                    //         .graph
                    //         .edges_directed(*node, Direction::Incoming)
                    //         .count()
                    //         > 0
                    // });
                    parsed
                        .owned_binaries
                        .retain(|idx, _| all_nodes.contains(idx));
                    parsed
                        .owned_unaries
                        .retain(|idx, _| all_edges.contains(idx));
                    ready_circs.insert(parsed.name.to_owned());
                }
                parsed_circs.insert(circ.name.to_owned(), parsed);
            }
        }

        for (name, circ) in parsed_circs.iter() {
            circ.write_svg(name.into()).unwrap();
        }
        Ok(parsed_circs.remove("main").unwrap())
    }

    pub fn write_svg(&self, filename: PathBuf) -> Result<()> {
        use std::io::Write;
        write!(
            &mut File::create(filename.with_extension("dot"))?,
            "{:?}",
            Dot::with_config(
                &self.graph,
                // &[],
                &[Config::EdgeIndexLabel, Config::NodeIndexLabel]
            )
        )?;
        let svg = Command::new("dot")
            .args([
                "-Tsvg",
                filename.with_extension("dot").as_os_str().to_str().unwrap(),
            ])
            .output()?;
        File::create(filename.with_extension("svg"))?.write_all(&svg.stdout)?;
        Ok(())
    }
}
