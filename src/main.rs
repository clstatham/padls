#![allow(non_snake_case)]
#![allow(clippy::type_complexity)]

use std::{cell::RefCell, sync::Arc, time::Duration};

use async_channel::Receiver;
use bit::{Bit, PBit, PBitHandle};
use circuit::Circuit;
use dioxus::prelude::*;
use dioxus_desktop::Config;
use petgraph::prelude::*;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};

pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;

pub const HEARTBEAT: Duration = Duration::from_micros(100);

pub(crate) async fn beat() {
    tokio::time::sleep(HEARTBEAT).await;
}

pub(crate) async fn yield_now() {
    beat().await;
    tokio::task::yield_now().await;
}

struct InputCtx {
    idx: NodeIndex,
    name: String,
    handle: PBitHandle,
    state: Bit,
}

#[derive(Clone)]
struct NodeCtx {
    name: String,
    rx: Receiver<Bit>,
}

struct AppState {
    circ: Circuit,
    inputs: FxHashMap<String, InputCtx>,
    outputs: FxHashMap<String, NodeCtx>,
    rx: mpsc::UnboundedReceiver<AppControl>,
}

#[derive(Debug)]
enum AppControl {
    QueryNodes {
        resp: oneshot::Sender<Option<FxHashMap<String, Option<Bit>>>>,
    },
    SetInput {
        name: String,
        bit: Bit,
        resp: oneshot::Sender<Option<()>>,
    },
}

#[derive(Clone)]
struct AppManager {
    tx: mpsc::UnboundedSender<AppControl>,
}

impl AppState {
    fn new() -> (Self, AppManager) {
        let script = include_str!("parser/test_scripts/full_adder.pals");
        let circ = Circuit::parse(script).unwrap();
        circ.write_svg("full_adder.svg".into()).unwrap();
        let mut input_idxs = vec![];
        let mut inputs = vec![];
        for idx in circ.input_nodes().iter() {
            input_idxs.push(*idx);
            let handle = PBit::init(Bit::LO);
            handle.set(Bit::LO);
            inputs.push(InputCtx {
                idx: *idx,
                // state,
                state: Bit::LO,
                name: circ.node_names[idx].to_owned(),
                handle,
            });
        }
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                rx,
                inputs: FxHashMap::from_iter(
                    inputs.into_iter().map(|inp| (inp.name.to_owned(), inp)),
                ),
                outputs: FxHashMap::default(),
                circ,
            },
            AppManager { tx },
        )
    }

    async fn spawn(&mut self) {
        let rxs = self
            .circ
            .spawn_eval(FxHashMap::from_iter(
                self.inputs
                    .values_mut()
                    .map(|inp| (inp.idx, inp.handle.subscribe())),
            ))
            .unwrap();
        self.outputs = rxs
            .into_iter()
            .map(|(idx, rx)| {
                let name = self.circ.node_names[&idx].to_owned();
                (name.to_owned(), NodeCtx { name, rx })
            })
            .collect();
        for input in self.inputs.values_mut() {
            input.handle.spawn();
        }
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                AppControl::QueryNodes { resp } => {
                    let mut out = FxHashMap::default();
                    for (name, node) in self.inputs.iter() {
                        out.insert(name.to_owned(), Some(node.state));
                    }
                    for (name, node) in self.outputs.iter() {
                        out.insert(name.to_owned(), node.rx.try_recv().ok());
                    }
                    resp.send(Some(out)).unwrap();
                }
                AppControl::SetInput { name, bit, resp } => {
                    if let Some(inp) = self.inputs.get_mut(&name) {
                        inp.state = bit;
                        inp.handle.set(bit);
                        resp.send(Some(())).unwrap();
                    } else {
                        resp.send(None).unwrap();
                    }
                }
            }
        }
        println!("Job's done?");
    }
}

impl AppManager {
    async fn query_nodes(&self) -> Option<FxHashMap<String, Option<Bit>>> {
        let (tx, rx) = oneshot::channel();
        let cmd = AppControl::QueryNodes {
            // name: name.to_owned(),
            resp: tx,
        };
        self.tx.send(cmd).unwrap();
        rx.await.unwrap()
    }

    async fn set_input(&self, name: String, bit: Bit) -> Option<()> {
        let (tx, rx) = oneshot::channel();
        let cmd = AppControl::SetInput {
            name: name.to_owned(),
            bit,
            resp: tx,
        };
        self.tx.send(cmd).unwrap();
        rx.await.unwrap()
    }
}

struct AppProps {
    inner: RefCell<AppPropsInner>,
}

impl AppProps {
    fn new() -> Self {
        Self {
            inner: RefCell::new(AppPropsInner::new()),
        }
    }

    fn input_names(&self) -> Vec<String> {
        self.inner.borrow().input_names.to_owned()
    }

    fn output_names(&self) -> Vec<String> {
        self.inner.borrow().output_names.to_owned()
    }

    fn manager(&self) -> Arc<AppManager> {
        self.inner.borrow().manager.clone()
    }

    fn spawn<'a>(&self, cx: Scope<'a, AppProps>) -> Option<&'a UseFuture<()>> {
        let mut inner = self.inner.borrow_mut();
        // use_future(cx, (), move |_| async move {
        inner.spawn(cx)
        // });
    }
}

struct AppPropsInner {
    state: Option<AppState>,
    manager: Arc<AppManager>,
    input_names: Vec<String>,
    output_names: Vec<String>,
}

