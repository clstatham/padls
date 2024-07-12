use std::path::PathBuf;

use eframe::egui::{self};
use rustc_hash::FxHashMap;

use crate::{
    parser::{parse_circuits_pretty, Circuit},
    runner::CircuitRunner,
};

pub struct PadlsApp {
    pub circuit: Circuit,
    pub runner: CircuitRunner,
    pub inputs: FxHashMap<String, usize>,
    pub outputs: FxHashMap<String, usize>,
}

impl PadlsApp {
    pub fn new(script_path: PathBuf) -> Self {
        let script = std::fs::read_to_string(script_path).unwrap();
        let circuits = parse_circuits_pretty(&script).unwrap();
        let circuit = circuits.into_iter().last().unwrap();

        let dot = circuit.dot();
        std::fs::write("circuit.dot", dot).unwrap();

        let runner = CircuitRunner::new();
        runner.run(&circuit).unwrap();

        let inputs = circuit
            .inputs
            .iter()
            .map(|binding| {
                let idx = circuit.bindings[binding];
                (binding.to_owned(), idx.index())
            })
            .collect::<FxHashMap<_, _>>();

        let outputs = circuit
            .outputs
            .iter()
            .map(|binding| {
                let idx = circuit.bindings[binding];
                (binding.to_owned(), idx.index())
            })
            .collect::<FxHashMap<_, _>>();

        Self {
            circuit,
            runner,
            inputs,
            outputs,
        }
    }
}

impl eframe::App for PadlsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut state = self.runner.state.write().unwrap();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Inputs");

            for (name, idx) in &self.inputs {
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.checkbox(state[*idx].as_bool_mut(), "");
                });
            }

            ui.heading("Outputs");

            for (name, idx) in &self.outputs {
                ui.horizontal(|ui| {
                    ui.label(name);
                    ui.checkbox(state[*idx].as_bool_mut(), "");
                });
            }
        });

        ctx.request_repaint();
    }
}
