use std::path::PathBuf;

use eframe::egui::{self};

use crate::{
    bit::Bit,
    parser::{parse_circuits_pretty, Circuit},
    runner::CircuitRunner,
};

pub trait UiExt {
    fn bit(&mut self, value: &mut Bit, text: &str);
}

impl UiExt for egui::Ui {
    fn bit(&mut self, value: &mut Bit, text: &str) {
        let b = value.as_bool_mut();
        let color = if *b {
            egui::Color32::GREEN
        } else {
            egui::Color32::DARK_RED
        };
        self.horizontal(|ui| {
            ui.label(text);
            if ui
                .add(
                    egui::Button::new("")
                        .min_size(egui::Vec2::new(30.0, 30.0))
                        .fill(color),
                )
                .clicked()
            {
                *b = !*b;
            }
        });
    }
}

pub struct PadlsApp {
    pub circuit: Circuit,
    pub runner: CircuitRunner,
    pub inputs: Vec<(String, usize)>,
    pub outputs: Vec<(String, usize)>,
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
            .collect();

        let outputs = circuit
            .outputs
            .iter()
            .map(|binding| {
                let idx = circuit.bindings[binding];
                (binding.to_owned(), idx.index())
            })
            .collect();

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
                ui.bit(&mut state[*idx], name);
            }

            ui.heading("Outputs");

            for (name, idx) in &self.outputs {
                ui.bit(&mut state[*idx], name);
            }
        });

        ctx.request_repaint();
    }
}
