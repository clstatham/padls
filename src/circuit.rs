use std::{collections::hash_map::Entry, fs::File, path::PathBuf};

use anyhow::{Error, Result};
use petgraph::{
    dot::Dot,
    prelude::*,
    visit::{Control, DfsEvent},
};
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::watch::Receiver;

use crate::{
    bit::{ABitBehavior, Bit, SpawnResult},
    ops::{BinaryGate, OwnedBinaryGate, OwnedUnaryGate, UnaryGate},
    parser::{self, Binding},
    CLOCK_FULL_INTERVAL,
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

    pub fn spawn_eager(
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
                // for node_id in other_graph.node_indices() {
                //     let node = &other_graph[node_id];
                //     self.graph[node_id] = node.clone();
                // }
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
            // parsed.output_nodes.push(always_hi);
            bindings_to_nodes.insert(Binding::Hi, always_hi);
            parsed.node_bindings.insert(always_hi, Binding::Hi);
            let always_lo = parsed.add_node(
                BinaryGate::IgnoreInput(ABitBehavior::AlwaysLo),
                Some(Binding::Lo),
            );
            // parsed.output_nodes.push(always_lo);
            bindings_to_nodes.insert(Binding::Lo, always_lo);
            parsed.node_bindings.insert(always_lo, Binding::Lo);

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

        // for (name, circ) in parsed_circs.iter() {
        //     circ.write_dot(name.into()).unwrap();
        // }
        Ok(parsed_circs.remove("main").unwrap())
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
