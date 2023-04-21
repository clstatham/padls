use std::fmt::{Debug, Display};

use derive_more::*;
use tokio::sync::watch;

#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    From,
    Into,
    Constructor,
    Not,
    BitAnd,
    BitOr,
    BitXor,
)]
pub struct Bit(bool);

impl Bit {
    pub const HI: Self = Self(true);
    pub const LO: Self = Self(false);
}

impl Debug for Bit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 {
            write!(f, "1")
        } else {
            write!(f, "0")
        }
    }
}

impl Display for Bit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// pub type BitFuture = Pin<Box<dyn Future<Output = ()>>>;

pub struct AsyncBit {
    bit: Bit,
    input_rx: watch::Receiver<Bit>,
    output_tx: watch::Sender<Bit>,
}

pub struct BitListener {
    // bit: AsyncBit,
    input_tx: watch::Sender<Bit>,
    output_rx: watch::Receiver<Bit>,
}

impl AsyncBit {
    pub fn spawn(bit: Bit) -> BitListener {
        let (input_tx, input_rx) = watch::channel(bit);
        let (output_tx, output_rx) = watch::channel(bit);
        let this = Self {
            bit,
            input_rx,
            output_tx,
        };
        this.spawn_internal();
        BitListener {
            // bit: this,
            input_tx,
            output_rx,
        }
    }

    fn spawn_internal(mut self) {
        tokio::spawn(async move {
            loop {
                self.bit = *self.input_rx.borrow_and_update();
                self.output_tx.send(self.bit).unwrap();
                tokio::task::yield_now().await;
            }
        });
    }
}

impl BitListener {
    pub fn borrow_and_update(&mut self) -> Bit {
        *self.output_rx.borrow_and_update()
    }

    pub fn send(&self, bit: Bit) {
        self.input_tx.send(bit).unwrap();
    }

    pub fn get_rx(&self) -> watch::Receiver<Bit> {
        self.output_rx.clone()
    }
}

// impl std::fmt::Debug for BitListener {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.borrow())
//     }
// }
