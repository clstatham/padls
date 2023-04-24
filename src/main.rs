#![allow(non_snake_case)]
#![allow(clippy::type_complexity)]

use std::{cell::RefCell, sync::Arc, time::Duration};

use bit::{Bit, PBit, PBitHandle};
use circuit::Circuit;
use dioxus::prelude::*;
use dioxus_desktop::Config;
use parser::Binding;
use petgraph::prelude::*;
use rustc_hash::FxHashMap;
use tokio::sync::{broadcast, mpsc, oneshot};

pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;

pub const HEARTBEAT: Duration = Duration::from_millis(1);

pub(crate) async fn beat() {
    tokio::time::sleep(HEARTBEAT).await;
}

pub(crate) async fn yield_now() {
    beat().await;
    tokio::task::yield_now().await;
}

struct InputCtx {
    idx: NodeIndex,
    name: Binding,
    handle: PBitHandle,
    state: Bit,
}

struct NodeCtx {
    name: Binding,
    rx: broadcast::Receiver<Bit>,
}

struct AppState {
    circ: Circuit,
    inputs: FxHashMap<Binding, InputCtx>,
    outputs: FxHashMap<Binding, NodeCtx>,
    rx: mpsc::UnboundedReceiver<AppControl>,
}

#[derive(Debug)]
enum AppControl {
    QueryNodes {
        resp: oneshot::Sender<Option<FxHashMap<Binding, Option<Bit>>>>,
    },
    SetInput {
        name: Binding,
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
        let script = include_str!("parser/test_scripts/test.pals");
        let circ = Circuit::parse(script).unwrap();
        circ.write_svg("test.svg".into()).unwrap();
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
                name: circ.node_bindings[idx].to_owned(),
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
            .spawn_eval(FxHashMap::from_iter(self.inputs.values_mut().map(|inp| {
                (inp.idx, (inp.handle.subscribe(), inp.handle.subscribe()))
            })))
            .unwrap();
        self.outputs = rxs
            .into_iter()
            .map(|(idx, rx)| {
                let name = self.circ.node_bindings[&idx].to_owned();
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
                    for (name, node) in self.outputs.iter_mut() {
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
    async fn query_nodes(&self) -> Option<FxHashMap<Binding, Option<Bit>>> {
        let (tx, rx) = oneshot::channel();
        let cmd = AppControl::QueryNodes {
            // name: name.to_owned(),
            resp: tx,
        };
        self.tx.send(cmd).unwrap();
        rx.await.unwrap()
    }

    async fn set_input(&self, name: Binding, bit: Bit) -> Option<()> {
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

    fn input_names(&self) -> Vec<Binding> {
        self.inner.borrow().input_names.to_owned()
    }

    fn output_names(&self) -> Vec<Binding> {
        self.inner.borrow().output_names.to_owned()
    }

    fn manager(&self) -> Arc<AppManager> {
        self.inner.borrow().manager.clone()
    }

    fn spawn<'b>(&self, cx: Scope<'b, AppProps>) -> Option<&'b UseFuture<()>> {
        let mut inner = self.inner.borrow_mut();
        // use_future(cx, (), move |_| async move {
        inner.spawn(cx)
        // });
    }
}

struct AppPropsInner {
    state: Option<AppState>,
    manager: Arc<AppManager>,
    input_names: Vec<Binding>,
    output_names: Vec<Binding>,
}

impl AppPropsInner {
    fn new() -> Self {
        let (app_state, manager) = AppState::new();
        let input_names = app_state
            .circ
            .input_nodes()
            .iter()
            .map(|idx| app_state.circ.node_bindings[idx].to_owned())
            .collect::<Vec<_>>();
        let output_names = app_state
            .circ
            .output_nodes()
            .iter()
            .map(|idx| app_state.circ.node_bindings[idx].to_owned())
            .collect::<Vec<_>>();
        Self {
            state: Some(app_state),
            manager: Arc::new(manager),
            input_names,
            output_names,
        }
    }

    fn spawn<'b>(&mut self, cx: Scope<'b, AppProps>) -> Option<&'b UseFuture<()>> {
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

    let set_input: &Coroutine<(Binding, Bit)> = use_coroutine(cx, move |mut rx| {
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
                        background: if let Some(bit) = node_states.read().get(&input) {
                            if *bit == Bit::HI {
                                "red"
                            } else {
                                "white"
                            }
                        } else {
                            "white"
                        },
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
                        background: if let Some(bit) = node_states.read().get(&output) {
                            if *bit == Bit::HI {
                                "red"
                            } else {
                                "white"
                            }
                        } else {
                            "white"
                        },
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

fn main() {
    let props = AppProps::new();
    dioxus_desktop::launch_with_props(App, props, Config::default());
}

// #[tokio::main]
// async fn main() {
//     test().await.unwrap();
// }
