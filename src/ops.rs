use std::time::Duration;

use crate::bit::{Bit, PBit, PBitHandle};
use anyhow::{Error, Result};
use async_channel::Receiver;
use petgraph::prelude::*;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
}

#[derive(Clone, Copy)]
pub struct BinaryInput(Bit, Bit);

#[derive(Debug)]
pub struct OwnedBinaryOp {
    pub idx: NodeIndex,
    pub op: BinaryOp,
    pub(crate) handle: Option<PBitHandle>,
    pub(crate) inp_a_id: Option<EdgeIndex>,
    pub(crate) inp_b_id: Option<EdgeIndex>,
    inp_a: Option<Receiver<Bit>>,
    inp_b: Option<Receiver<Bit>>,
    out: Receiver<Bit>,
}

impl Drop for OwnedBinaryOp {
    fn drop(&mut self) {}
}

impl OwnedBinaryOp {
    pub fn new(idx: NodeIndex, op: BinaryOp) -> Self {
        let mut handle = PBit::init(Bit::LO);
        Self {
            idx,
            op,
            out: handle.subscribe(),
            handle: Some(handle),
            inp_a: None,
            inp_b: None,
            inp_a_id: None,
            inp_b_id: None,
        }
    }

    pub fn set_input_a(&mut self, idx: Option<EdgeIndex>, rx: Receiver<Bit>) {
        self.inp_a = Some(rx);
        self.inp_a_id = idx;
    }

    pub fn set_input_b(&mut self, idx: Option<EdgeIndex>, rx: Receiver<Bit>) {
        self.inp_b = Some(rx);
        self.inp_b_id = idx;
    }

    pub fn get_output(&self) -> Receiver<Bit> {
        self.out.clone()
    }

    pub fn try_spawn(&mut self) -> Result<()> {
        if let Some(a) = self.inp_a.as_ref().cloned() {
            if self.op == BinaryOp::A {
                let op = self.op;
                let mut handle = std::mem::take(&mut self.handle).unwrap();
                handle.spawn();
                let idx = self.idx.index();
                tokio::spawn(async move {
                    loop {
                        match a.recv().await {
                            Ok(a) => handle.set(op.eval(a, a)).await,
                            Err(_e) => {
                                println!("{:?} {}: a_res disconnected", op, idx);
                                return;
                            }
                        }
                    }
                });
                return Ok(());
            } else if let Some(b) = self.inp_b.as_ref().cloned() {
                let op = self.op;
                let mut handle = std::mem::take(&mut self.handle).unwrap();
                handle.spawn();
                let idx = self.idx.index();
                let mut last_a = Bit::LO;
                let mut last_b = Bit::LO;

                tokio::spawn(async move {
                    {
                        loop {
                            tokio::select! {
                                a_res = a.recv() => match a_res {
                                    Ok(a) => {
                                        last_a = a;
                                        handle.set(op.eval(a, last_b)).await;
                                    }
                                    Err(_e) => {
                                        // tracing::error!("a_res is Err");
                                        println!("{:?} {}: a_res disconnected", op, idx);
                                        return;
                                    }
                                },
                                b_res = b.recv() => match b_res {
                                    Ok(b) => {
                                        last_b = b;
                                        handle.set(op.eval(last_a, b)).await;
                                    }
                                    Err(_e) => {
                                        println!("{:?} {}: b_res disconnected", op, idx);
                                        return;
                                    }
                                },
                            }
                        }
                    }
                });
                return Ok(());
            }
        }
        Err(Error::msg("Cannot spawn op: both inputs must be Some"))
    }
}
#[derive(Debug)]
pub struct OwnedUnaryOp {
    pub idx: EdgeIndex,
    pub op: UnaryOp,
    pub(crate) inp_idx: Option<NodeIndex>,
    pub(crate) handle: Option<PBitHandle>,
    inp: Option<Receiver<Bit>>,
    out: Receiver<Bit>,
}

impl Drop for OwnedUnaryOp {
    fn drop(&mut self) {}
}

impl OwnedUnaryOp {
    pub fn new(idx: EdgeIndex, op: UnaryOp) -> Self {
        let mut handle = PBit::init(Bit::LO);
        Self {
            idx,
            op,
            inp_idx: None,
            out: handle.subscribe(),
            handle: Some(handle),
            inp: None,
        }
    }

    pub fn set_input(&mut self, idx: NodeIndex, rx: Receiver<Bit>) {
        self.inp = Some(rx);
        self.inp_idx = Some(idx);
    }

    pub fn get_output(&self) -> Receiver<Bit> {
        self.out.clone()
    }

    pub fn try_spawn(&mut self) -> Result<()> {
        if let Some(x) = self.inp.as_ref().cloned() {
            let mut handle = std::mem::take(&mut self.handle).unwrap();
            handle.spawn();
            let op = self.op;
            let idx = self.idx.index();
            tokio::spawn(async move {
                loop {
                    match x.recv().await {
                        Ok(x) => {
                            handle.set(op.eval(x)).await;
                        }
                        Err(_e) => {
                            println!("{:?} {}: x_res disconnected", op, idx);
                            return;
                        }
                    }
                }
            });
            return Ok(());
        }
        Err(Error::msg("Cannot spawn op: input must be Some"))
    }
}