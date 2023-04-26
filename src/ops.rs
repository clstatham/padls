use crate::{
    bit::{ABit, ABitBehavior, Bit, SpawnResult},
    HEARTBEAT,
};

use async_timer::Interval;
use petgraph::prelude::*;
use tokio::sync::watch;

#[derive(Clone, Copy)]
pub enum UnaryGate {
    Identity,
    Not,
}

impl std::fmt::Debug for UnaryGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Self::Not = self {
            write!(f, "Not")?;
        }
        Ok(())
    }
}

impl UnaryGate {
    pub fn eval(self, x: Bit) -> Bit {
        match self {
            Self::Identity => x,
            Self::Not => !x,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryGate {
    And,
    Or,
    Xor,
    AlwaysA,
    IgnoreInput(ABitBehavior),
}

impl BinaryGate {
    pub fn eval(self, a: Bit, b: Bit) -> Bit {
        match self {
            Self::AlwaysA => a,
            Self::And => a & b,
            Self::Or => a | b,
            Self::Xor => a ^ b,
            Self::IgnoreInput(bh) => panic!(
                "eval() called on BinaryOp that ignores input, and instead uses PBitBehavior {:?}",
                bh
            ),
        }
    }
}

pub struct OwnedBinaryGate {
    pub idx: NodeIndex,
    pub gate: BinaryGate,
    pub(crate) bit: Option<ABit>,
    set_tx: Option<watch::Sender<Bit>>,
    pub(crate) inp_a_id: Option<EdgeIndex>,
    pub(crate) inp_b_id: Option<EdgeIndex>,
    inp_a: Option<watch::Receiver<Bit>>,
    inp_b: Option<watch::Receiver<Bit>>,
    _out: watch::Receiver<Bit>,
    interval: Interval,
}

impl Drop for OwnedBinaryGate {
    fn drop(&mut self) {
        // println!("Warning, op {:?} is being dropped", self.idx);
    }
}

impl OwnedBinaryGate {
    pub fn new(idx: NodeIndex, gate: BinaryGate) -> Self {
        let bh = if let BinaryGate::IgnoreInput(bh) = gate {
            bh
        } else {
            ABitBehavior::Normal { value: Bit::LO }
        };
        let initial = if let ABitBehavior::Normal { value } = bh {
            value
        } else {
            Bit::LO
        };
        let (set_tx, set_rx) = watch::channel(initial);
        let handle = ABit::new(bh, set_rx);
        Self {
            idx,
            gate,
            _out: handle.subscribe(),
            bit: Some(handle),
            set_tx: Some(set_tx),
            inp_a: None,
            inp_b: None,
            inp_a_id: None,
            inp_b_id: None,
            interval: Interval::platform_new(HEARTBEAT),
        }
    }

    pub fn set_input_a(&mut self, idx: Option<EdgeIndex>, rx: watch::Receiver<Bit>) {
        self.inp_a = Some(rx);
        self.inp_a_id = idx;
    }

    pub fn set_input_b(&mut self, idx: Option<EdgeIndex>, rx: watch::Receiver<Bit>) {
        self.inp_b = Some(rx);
        self.inp_b_id = idx;
    }

    pub fn subscribe(&self) -> watch::Receiver<Bit> {
        self.bit.as_ref().unwrap().subscribe()
    }

    pub fn spawn_eager(&mut self) -> SpawnResult {
        let mut interval = Interval::platform_new(self.interval.interval);
        if let BinaryGate::IgnoreInput(_bh) = self.gate {
            let bit = self.bit.take().unwrap();
            bit.spawn_eager();
            SpawnResult::Ok
        } else if let Some(mut inp_a) = self.inp_a.take() {
            if self.gate == BinaryGate::AlwaysA {
                let bit = self.bit.take().unwrap();
                let set_tx = self.set_tx.take().unwrap();
                bit.spawn_eager();
                tokio::spawn(async move {
                    loop {
                        let a = *inp_a.borrow_and_update();
                        set_tx.send(a).ok();
                        interval.wait().await;
                    }
                });

                SpawnResult::Ok
            } else if let Some(mut inp_b) = self.inp_b.take() {
                let bit = self.bit.take().unwrap();
                let set_tx = self.set_tx.take().unwrap();
                bit.spawn_eager();
                let op = self.gate;
                tokio::spawn(async move {
                    loop {
                        let last_a = *inp_a.borrow_and_update();
                        let last_b = *inp_b.borrow_and_update();
                        set_tx.send(op.eval(last_a, last_b)).ok();
                        interval.wait().await;
                    }
                });

                SpawnResult::Ok
            } else {
                SpawnResult::NotConnected
            }
        } else {
            SpawnResult::NotConnected
        }
    }
}
pub struct OwnedUnaryGate {
    pub idx: EdgeIndex,
    pub gate: UnaryGate,
    pub(crate) inp_idx: Option<NodeIndex>,
    pub(crate) bit: Option<ABit>,
    set_tx: Option<watch::Sender<Bit>>,
    inp: Option<watch::Receiver<Bit>>,
    _out: watch::Receiver<Bit>,
    interval: Interval,
}

impl Drop for OwnedUnaryGate {
    fn drop(&mut self) {
        // println!("Warning, op {:?} is being dropped", self.idx);
    }
}

impl OwnedUnaryGate {
    pub fn new(idx: EdgeIndex, gate: UnaryGate) -> Self {
        let (set_tx, set_rx) = watch::channel(Bit::LO);
        let handle = ABit::new(ABitBehavior::Normal { value: Bit::LO }, set_rx);
        Self {
            idx,
            gate,
            inp_idx: None,
            set_tx: Some(set_tx),
            _out: handle.subscribe(),
            bit: Some(handle),
            inp: None,
            interval: Interval::platform_new(HEARTBEAT),
        }
    }

    pub fn set_input(&mut self, idx: NodeIndex, rx: watch::Receiver<Bit>) {
        self.inp = Some(rx);
        self.inp_idx = Some(idx);
    }

    pub fn get_output(&self) -> watch::Receiver<Bit> {
        self.bit.as_ref().unwrap().subscribe()
    }

    pub fn spawn_eager(&mut self) -> SpawnResult {
        let mut interval = Interval::platform_new(self.interval.interval);
        if let Some(mut inp) = self.inp.take() {
            let bit = self.bit.take().unwrap();
            let set_tx = self.set_tx.take().unwrap();
            bit.spawn_eager();
            let op = self.gate;
            tokio::spawn(async move {
                loop {
                    let x = *inp.borrow_and_update();
                    set_tx.send(op.eval(x)).ok();
                    interval.wait().await;
                }
            });

            SpawnResult::Ok
        } else {
            SpawnResult::NotConnected
        }
    }
}
