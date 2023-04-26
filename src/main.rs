#![allow(non_snake_case)]
#![allow(clippy::type_complexity)]

use std::{cell::RefCell, sync::Arc, time::Duration};

use bit::{ABit, Bit};
use circuit::Circuit;
use dioxus::prelude::*;
use dioxus_desktop::{Config, LogicalSize, WindowBuilder};
use parser::Binding;
use petgraph::prelude::*;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};

pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;
pub mod runtime;

pub const GLOBAL_QUEUE_INTERVAL: u32 = 61;
pub const CLOCK_FULL_INTERVAL: Duration = Duration::from_millis(200);
pub const HEARTBEAT: Duration = Duration::from_millis(10);

pub const NUM_DISPLAYS: u8 = 4;

struct InputCtx {
    idx: NodeIndex,
    name: Binding,
    bit: Option<ABit>,
    set_tx: watch::Sender<Bit>,
    state: Bit,
}

struct NodeCtx {
    rx: watch::Receiver<Bit>,
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
        let script = include_str!("parser/test_scripts/test.padls");
        let circ = Circuit::parse(script).unwrap();
        circ.write_dot("test".into()).unwrap();
        let mut input_idxs = vec![];
        let mut inputs = vec![];
        for idx in circ.input_nodes().iter() {
            input_idxs.push(*idx);
            let (set_tx, set_rx) = watch::channel(Bit::LO);
            let bit = ABit::new(bit::ABitBehavior::Normal { value: Bit::LO }, set_rx);
            inputs.push(InputCtx {
                idx: *idx,
                // state,
                state: Bit::LO,
                name: circ.node_bindings[idx].to_owned(),
                bit: Some(bit),
                set_tx,
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
            .spawn_eager(FxHashMap::from_iter(self.inputs.values_mut().map(|inp| {
                (
                    inp.idx,
                    (
                        inp.bit.as_ref().unwrap().subscribe(),
                        inp.bit.as_ref().unwrap().subscribe(),
                    ),
                )
            })))
            .unwrap();
        self.outputs = rxs
            .into_iter()
            .map(|(idx, rx)| {
                let name = self.circ.node_bindings[&idx].to_owned();
                (name, NodeCtx { rx })
            })
            .collect();
        for input in self.inputs.values_mut() {
            let bit = input.bit.take().unwrap();
            bit.spawn_eager();
        }
        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                AppControl::QueryNodes { resp } => {
                    let mut out = FxHashMap::default();
                    for (name, node) in self.inputs.iter() {
                        out.insert(name.to_owned(), Some(node.state));
                    }
                    for (name, node) in self.outputs.iter_mut() {
                        out.insert(name.to_owned(), Some(*node.rx.borrow_and_update()));
                    }
                    resp.send(Some(out)).ok();
                }
                AppControl::SetInput { name, bit, resp } => {
                    if let Some(inp) = self.inputs.get_mut(&name) {
                        inp.state = bit;
                        inp.set_tx.send(bit).ok();
                        resp.send(Some(())).ok();
                    } else {
                        resp.send(None).ok();
                    }
                }
            }
        }
        println!("Job's done!");
    }
}

impl AppManager {
    async fn query_nodes(&self) -> Option<FxHashMap<Binding, Option<Bit>>> {
        let (tx, rx) = oneshot::channel();
        let cmd = AppControl::QueryNodes { resp: tx };
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

    fn spawn(&self) {
        let mut inner: std::cell::RefMut<AppPropsInner> = self.inner.borrow_mut();
        inner.spawn();
    }
}

struct AppPropsInner {
    state: Option<AppState>,
    manager: Arc<AppManager>,
    input_names: Vec<Binding>,
    output_names: Vec<Binding>,
    runtime: Option<tokio::runtime::Runtime>,
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
            runtime: Some(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    // .worker_threads(2)
                    .thread_name("padls-worker")
                    .global_queue_interval(GLOBAL_QUEUE_INTERVAL)
                    .build()
                    .unwrap(),
            ),
        }
    }

    fn spawn(&mut self) {
        if let Some(mut state) = self.state.take() {
            if let Some(runtime) = self.runtime.take() {
                std::thread::Builder::new()
                    .name("padls-runtime".to_owned())
                    .spawn(move || {
                        runtime.block_on(async move {
                            state.spawn().await;
                        });
                    })
                    .unwrap();
            }
        }
    }
}

fn App(cx: Scope<AppProps>) -> Element {
    let binary_to_num =
        |num: &[Bit]| -> u8 {
            num[0].as_u8()
                | (num[1].as_u8() << 1)
                | (num[2].as_u8() << 2)
                | (num[3].as_u8() << 3)
                | (num[4].as_u8() << 4)
                | (num[5].as_u8() << 5)
                | (num[6].as_u8() << 6)
                | (num[7].as_u8() << 7)
        };

    let nums = use_ref(cx, || vec![vec![Bit::LO; 8]; NUM_DISPLAYS as usize]);


    let state = cx.props;
    let node_states = use_ref(cx, FxHashMap::default);
    let _query_nodes: &Coroutine<()> = use_coroutine(cx, move |_rx| {
        let mgr = state.manager();
        to_owned![node_states, nums];
        async move {
            loop {
                if let Some(s) = mgr.query_nodes().await {
                    for (name, bit) in s.into_iter() {
                        if let Some(bit) = bit {
                            node_states.write().insert(name.to_owned(), bit);

                            if let Binding::Num8Bit { display_idx, bit_shift } = name {
                                let mut num = nums.read()[display_idx as usize].to_owned();
                                num[bit_shift as usize] = bit;
                                nums.write()[display_idx as usize] = num;
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
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

    // const seven_seg_class: &str = "display-container display-size-12 display-no-";
    // let class_for_digit = |dig: u8| -> String {
    //     let mut class = seven_seg_class.to_owned();
    //     assert!(dig < 10);
    //     class.push_str(&dig.to_string());
    //     class
    // };
    
    state.spawn();
    cx.render(rsx! {
        style {
            include_str!("assets/style.css")
        }
        // style {
        //     include_str!("assets/seven_seg.css")
        // }
        body {
            // div {
            //     dangerous_inner_html: include_str!("assets/seven_seg.html")
            // }
            div {
                class: "container",
                div {
                    class: "inputs",
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
                    class: "outputs",
                    h4 { "Outputs" },
                    div {
                        for display in nums.read().clone().into_iter() {
                            h1 {
                                "{binary_to_num(&display)}"
                            }
                        }
                    }
                    for output in state.output_names().into_iter() {
                        if let Binding::Num8Bit { .. } = output {
                            rsx! {
                                div {}
                            }
                        } else {
                            rsx! {
                                div {
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
                                    "{output}",
                                }
                            }
                        }
                        
                    }

                    
                }
            }
        }

    })
}

fn main() {
    // console_subscriber::init();
    let props = AppProps::new();
    dioxus_desktop::launch_with_props(
        App,
        props,
        Config::default()
            .with_window(WindowBuilder::default().with_min_inner_size(LogicalSize::new(1000, 900))),
    );
}
