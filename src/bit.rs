use std::{
    fmt::{Debug, Display},
    sync::atomic::AtomicUsize,
};

use async_channel::{Receiver, Sender};
use derive_more::*;
use tokio::task::JoinHandle;

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

static BIT_IDS: AtomicUsize = AtomicUsize::new(0);

/// Parallel Bit
#[derive(Debug)]
pub struct PBit {
    id: usize,
    bit: Bit,
    set_rx: Receiver<Bit>,
    get_txs: Vec<Sender<Bit>>,
}

impl Drop for PBit {
    fn drop(&mut self) {
        // println!("PBit {:?} is being dropped", self.id);
    }
}

#[derive(Debug)]
pub struct PBitHandle {
    handle: Option<JoinHandle<()>>,
    bit: Option<PBit>,
    set_tx: Sender<Bit>,
}

impl PBit {
    pub fn init(initial: Bit) -> PBitHandle {
        let (set_tx, set_rx) = async_channel::unbounded();
        let this = Self {
            id: BIT_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            bit: initial,
            set_rx,
            get_txs: vec![],
        };

        PBitHandle {
            handle: None,
            bit: Some(this),
            set_tx,
        }
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn spawn(initial: Bit) -> PBitHandle {
        let mut handle = Self::init(initial);
        handle.spawn();
        handle
    }

    fn subscribe(&mut self) -> Receiver<Bit> {
        let (tx, rx) = async_channel::unbounded();
        self.get_txs.push(tx);
        rx
    }

    fn spawn_internal(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.set_rx.try_recv() {
                    Ok(bit) => {
                        self.bit = bit;
                    }
                    // Ok(_) => {}
                    Err(e) if e.is_closed() => {
                        println!("PBit {:?} recv() Closed", self.id);
                        return;
                    }
                    _ => {}
                }

                for output_tx in self.get_txs.iter() {
                    match output_tx.try_send(self.bit) {
                        Ok(_) => {}
                        Err(e) if e.is_closed() => {
                            println!("PBit {:?} send() Closed", self.id);
                            return;
                        }
                        _ => {}
                    }
                }

                crate::yield_now().await;
            }
        })
    }
}

impl PBitHandle {
    pub fn set(&self, bit: Bit) {
        match self.set_tx.try_send(bit) {
            Ok(_) => {}
            Err(e) if e.is_closed() => println!("PBitHandle {:?} is closed", self.id()),
            _ => {}
        }
    }

    pub fn id(&self) -> Option<usize> {
        self.bit.as_ref().map(|b| b.id())
    }

    pub fn subscribe(&mut self) -> Receiver<Bit> {
        self.bit.as_mut().unwrap().subscribe()
    }

    pub fn spawn(&mut self) {
        let bit = std::mem::take(&mut self.bit).unwrap();
        self.handle = Some(bit.spawn_internal());
    }
}

impl Drop for PBitHandle {
    fn drop(&mut self) {
        // println!("PBitHandle {:?} is being dropped", self.bit);
    }
}
