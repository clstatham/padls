use std::{
    cell::RefCell,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Context,
    thread::{self, JoinHandle},
};

use crossbeam::channel::{Receiver as Rx, Sender as Tx};

use futures::task::ArcWake;
use futures_util::Future;

use crate::bit::Bit;

pub struct BitFuture {
    future: Mutex<Pin<Box<dyn Future<Output = Bit> + Send + 'static>>>,
    executor: Tx<Arc<BitFuture>>,
}

thread_local! {
    static CURRENT: RefCell<Option<Tx<Arc<BitFuture>>>> = RefCell::new(None);
}

pub fn thread<F>(future: F) -> JoinHandle<()>
where
    F: Future<Output = Bit> + Send + 'static,
{
    let mut bt = BitThread::new();
    thread::spawn(move || bt.spawn(future))
}

impl BitFuture {
    fn schedule(self: &Arc<Self>) {
        let _ = self.executor.send(self.clone());
    }

    fn poll(self: Arc<Self>) {
        let waker = futures::task::waker(self.clone());
        let mut cx = Context::from_waker(&waker);
        let mut future = self.future.try_lock().unwrap();
        let _ = future.as_mut().poll(&mut cx);
    }

    fn spawn<F>(future: F, sender: &Tx<Arc<BitFuture>>)
    where
        F: Future<Output = Bit> + Send + 'static,
    {
        let task = Arc::new(BitFuture {
            future: Mutex::new(Box::pin(future)),
            executor: sender.clone(),
        });

        let _ = sender.send(task);
    }
}

impl ArcWake for BitFuture {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.schedule();
    }
}

pub struct BitThread {
    scheduled: Rx<Arc<BitFuture>>,
    sender: Tx<Arc<BitFuture>>,
}

impl BitThread {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam::channel::unbounded();
        Self {
            scheduled: rx,
            sender: tx,
        }
    }

    pub fn spawn<F>(&mut self, future: F)
    where
        F: Future<Output = Bit> + Send + 'static,
    {
        BitFuture::spawn(future, &self.sender);
    }

    pub fn run(&mut self) {
        CURRENT.with(|cell| {
            *cell.borrow_mut() = Some(self.sender.clone());
        });
        while let Ok(task) = self.scheduled.recv() {
            task.poll();
        }
    }
}

impl Default for BitThread {
    fn default() -> Self {
        Self::new()
    }
}