impl AppPropsInner {
    fn new() -> Self {
        let (app_state, manager) = AppState::new();
        let input_names = app_state
            .circ
            .input_nodes()
            .iter()
            .map(|idx| app_state.circ.node_names[idx].to_owned())
            .collect::<Vec<_>>();
        let output_names = app_state
            .circ
            .output_nodes()
            .iter()
            .map(|idx| app_state.circ.node_names[idx].to_owned())
            .collect::<Vec<_>>();
        Self {
            state: Some(app_state),
            manager: Arc::new(manager),
            input_names,
            output_names,
        }
    }

    fn spawn<'a>(&mut self, cx: Scope<'a, AppProps>) -> Option<&'a UseFuture<()>> {
        let state = std::mem::take(&mut self.state);
        if let Some(mut state) = state {
            // cx.spawn_forever(async move {
            Some(use_future(cx, (), move |_| async move {
                state.spawn().await;
            }))
            // });
        } else {
            None
        }
    }
}

fn App(cx: Scope<AppProps>) -> Element {
    let state = cx.props;
    let node_states = use_ref(cx, FxHashMap::default);
    let _query_nodes: &Coroutine<()> = use_coroutine(cx, move |_rx| {
        let mgr = state.manager();
        to_owned![node_states];
        async move {
            loop {
                if let Some(s) = mgr.query_nodes().await {
                    for (name, bit) in s.into_iter() {
                        if let Some(bit) = bit {
                            node_states.write().insert(name, bit);
                        }
                    }
                }
                crate::yield_now().await;
            }
        }
    });

    let set_input: &Coroutine<(String, Bit)> = use_coroutine(cx, move |mut rx| {
        let state = state.manager();
        async move {
            use futures_util::stream::StreamExt;
            while let Some((name, value)) = rx.next().await {
                state.set_input(name, value).await;
            }
        }
    });

    // cx.spawn(async move {
    let spawn = state.spawn(cx);
    // });
    // let spawn = use_future(cx, state, move |_| async {});

    cx.render(rsx! {
        div {
            button {
                onclick: move |_| {
                    if let Some(spawn) = spawn {
                        spawn.restart();
                    }
                },
                "Spawn",
            }
        }
        div {
            h4 { "Inputs" },
            for input in state.input_names().into_iter() {
                div {
                    "{input}",
                    button {
                        onclick: move |_| {
                            to_owned![input];
                            if let Some(bit) = node_states.read().get(&input) {
                                set_input.send((input.to_owned(), !*bit));
                            }
                        },
                        if let Some(bit) = node_states.read().get(&input) {
                            if *bit == Bit::HI {
                                "HI"
                            } else {
                                "LO"
                            }
                        } else {
                            "ERROR"
                        }
                    }
                }
            }
        }
        div {
            h4 { "Outputs" },
            for output in state.output_names().into_iter() {
                div {
                    "{output}",
                    button {
                        if let Some(bit) = node_states.read().get(&output) {
                            if *bit == Bit::HI {
                                "HI"
                            } else {
                                "LO"
                            }
                        } else {
                            "ERROR"
                        }
                    }
                }
            }
        }
    })
}

#[allow(dead_code)]
async fn test() -> Result<(), Box<dyn std::error::Error>> {
    let script = include_str!("parser/test_scripts/test.pals");
    let mut circ: Circuit = Circuit::parse(script).unwrap();
    circ.write_svg("test.svg".into())?;
    let j_idx = circ.node_by_name("j").unwrap();
    let k_idx = circ.node_by_name("k").unwrap();

    let clk_idx = circ.node_by_name("clk").unwrap();
    let q_idx = circ.node_by_name("q").unwrap();
    let qbar_idx = circ.node_by_name("qbar").unwrap();

    let mut clk = PBit::init(Bit::LO);
    let mut j = PBit::init(Bit::LO);
    let mut k = PBit::init(Bit::LO);

    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (clk_idx, clk.subscribe()),
            (j_idx, j.subscribe()),
            (k_idx, k.subscribe()),
        ]))
        .unwrap();

    clk.spawn();
    j.spawn();
    k.spawn();

    let q_rx = rxs[&q_idx].clone();
    let qbar_rx = rxs[&qbar_idx].clone();
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    tokio::spawn(async move {
        // let mut ticks = 0;
        loop {
            interval.tick().await;
            println!("K HI");
            k.set(Bit::HI);
            tokio::time::sleep(Duration::from_millis(1)).await;
            k.set(Bit::LO);

            interval.tick().await;
            println!("J HI");
            j.set(Bit::HI);
            tokio::time::sleep(Duration::from_millis(1)).await;
            j.set(Bit::LO);
        }
    });
    tokio::spawn(async move {
        loop {
            clk.set(Bit::LO);
            tokio::time::sleep(Duration::from_millis(100)).await;
            clk.set(Bit::HI);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    loop {
        match q_rx.try_recv() {
            Ok(q) => println!("q = {:?}", q),
            Err(e) if e.is_closed() => println!("Q closed"),
            _ => {}
        }
        match qbar_rx.try_recv() {
            Ok(qbar) => println!("qbar = {:?}", qbar),
            Err(e) if e.is_closed() => println!("Qbar closed"),
            _ => {}
        }
        crate::yield_now().await;
    }

    // Ok(())
}

fn main() {
    let props = AppProps::new();
    dioxus_desktop::launch_with_props(App, props, Config::default());
}

// #[tokio::main]
// async fn main() {
//     test().await.unwrap();
// }
