use std::{fs::File, path::PathBuf, process::Command, sync::Arc};

use anyhow::{Error, Result};
use petgraph::{dot::Dot, prelude::*};
use rustc_hash::FxHashMap;
use tokio::sync::{watch, Mutex};

use crate::{
    bit::{AsyncBit, Bit},
    parser,
};

// pub struct UnaryOp(pub Arc<dyn Fn(Bit) -> Bit>);
// impl Deref for UnaryOp {
//     type Target = dyn Fn(Bit) -> Bit;
//     fn deref(&self) -> &Self::Target {
//         self.0.as_ref()
//     }
// }
#[derive(Clone, Debug, Copy)]
pub enum UnaryOp {
    Identity,
    Not,
}

impl UnaryOp {
    pub fn eval(self, x: Bit) -> Bit {
        match self {
            Self::Identity => x,
            Self::Not => !x,
        }
    }

    pub fn spawn(self, mut x: watch::Receiver<Bit>) -> watch::Receiver<Bit> {
        let listener = AsyncBit::spawn(self.eval(*x.borrow_and_update()));
        let rx = listener.get_rx();
        tokio::spawn(async move {
            loop {
                x.changed().await.unwrap();
                // println!("X Changed");
                match self {
                    Self::Identity => listener.send(*x.borrow_and_update()),
                    Self::Not => listener.send(!*x.borrow_and_update()),
                }
                tokio::task::yield_now().await;
            }
        });
        rx
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BinaryOp {
    And,
    Or,
    Xor,
    A,
    B,
}

impl BinaryOp {
    pub fn eval(self, a: Bit, b: Bit) -> Bit {
        match self {
            Self::A => a,
            Self::B => b,
            Self::And => a & b,
            Self::Or => a | b,
            Self::Xor => a ^ b,
        }
    }

    pub fn spawn(
        self,
        mut a: watch::Receiver<Bit>,
        mut b: watch::Receiver<Bit>,
    ) -> watch::Receiver<Bit> {
        let listener = AsyncBit::spawn(self.eval(*a.borrow_and_update(), *b.borrow_and_update()));
        let rx = listener.get_rx();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = a.changed() => {},
                    _ = b.changed() => {},
                };
                match self {
                    Self::A => listener.send(*a.borrow_and_update()),
                    Self::B => listener.send(*b.borrow_and_update()),
                    Self::And => listener.send(*a.borrow_and_update() & *b.borrow_and_update()),
                    Self::Or => listener.send(*a.borrow_and_update() | *b.borrow_and_update()),
                    Self::Xor => listener.send(*a.borrow_and_update() ^ *b.borrow_and_update()),
                }
                tokio::task::yield_now().await;
            }
        });
        rx
    }
}

#[derive(Clone, Copy)]
pub struct BinaryInput(Bit, Bit);

#[derive(Clone)]
pub struct Circuit {
    pub name: String,
    graph: Arc<Mutex<DiGraph<BinaryOp, UnaryOp>>>,
    patterns: FxHashMap<String, Self>,
    input_nodes: FxHashMap<String, NodeIndex>,
    output_nodes: FxHashMap<String, NodeIndex>,
    next_anon_bind: usize,
}

