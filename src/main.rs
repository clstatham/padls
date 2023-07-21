use std::{
    borrow::BorrowMut, cell::RefCell, fs::File, io::Read, path::PathBuf, sync::Arc, time::Duration,
};

use bit::{ABit, Bit};
use circuit::Circuit;
use eframe::{
    egui::{Button, CentralPanel, Layout, SidePanel, Visuals},
    epaint::Color32,
    NativeOptions,
};
use parser::Binding;
use petgraph::prelude::*;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot, watch};

use eframe::NativeOptions;

pub mod app;
pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;
pub mod runtime;
// mod main_old;

pub const GLOBAL_QUEUE_INTERVAL: u32 = 1;
pub const CLOCK_FULL_INTERVAL: Duration = Duration::from_millis(2000);

pub const NUM_DISPLAYS: u8 = 4;

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

struct AppPropsInner {
    state: Option<AppState>,
    manager: Arc<AppManager>,
    input_names: Vec<Binding>,
    output_names: Vec<Binding>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl AppPropsInner {
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
            state: Some(app_state),
            manager: Arc::new(manager),
            input_names,
            output_names,
            runtime: Some(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(threads as usize)
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

struct PadlsApp {
    props: RefCell<AppPropsInner>,
    node_states: FxHashMap<Binding, Bit>,
    nums: Vec<Vec<Bit>>,
}

impl PadlsApp {
    pub fn new(threads: u8, script_path: PathBuf) -> Self {
        Self {
            props: RefCell::new(AppPropsInner::new(threads, script_path)),
            node_states: FxHashMap::default(),
            nums: vec![vec![Bit::LO; 8]; NUM_DISPLAYS as usize],
        }
    }
}

impl eframe::App for PadlsApp {
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
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

        let state = self.props.borrow();
        {
            let mgr = state.manager.clone();
            if let Some(s) = pollster::block_on(async move { mgr.query_nodes().await }) {
                for (name, bit) in s.into_iter() {
                    if let Some(bit) = bit {
                        self.node_states.insert(name.to_owned(), bit);

                        if let Binding::Num8Bit {
                            display_idx,
                            bit_shift,
                        } = name
                        {
                            let mut num = self.nums[display_idx as usize].to_owned();
                            num[bit_shift as usize] = bit;
                            self.nums[display_idx as usize] = num;
                        }
                    }
                }
            }
        }
        CentralPanel::default().show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Inputs");
                    for input in state.input_names.iter().cloned() {
                        ui.horizontal(|ui| {
                            ui.label(input.to_string());
                            let bit = self.node_states.get(&input).cloned();
                            let (bit_text, bit_color) = if let Some(bit) = bit {
                                if bit == Bit::HI {
                                    ("    ", Color32::GREEN)
                                } else {
                                    ("    ", Color32::WHITE)
                                }
                            } else {
                                ("    ", Color32::RED)
                            };
                            let btn = ui.add(Button::new(bit_text).fill(bit_color));
                            if btn.clicked() {
                                let mgr = state.manager.clone();
                                if let Some(bit) = bit {
                                    pollster::block_on(async move {
                                        mgr.set_input(input, !bit).await;
                                    });
                                }
                            }
                        });
                    }
                });
                ui.vertical(|ui| {
                    ui.heading("Outputs");
                    for display in self.nums.iter() {
                        ui.heading(binary_to_num(display).to_string());
                    }
                    for output in state.output_names.iter().cloned() {
                        if let Binding::Num8Bit { .. } = output {
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(output.to_string());
                                let bit = self.node_states.get(&output).cloned();
                                let (bit_text, bit_color) = if let Some(bit) = bit {
                                    if bit == Bit::HI {
                                        ("    ", Color32::GREEN)
                                    } else {
                                        ("    ", Color32::WHITE)
                                    }
                                } else {
                                    ("    ", Color32::RED)
                                };
                                ui.add(Button::new(bit_text).fill(bit_color));
                            });
                        }
                    }
                });
            });
        });

        ctx.request_repaint();
    }
}

use clap::Parser;
#[derive(Parser, Debug)]
struct Args {
    script_path: PathBuf,
    #[arg(short, long, default_value_t = 1)]
    threads: u8,
}

fn main() {
    let args = Args::parse();
    let app = PadlsApp::new(args.threads, args.script_path);
    eframe::run_native(
        "padls",
        NativeOptions::default(),
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(Visuals::dark());
            let app = PadlsApp::new(args.threads, args.script_path);
            app.props.borrow_mut().spawn();
            Box::new(app)
        }),
    )
    .unwrap();
}
