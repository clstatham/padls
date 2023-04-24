use std::{collections::hash_map::Entry, fs::File, path::PathBuf, process::Command};

use anyhow::{Error, Result};
use petgraph::{
    dot::{Config, Dot},
    prelude::*,
    visit::{Control, DfsEvent},
};
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::broadcast::Receiver;

use crate::{
    bit::Bit,
    ops::{BinaryOp, OwnedBinaryOp, OwnedUnaryOp, UnaryOp},
    parser::{self, Binding},
};

#[derive(Clone)]
pub struct BinaryOpNode {
    op: BinaryOp,
    name: Option<Binding>,
}

impl std::fmt::Debug for BinaryOpNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
    graph: DiGraph<BinaryOpNode, UnaryOp>,
    pub(crate) owned_unaries: FxHashMap<EdgeIndex, OwnedUnaryOp>,
    pub(crate) owned_binaries: FxHashMap<NodeIndex, OwnedBinaryOp>,
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

    pub fn input_nodes(&self) -> &Vec<NodeIndex> {
        &self.input_nodes
    }

    pub fn output_nodes(&self) -> &Vec<NodeIndex> {
        &self.output_nodes
    }

    pub fn add_default_node(&mut self, name: Option<Binding>) -> NodeIndex {
        self.graph.add_node(BinaryOpNode {
            op: BinaryOp::A,
            name,
        })
    }

    pub fn add_node(&mut self, op: BinaryOp, name: Option<Binding>) -> NodeIndex {
        self.graph.add_node(BinaryOpNode { op, name })
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

    pub fn set_node_op(&mut self, idx: NodeIndex, op: BinaryOp) {
        self.graph[idx].op = op;
    }

    pub fn node_by_binding(&self, name: &Binding) -> Option<NodeIndex> {
        self.node_bindings
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
        let idx = { self.graph.add_node(BinaryOpNode { op, name: None }) };

        let _edge_a = { self.connect(input_a, idx, input_op_a) };

        let _edge_b = { self.connect(input_b, idx, input_op_b) };

        if is_output {
            self.output_nodes.push(idx);
        }
        Ok(idx)
    }

    pub fn connect(&mut self, source: NodeIndex, sink: NodeIndex, op: UnaryOp) -> EdgeIndex {
        self.graph.add_edge(source, sink, op)
    }

    fn flesh_out_graph(&mut self) {
        let inputs = self.input_nodes().clone();
        #[allow(clippy::iter_nth_zero)]
        #[allow(clippy::map_entry)] // stfu clippy, that's way less readable
        petgraph::visit::depth_first_search(&self.graph, inputs, |event| {
            match event {
                DfsEvent::Discover(node_id, _t) => {
                    let op = &self.graph[node_id];
                    if !self.owned_binaries.contains_key(&node_id) {
                        self.owned_binaries
                            .insert(node_id, OwnedBinaryOp::new(node_id, op.op));
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
                    let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
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
                        let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
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
                        let mut edge = OwnedUnaryOp::new(edge_id, edge_op);
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
                            let a_out = a_op.get_output();
                            binary.set_input_a(Some(a.id()), a_out);
                        }
                        if let Some(b) = edges.next() {
                            if let Some(b_op) = self.owned_unaries.get(&b.id()) {
                                let b_out = b_op.get_output();
                                binary.set_input_b(Some(b.id()), b_out);
                            }
                        } else if let Some(a_op) = self.owned_unaries.get(&a.id()) {
                            let a_out = a_op.get_output();
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
        inputs: FxHashMap<NodeIndex, (Receiver<Bit>, Receiver<Bit>)>,
    ) -> Option<FxHashMap<NodeIndex, Receiver<Bit>>> // output rx
    {
        assert_eq!(inputs.len(), self.input_nodes.len());
        assert!(self.input_nodes.iter().all(|val| inputs.contains_key(val)));
        self.flesh_out_graph();
        for (inp_idx, (inp_a, inp_b)) in inputs.into_iter() {
            let binary = self.owned_binaries.get_mut(&inp_idx).unwrap();
            binary.set_input_a(None, inp_a);
            binary.set_input_b(None, inp_b);
        }
        for _ in 0..10 {
            self.flesh_out_graph();
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
            let rx = binary.subscribe();
            if binary.try_spawn().is_err() {
                println!("{:?} failed to spawn", binary.idx);
                all_spawned = false;
            } else if output_nodes.contains(b_idx) {
                if let Entry::Vacant(e) = outputs.entry(*b_idx) {
                    e.insert(rx);
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
                        println!("    Op: {:?}", node.op);
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
                            node.handle.as_ref().and_then(|handle| handle.id())
                        )
                    }
                    DfsEvent::TreeEdge(src, target) | DfsEvent::CrossForwardEdge(src, target) => {
                        let edges = self.graph.edges_connecting(src, target);
                        for edge in edges {
                            let edge = &self.owned_unaries[&edge.id()];
                            println!(
                                "Edge ID {:?}:\n    Op: {:?}\n    {:?} ({:?}) -> {:?}\n    PBit: {:?}",
                                edge.idx,
                                edge.op,
                                edge.inp_idx,
                                edge.inp_idx.map(bind),
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
                    op: BinaryOp::A,
                    name: None,
                });
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
                    let my_node_id = self.add_default_node(
                        other.node_bindings.get(&node_id).cloned(), // .map(|b| format!("{}", b).as_str()),
                    );
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
                    let node = &other_graph[node_id];
                    self.graph[node_id] = node.clone();
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
            // let clk = parsed.add_default_node();
            // parsed.input_nodes.push(clk);
            // bindings_to_nodes.insert(Binding::Clk, clk);
            // parsed.node_bindings.insert(clk, Binding::Clk);
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
            for assignment in circ.logic.iter() {
                let refs = assignment.all_refs();
                for refr in
                    refs.difference(&FxHashSet::from_iter(bindings_to_nodes.keys().cloned()))
                {
                    let id = parsed.add_default_node(Some(refr.to_owned()));
                    // let id = parsed.graph.lock().add_node(BinaryOp::B);
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
                for assignment in circ.logic.iter() {
                    let outputs = parsed.parse_expr(&assignment.expr, &parsed_circs)?;

                    if let Some(outputs) = outputs {
                        assert_eq!(assignment.targets.len(), outputs.len());
                        for (target, output) in assignment.targets.iter().zip(outputs.iter()) {
                            let target = parsed.node_by_binding(target).unwrap();
                            parsed.connect(*output, target, UnaryOp::Identity);
                        }
                    } else {
                        all_parsed = false;
                    }
                }

                if all_parsed {
                    // parsed.graph.retain_nodes(|graph, node| {
                    //     graph.edges(node).count() > 0
                    //         && !graph.edges(node).all(|edge| edge.source() == edge.target())
                    // });
                    // parsed.graph.retain_edges(|graph, edge| {
                    //     let (a, b) = graph.edge_endpoints(edge).unzip();
                    //     if let (Some(a), Some(b)) = (a, b) {
                    //         graph.node_indices().any(|i| i == a)
                    //             && graph.node_indices().any(|i| i == b)
                    //     } else {
                    //         false
                    //     }
                    // });

                    // let all_nodes = parsed.graph.node_indices().collect::<Vec<_>>();
                    // let all_edges = parsed.graph.edge_indices().collect::<Vec<_>>();
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
                    // parsed
                    //     .owned_binaries
                    //     .retain(|idx, _| all_nodes.contains(idx));
                    // parsed
                    //     .owned_unaries
                    //     .retain(|idx, _| all_edges.contains(idx));
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
                &[] // &[Config::EdgeIndexLabel, Config::NodeIndexLabel]
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