impl Circuit {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            graph: Arc::new(Mutex::new(DiGraph::new())),
            patterns: FxHashMap::default(),
            input_nodes: FxHashMap::default(),
            output_nodes: FxHashMap::default(),
            next_anon_bind: 0,
        }
    }

    pub fn input_nodes(&self) -> &FxHashMap<String, NodeIndex> {
        &self.input_nodes
    }

    pub fn output_nodes(&self) -> &FxHashMap<String, NodeIndex> {
        &self.output_nodes
    }

    pub async fn add_input(&mut self) -> NodeIndex {
        self.graph.lock().await.add_node(BinaryOp::Or)
    }

    pub async fn add_many_inputs(&mut self, n: u32) -> Vec<NodeIndex> {
        let mut out = vec![];
        for _ in 0..n {
            out.push(self.add_input().await);
        }
        out
    }

    pub async fn connect(
        &mut self,
        op: BinaryOp,
        input_a: NodeIndex,
        input_op_a: UnaryOp,
        input_b: NodeIndex,
        input_op_b: UnaryOp,
    ) -> Result<NodeIndex> {
        let idx = { self.graph.lock().await.add_node(op) };
        {
            self.graph
                .lock()
                .await
                .update_edge(input_a, idx, input_op_a);
        }

        {
            self.graph
                .lock()
                .await
                .update_edge(input_b, idx, input_op_b);
        }

        Ok(idx)
    }

    // #[tracing::instrument(skip(self))]
    pub async fn spawn_eval(
        &self,
        inputs: FxHashMap<NodeIndex, watch::Receiver<Bit>>,
    ) -> FxHashMap<NodeIndex, watch::Receiver<Bit>> // output rx
    {
        #[derive(Debug)]
        struct NodeState {
            out: watch::Receiver<Bit>,
        }
        #[derive(Debug)]
        struct EdgeState {
            out: watch::Receiver<Bit>,
        }
        let output_nodes = self.output_nodes().values().copied().collect::<Vec<_>>();
        let n_nodes = { self.graph.lock().await.node_count() };
        let n_edges = { self.graph.lock().await.edge_count() };

        let mut out_rxs = FxHashMap::default();

        let mut nodes_ctx = FxHashMap::default();
        let mut edges_ctx = FxHashMap::default();

        for (target_id, input) in inputs {
            let out = BinaryOp::A.spawn(input.clone(), input.clone());
            nodes_ctx.insert(target_id, Arc::new(NodeState { out }));
        }

        loop {
            {
                let graph = self.graph.lock().await;
                for edge in graph.edge_indices() {
                    let weight = graph[edge];
                    let (source_idx, _target_idx) = graph.edge_endpoints(edge).unwrap();
                    if let Some(source) = nodes_ctx.get(&source_idx) {
                        let input = source.out.clone();
                        edges_ctx.insert(
                            edge,
                            EdgeState {
                                out: weight.spawn(input.clone()),
                            },
                        );
                    }
                }
            }
            {
                let graph = self.graph.lock().await;
                for node_idx in graph.node_indices() {
                    let node = graph[node_idx];
                    let in_idxs = graph
                        .edges_directed(node_idx, Direction::Incoming)
                        .map(|edge| edge.id())
                        .collect::<Vec<_>>();
                    if !in_idxs.is_empty() {
                        if let Some(in_a) = edges_ctx.get(&in_idxs[0]) {
                            if let Some(in_b) = edges_ctx.get(&in_idxs[1]) {
                                let in_a = in_a.out.clone();
                                let in_b = in_b.out.clone();
                                let state = Arc::new(NodeState {
                                    out: node.spawn(in_a.clone(), in_b.clone()),
                                });
                                nodes_ctx.insert(node_idx, state.clone());
                                if output_nodes.contains(&node_idx) {
                                    out_rxs.insert(node_idx, state.out.clone());
                                }
                            }
                        }
                    }
                }
            }

            if nodes_ctx.len() == n_nodes && edges_ctx.len() == n_edges {
                break;
            }
        }

        out_rxs
    }

    pub fn eval(&mut self, inputs: &[(NodeIndex, Bit)]) -> Result<FxHashMap<NodeIndex, Bit>> {
        let output_nodes = self.output_nodes().values().copied().collect::<Vec<_>>();
        let mut outputs = FxHashMap::default();
        let mut inputs = inputs
            .iter()
            .map(|(idx, bit)| (*idx, vec![*bit]))
            .collect::<FxHashMap<NodeIndex, Vec<Bit>>>();
        let graph = self.graph.blocking_lock();
        while outputs.len() < output_nodes.len() {
            let mut next_inputs = FxHashMap::default();
            for (target_idx, target_inputs) in inputs {
                let result = if target_inputs.len() == 1 {
                    // todo: what should we do here?
                    let op = graph[target_idx];
                    op.eval(target_inputs[0], target_inputs[0])
                } else if target_inputs.len() == 2 {
                    let op = graph[target_idx];
                    op.eval(target_inputs[0], target_inputs[1])
                } else {
                    return Err(Error::msg("Expected exactly one or two inputs to op"));
                };
                if output_nodes.contains(&target_idx) {
                    outputs.insert(target_idx, result);
                } else {
                    for next_target_edge in graph.edges_directed(target_idx, Direction::Outgoing) {
                        let next_target_idx = next_target_edge.target();
                        next_inputs
                            .entry(next_target_idx)
                            .or_insert(vec![])
                            .push(next_target_edge.weight().eval(result));
                    }
                }
            }
            inputs = next_inputs;
        }

        Ok(outputs)
    }

    fn gen_anon_bind(&mut self) -> parser::Binding {
        let out = parser::Binding(format!("__anon_{}", self.next_anon_bind));
        self.next_anon_bind += 1;
        out
    }

    #[async_recursion::async_recursion]
    async fn parse_expr(
        &mut self,
        expr: parser::Expr,
        input_bindings: &FxHashMap<parser::Binding, NodeIndex>,
    ) -> Result<FxHashMap<parser::Binding, NodeIndex>> {
        let idxs = match expr {
            parser::Expr::Binding(bind) => FxHashMap::from_iter(
                [(bind.to_owned(), input_bindings[&bind].to_owned())].into_iter(),
            ),
            parser::Expr::Not(expr) => {
                let not_expr = self.parse_expr(*expr.expr, input_bindings).await?;
                assert_eq!(not_expr.len(), 1);
                let bind = not_expr.keys().next().unwrap();
                FxHashMap::from_iter(
                    [(
                        self.gen_anon_bind(),
                        self.connect(
                            BinaryOp::A,
                            not_expr[bind],
                            UnaryOp::Not,
                            not_expr[bind],
                            UnaryOp::Not,
                        )
                        .await?,
                    )]
                    .into_iter(),
                )
            }
            parser::Expr::BinaryExpr(expr) => {
                let a_expr = self.parse_expr(*expr.a, input_bindings).await?;
                let b_expr = self.parse_expr(*expr.b, input_bindings).await?;
                assert_eq!(a_expr.len(), 1);
                assert_eq!(b_expr.len(), 1);
                let a_bind = a_expr.keys().next().unwrap();
                let b_bind = b_expr.keys().next().unwrap();
                let op = match expr.op {
                    parser::BinaryOperator::And => BinaryOp::And,
                    parser::BinaryOperator::Or => BinaryOp::Or,
                    parser::BinaryOperator::Xor => BinaryOp::Xor,
                };
                let op_bind = self.gen_anon_bind();
                FxHashMap::from_iter(
                    [(
                        op_bind,
                        self.connect(
                            op,
                            a_expr[a_bind],
                            UnaryOp::Identity,
                            b_expr[b_bind],
                            UnaryOp::Identity,
                        )
                        .await?,
                    )]
                    .into_iter(),
                )
            }
            parser::Expr::CircuitCall(circ) => {
                // merge

                let circ_def = self.patterns[&circ.name].clone();

                let mut mappings = FxHashMap::default();
                // assert_eq!(
                //     circ_def.output_nodes().len(),
                //     1,
                //     "todo: multiple output circuits"
                // );
                let their_outputs_their_graph = circ_def.output_nodes();

                let mut their_inputs_my_graph = FxHashMap::default();
                for expr in circ.inputs {
                    let parsed = self.parse_expr(expr, input_bindings).await.unwrap();
                    for (k, v) in parsed {
                        their_inputs_my_graph.insert(k, v);
                    }
                }
                // let their_inputs_my_graph = circ
                //     .inputs
                //     .into_iter()
                //     .flat_map(|expr| self.parse_expr(expr, input_bindings))
                //     .collect::<FxHashMap<_, _>>();

                let their_inputs_their_graph = circ_def.input_nodes();
                for ((_, their_input_my_graph), (_, their_input_their_graph)) in
                    their_inputs_my_graph
                        .iter()
                        .zip(their_inputs_their_graph.iter())
                {
                    mappings.insert(*their_input_their_graph, *their_input_my_graph);
                }

                let their_graph = circ_def.graph.lock().await;
                for their_edge in their_graph.edge_references() {
                    let my_source = mappings.get(&their_edge.source()).copied();
                    let my_source = if let Some(my_source) = my_source {
                        my_source
                    } else {
                        self.graph
                            .lock()
                            .await
                            .add_node(their_graph[their_edge.source()])
                    };
                    mappings.entry(their_edge.source()).or_insert(my_source);

                    let my_target = mappings.get(&their_edge.target()).copied();
                    let my_target = if let Some(my_target) = my_target {
                        my_target
                    } else {
                        self.graph
                            .lock()
                            .await
                            .add_node(their_graph[their_edge.target()])
                    };
                    mappings.entry(their_edge.target()).or_insert(my_target);

                    self.graph
                        .lock()
                        .await
                        .add_edge(my_source, my_target, *their_edge.weight());
                }

                their_outputs_their_graph
                    .iter()
                    .map(|(bind, out)| (parser::Binding(bind.to_owned()), mappings[out]))
                    .collect()
            }
        };
        Ok(idxs)
    }

    pub async fn parse(script: &str) -> Result<Self> {
        let script = script.lines().collect::<Vec<_>>().join(" ");
        let (_junk, circs) = parser::parse_circuits(&script)
            .map_err(|e| Error::msg(format!("Parsing error: {}", e)))?;
        Self::parse_impl(circs).await
    }

    #[async_recursion::async_recursion]
    async fn parse_impl(mut other_circs: Vec<parser::Circuit>) -> Result<Self> {
        let main_circ = other_circs.pop().unwrap();
        let mut this = Self::new(&main_circ.name);

        let mut inputs = FxHashMap::default();
        for input in main_circ.inputs.iter() {
            inputs.insert(input.to_owned(), this.add_input().await);
        }
        this.input_nodes = inputs
            .iter()
            .map(|(b, idx)| (b.0.to_owned(), *idx))
            .collect();

        for other_circ in other_circs.clone() {
            this.patterns.insert(
                other_circ.name,
                Self::parse_impl(other_circs.clone()).await?,
            );
        }
        for assignment in main_circ.logic.iter() {
            let parser::Assignment { targets, expr } = assignment;
            let target_ids = this.parse_expr(expr.to_owned(), &inputs).await?;
            for (target_id, target) in target_ids.iter().zip(targets.iter()) {
                println!(
                    "Adding target id {} for target {}",
                    target_id.1.index(),
                    target.0
                );
                inputs.entry(target.to_owned()).or_insert(*target_id.1);
                if main_circ.outputs.contains(target) {
                    this.output_nodes.insert(target.0.to_owned(), *target_id.1);
                }
            }
        }

        Ok(this)
    }

    pub async fn write_svg(&self, filename: PathBuf) -> Result<()> {
        use std::io::Write;
        write!(
            &mut File::create(filename.with_extension("dot"))?,
            "{:?}",
            Dot::with_config(&*self.graph.lock().await, &[])
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

// #[cfg(test)]
// mod tests {
//     use std::fs::File;
//     use std::io::Write;

//     use petgraph::dot::Dot;

//     use crate::{
//         bit::Bit,
//         module::{BinaryOp, Circuit, UnaryOp},
//     };

//     // #[test]
//     // fn test_circuit_parse() {
//     //     env_logger::init();
//     //     let script = include_str!("parser/test_scripts/sr_latch.pals");
//     //     let mut circ = Circuit::parse(script).unwrap();
//     //     let dot = Dot::with_config(&circ.graph, &[]);
//     //     println!("{:?}", dot);
//     //     write!(&mut File::create("sr_latch.dot").unwrap(), "{:?}", dot).unwrap();
//     //     let input_nodes = circ.input_nodes();
//     //     let a = input_nodes[0];
//     //     let b = input_nodes[1];
//     //     let last = input_nodes[2];
//     //     let c = circ.output_nodes()[0];
//     //     let outputs = circ
//     //         .eval(&[(a, Bit::HI), (b, Bit::LO), (last, Bit::LO)])
//     //         .unwrap();
//     //     let out = outputs[&c];
//     //     dbg!(out);
//     // }
// }
