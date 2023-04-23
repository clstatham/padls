#![allow(clippy::type_complexity)]

use std::{thread, time::Duration};

use bit::{Bit, PBit};
use circuit::Circuit;
use rustc_hash::FxHashMap;

pub mod bit;
pub mod circuit;
pub mod ops;
pub mod parser;

pub(crate) async fn yield_now() {
    tokio::task::yield_now().await;
}

#[allow(dead_code)]
async fn test() -> Result<(), Box<dyn std::error::Error>> {
    let script = include_str!("parser/test_scripts/test.pals");
    let mut circ: Circuit = Circuit::parse(script).unwrap();
    circ.write_svg("test.svg".into())?;
    let clk_last_idx = circ.node_by_name("clk_last").unwrap();
    let j_idx = circ.node_by_name("j").unwrap();
    let k_idx = circ.node_by_name("k").unwrap();

    let clk_idx = circ.node_by_name("clk").unwrap();
    let q_idx = circ.node_by_name("q").unwrap();
    let qbar_idx = circ.node_by_name("qbar").unwrap();

    let mut clk_last = PBit::init(Bit::LO);
    let mut j = PBit::init(Bit::LO);
    let mut k = PBit::init(Bit::LO);

    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (clk_last_idx, clk_last.subscribe()),
            (j_idx, j.subscribe()),
            (k_idx, k.subscribe()),
        ]))
        .unwrap();

    let clk_rx = rxs[&clk_idx].clone();
    let q_rx = rxs[&q_idx].clone();
    let qbar_rx = rxs[&qbar_idx].clone();
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    tokio::spawn(async move {
        // let mut ticks = 0;
        loop {
            interval.tick().await;
            // thread::sleep(interval);
            println!("K HI");
            k.set(Bit::HI).await;
            tokio::time::sleep(Duration::from_millis(1)).await;
            k.set(Bit::LO).await;
            // j.send(Some(Bit::HI));
            // k.send(Some(Bit::LO));

            interval.tick().await;
            // thread::sleep(interval);
            println!("J HI");
            j.set(Bit::HI).await;
            tokio::time::sleep(Duration::from_millis(1)).await;
            j.set(Bit::LO).await;

            // ticks += 1;
        }
    });
    // tokio::spawn(async move {
    loop {
        tokio::select! {
            clk = clk_rx.recv() => {
                println!("clk = {:?}", clk.unwrap());
                clk_last.set(clk.unwrap()).await;
            },
            q = q_rx.recv() => {
                println!("q = {:?}", q.unwrap());
            },
            qbar = qbar_rx.recv() => {
                println!("qbar = {:?}", qbar.unwrap());
            },
        }
    }
    // });

    // Ok(())
}

#[allow(dead_code)]
async fn full_adder() -> Result<(), Box<dyn std::error::Error>> {
    let script = include_str!("parser/test_scripts/full_adder.pals");
    let mut circ: Circuit = Circuit::parse(script).unwrap();
    circ.write_svg("full_adder.svg".into())?;
    let a_idx = circ.node_by_name("a").unwrap();
    let b_idx = circ.node_by_name("b").unwrap();
    let cin_idx = circ.node_by_name("cin").unwrap();

    let s_idx = circ.node_by_name("s").unwrap();
    let c_idx = circ.node_by_name("c").unwrap();

    let mut a = PBit::init(Bit::LO);
    let mut b = PBit::init(Bit::LO);
    let mut cin = PBit::init(Bit::LO);

    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (a_idx, a.subscribe()),
            (b_idx, b.subscribe()),
            (cin_idx, cin.subscribe()),
        ]))
        .unwrap();

    // circ.dump_ops();

    a.spawn();
    b.spawn();
    cin.spawn();

    let s_rx = rxs[&s_idx].clone();
    let c_rx = rxs[&c_idx].clone();

    tokio::spawn(async move {
        loop {
            tokio::join!(a.set(Bit::HI), b.set(Bit::HI));
            tokio::time::sleep(Duration::from_millis(1)).await;
            tokio::join!(a.set(Bit::LO), b.set(Bit::LO));
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    loop {
        tokio::select! {
            val = s_rx.recv() => {println!("s = {:?} ", val);},
            val = c_rx.recv() => {println!("c = {:?} ", val);},
        }
    }

    // Ok(())
}

#[allow(dead_code)]
async fn simple_and_gate() -> Result<(), Box<dyn std::error::Error>> {
    let script = include_str!("parser/test_scripts/simple_and_gate.pals");
    let mut circ: Circuit = Circuit::parse(script).unwrap();
    circ.write_svg("simple_and_gate.svg".into())?;

    let a_idx = circ.node_by_name("a").unwrap();
    let b_idx = circ.node_by_name("b").unwrap();
    let c_idx = circ.node_by_name("c").unwrap();

    let mut a = PBit::init(Bit::LO);
    let mut b = PBit::init(Bit::LO);

    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (a_idx, a.subscribe()),
            (b_idx, b.subscribe()),
        ]))
        .unwrap();

    let c_rx = rxs[&c_idx].clone();
    a.spawn();
    b.spawn();
    tokio::spawn(async move {
        loop {
            a.set(Bit::HI).await;
            b.set(Bit::HI).await;
            tokio::time::sleep(Duration::from_millis(1000)).await;
            a.set(Bit::LO).await;
            b.set(Bit::LO).await;
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    });

    loop {
        if let Ok(c) = c_rx.recv().await {
            println!("c = {:?}", c);
        }
    }

    // Ok(())
}

#[allow(dead_code)]
async fn manual() {
    let mut circ = Circuit::new("manual");
    let a_idx = circ.add_input();
    let b_idx = circ.add_input();
    let o_idx = circ
        .connect_with(
            ops::BinaryOp::And,
            a_idx,
            ops::UnaryOp::Identity,
            b_idx,
            ops::UnaryOp::Identity,
            true,
        )
        .unwrap();
    circ.dump_ops();
    circ.write_svg("manual.svg".into()).unwrap();
    let mut a = PBit::init(Bit::LO);
    let mut b = PBit::init(Bit::LO);
    let rxs = circ
        .spawn_eval(FxHashMap::from_iter([
            (a_idx, a.subscribe()),
            (b_idx, b.subscribe()),
        ]))
        .unwrap();
    a.spawn();
    b.spawn();
    let o_rx = rxs[&o_idx].clone();

    a.set(Bit::HI).await;
    b.set(Bit::HI).await;
    loop {
        if let Ok(o) = o_rx.try_recv() {
            println!("o = {:?}", o);
        }

        crate::yield_now().await;
    }
}

#[allow(dead_code)]
async fn manual2() {
    let mut a = PBit::init(Bit::HI);
    let a_rx = a.subscribe();
    a.spawn();
    a.set(Bit::LO).await;
    tokio::spawn(async move {
        loop {
            a.set(Bit::HI).await;
            crate::yield_now().await;
        }
    });
    tokio::spawn(async move {
        loop {
            let a = a_rx.recv();
            println!("{:?}", a);
            crate::yield_now().await;
        }
    });

    loop {
        crate::yield_now().await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // env_logger::init();
    // console_subscriber::init();
    test().await?;

    // global::shutdown_tracer_provider();

    Ok(())
}
