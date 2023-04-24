use std::{
    fmt::{Debug, Display},
    sync::atomic::AtomicUsize,
};

use derive_more::*;
use tokio::{
    sync::broadcast::{error::RecvError, Receiver, Sender},
    task::JoinHandle,
};

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
    get_tx: Sender<Bit>,
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
    _get_rx: Receiver<Bit>,
}

impl PBit {
    pub fn init(initial: Bit) -> PBitHandle {
        let (set_tx, set_rx) = tokio::sync::broadcast::channel(1024);
        let (get_tx, _get_rx) = tokio::sync::broadcast::channel(1024);
        let this = Self {
            id: BIT_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            bit: initial,
            set_rx,
            get_tx,
        };

        PBitHandle {
            handle: None,
            bit: Some(this),
            set_tx,
            _get_rx,
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

    fn subscribe(&self) -> Receiver<Bit> {
        self.get_tx.subscribe()
    }

    fn spawn_internal(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.set_rx.recv().await {
                    Ok(bit) => {
                        self.bit = bit;
                        crate::beat().await;
                        match self.get_tx.send(self.bit) {
                            Ok(_) => {}
                            Err(_) => {
                                println!("PBit {:?} send() Closed", self.id);
                                return;
                            }
                        }
                    }
                    Err(RecvError::Lagged(ref lag)) => {
                        println!("PBit {:?} lagged by {}", self.id, lag);
                    }
                    Err(e) if e == RecvError::Closed => {
                        println!("PBit {:?} recv() Closed", self.id);
                        return;
                    }
                    _ => {}
                }

                crate::yield_now().await;
            }
        })
    }
}

impl PBitHandle {
    pub fn set(&self, bit: Bit) {
        match self.set_tx.send(bit) {
            Ok(_) => {}
            Err(_) => println!("PBitHandle {:?} is closed", self.id()),
        }
    }

    pub fn id(&self) -> Option<usize> {
        self.bit.as_ref().map(|b| b.id())
    }

    pub fn subscribe(&self) -> Receiver<Bit> {
        self.bit.as_ref().unwrap().subscribe()
    }

    pub fn spawn(&mut self) {
        let bit = self.bit.take().unwrap();
        self.handle = Some(bit.spawn_internal());
    }
}

impl Drop for PBitHandle {
    fn drop(&mut self) {
        // println!("PBitHandle {:?} is being dropped", self.bit);
    }
}
