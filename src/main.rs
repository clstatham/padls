#![allow(clippy::type_complexity)]

use std::time::Duration;

use bit::{AsyncBit, Bit};
use module::Circuit;
use rustc_hash::FxHashMap;

pub mod bit;
pub mod module;
pub mod parser;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // env_logger::init();
    // console_subscriber::init();
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let script = include_str!("parser/test_scripts/full_adder.pals");
    let circ: Circuit = Circuit::parse(script).await.unwrap();
    circ.write_svg("simple_and_gate.svg".into()).await?;
    let a_idx = circ.input_nodes()["a"];
    let b_idx = circ.input_nodes()["b"];
    let cin_idx = circ.input_nodes()["cin"];
    let s_idx = circ.output_nodes()["s"];
    let c_idx = circ.output_nodes()["c"];

    let a = AsyncBit::spawn(Bit::LO);
    let b = AsyncBit::spawn(Bit::LO);
    let cin = AsyncBit::spawn(Bit::LO);
    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (a_idx, a.get_rx()),
            (b_idx, b.get_rx()),
            (cin_idx, cin.get_rx()),
        ]))
        .await;
    let mut handles = vec![];
    handles.push(tokio::spawn(async move {
        loop {
            // println!("Sending HI");
            a.send(Bit::LO);
            b.send(Bit::LO);
            cin.send(Bit::LO);
            tokio::time::sleep(Duration::from_millis(100)).await;
            tokio::task::yield_now().await;
        }
    }));

    for (id, mut rx) in rxs {
        handles.push(tokio::spawn(async move {
            loop {
                rx.changed().await.unwrap();
                match id {
                    v if v == c_idx => println!("c = {}", *rx.borrow_and_update()),
                    v if v == s_idx => println!("s = {}", *rx.borrow_and_update()),
                    _ => unreachable!(),
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
                tokio::task::yield_now().await;
            }
        }))
    }
    for handle in handles {
        handle.await?;
    }

    Ok(())
}
