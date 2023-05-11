use std::{fs::File, io::Read, path::PathBuf, sync::Arc, time::Duration};

use eframe::egui::{self, *};
use petgraph::prelude::*;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    bit::{ABit, ABitBehavior, Bit, BIT_IDS},
    circuit::Circuit,
    parser::Binding,
    GLOBAL_QUEUE_INTERVAL, NUM_DISPLAYS,
};

struct InputCtx {
    idx: NodeIndex,
    name: Binding,
    bit: Option<ABit>,
    set_tx: mpsc::Sender<Bit>,
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
    script: String,
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
    fn new(script_path: PathBuf) -> (Self, AppManager) {
        let mut script = String::new();
        File::open(script_path)
            .unwrap()
            .read_to_string(&mut script)
            .unwrap();
        let circ = Circuit::parse(&script).unwrap();
        let mut input_idxs = vec![];
        let mut inputs = vec![];
        for idx in circ.input_nodes().iter() {
            input_idxs.push(*idx);
            let (set_tx, set_rx) = mpsc::channel(1);
            let bit = ABit::new(ABitBehavior::Normal { value: Bit::LO }, set_rx);
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
                script,
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
            input.set_tx.send(input.state).await.ok();
            let bit = input.bit.take().unwrap();
            bit.spawn_eager();
        }
        println!(
            "Spawned {} async bits!",
            BIT_IDS.load(std::sync::atomic::Ordering::SeqCst)
        );
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
                        inp.set_tx.send(bit).await.ok();
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

    fn set_input(&self, name: Binding, bit: Bit) {
        let (tx, _rx) = oneshot::channel();
        let cmd = AppControl::SetInput {
            name,
            bit,
            resp: tx,
        };
        self.tx.send(cmd).unwrap();
        // rx.await.unwrap()
    }
}

struct AppProps {
    state: Option<AppState>,
    manager: Arc<AppManager>,
    input_names: Vec<Binding>,
    output_names: Vec<Binding>,
    runtime: Option<tokio::runtime::Runtime>,
    node_states: FxHashMap<Binding, Bit>,
    node_states_rx: Option<mpsc::Receiver<FxHashMap<Binding, Option<Bit>>>>,
    nums: Vec<Vec<Bit>>,
    script: String,
}

impl AppProps {
    fn new(threads: u8, script_path: PathBuf) -> Self {
        let (app_state, manager) = AppState::new(script_path);
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
            script: app_state.script.to_owned(),
            state: Some(app_state),
            manager: Arc::new(manager),
            input_names,
            output_names,
            node_states_rx: None,
            runtime: Some(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(threads as usize)
                    .thread_name("padls-worker")
                    .global_queue_interval(GLOBAL_QUEUE_INTERVAL)
                    .build()
                    .unwrap(),
            ),
            node_states: FxHashMap::default(),
            nums: vec![vec![Bit::LO; 8]; NUM_DISPLAYS as usize],
        }
    }

    fn spawn(&mut self) {
        if let Some(mut state) = self.state.take() {
            if let Some(runtime) = self.runtime.take() {
                let manager = self.manager.clone();
                let _enter = runtime.enter();
                let (tx, rx) = mpsc::channel(16);
                self.node_states_rx = Some(rx);
                std::thread::Builder::new()
                    .name("padls-runtime".to_owned())
                    .spawn(move || {
                        runtime.block_on(async move {
                            tokio::spawn(async move {
                                loop {
                                    if let Some(states) = manager.query_nodes().await {
                                        tx.send(states).await.ok();
                                    }
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    tokio::task::yield_now().await;
                                }
                            });
                            state.spawn().await;
                        });
                    })
                    .unwrap();
            }
        }
    }
}

pub struct PadlsApp {
    props: AppProps,
}
impl PadlsApp {
    pub fn new(threads: u8, script_path: PathBuf) -> Self {
        let mut this = Self {
            props: AppProps::new(threads, script_path),
        };
        this.props.spawn();
        this
    }
}

impl eframe::App for PadlsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let binary_to_num = |num: &[Bit]| -> u8 {
            num[0].as_u8()
                | (num[1].as_u8() << 1)
                | (num[2].as_u8() << 2)
                | (num[3].as_u8() << 3)
                | (num[4].as_u8() << 4)
                | (num[5].as_u8() << 5)
                | (num[6].as_u8() << 6)
                | (num[7].as_u8() << 7)
        };
        let props = &mut self.props;
        if let Ok(states) = props.node_states_rx.as_mut().unwrap().try_recv() {
            for (name, state) in states.into_iter() {
                if let Some(bit) = state {
                    props.node_states.insert(name.to_owned(), bit);

                    if let Binding::Num8Bit {
                        display_idx,
                        bit_shift,
                    } = name
                    {
                        props.nums[display_idx as usize][bit_shift as usize] = bit;
                    }
                }
            }
            ctx.request_repaint();
        }
        #[allow(clippy::redundant_closure)]
        TopBottomPanel::top("top_panel").show(ctx, |ui| global_dark_light_mode_switch(ui));
        SidePanel::left("inputs").show(ctx, |ui| {
            for input in props.input_names.iter() {
                ui.label(format!("{}", input));
                let (text, color) = if let Some(bit) = props.node_states.get(input) {
                    if *bit == Bit::HI {
                        ("HI", Color32::RED)
                    } else {
                        ("LO", Color32::WHITE)
                    }
                } else {
                    ("ERROR", Color32::YELLOW)
                };
                if Button::new(text).fill(color).ui(ui).clicked() {
                    if let Some(bit) = props.node_states.get(input).copied() {
                        let input = input.to_owned();
                        let mgr = props.manager.clone();
                        mgr.set_input(input, !bit);
                    }
                }
            }
        });

        SidePanel::right("outputs").show(ctx, |ui| {
            for display in props.nums.iter() {
                ui.heading(format!("{}", binary_to_num(display)));
            }
            for output in props.output_names.iter() {
                if let Binding::Num8Bit { .. } = output {
                } else {
                    ui.label(format!("{}", output));
                    let state = props.node_states.get(output);
                    let (text, color) = if let Some(bit) = state {
                        if *bit == Bit::HI {
                            ("HI", Color32::RED)
                        } else {
                            ("LO", Color32::WHITE)
                        }
                    } else {
                        ("ERROR", Color32::YELLOW)
                    };
                    Button::new(text).fill(color).ui(ui);
                }
            }
        });

        CentralPanel::default().show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                TextEdit::multiline(&mut props.script)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .interactive(false)
                    .show(ui);
            });
        });
    }
}
