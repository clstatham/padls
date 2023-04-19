use std::sync::Arc;

use async_recursion::async_recursion;

use crate::bit::Bit;

#[derive(Clone)]
pub struct Op {
    pub args: Vec<Arc<Op>>,
    pub f: Arc<dyn Fn(Vec<Bit>) -> Bit + Send + Sync>,
}
impl Op {
    pub fn new(f: Arc<dyn Fn(Vec<Bit>) -> Bit + Send + Sync>) -> Self {
        Self {
            args: Vec::new(),
            f,
        }
    }

    #[async_recursion]
    pub async fn eval(self: Arc<Self>) -> Bit {
        let mut js = tokio::task::JoinSet::new();
        let n_args = self.args.len();
        for arg in self.args.iter() {
            js.spawn(arg.clone().eval());
        }
        let mut args = Vec::with_capacity(n_args);
        for _ in 0..n_args {
            args.push(*js.join_next().await.as_ref().unwrap().as_ref().unwrap());
        }
        (self.f)(args)
    }

    pub fn connect_input(&mut self, inp: Arc<Op>) {
        self.args.push(inp);
    }
}

impl std::ops::Not for Op {
    type Output = Self;
    fn not(self) -> Self {
        Self {
            args: vec![Arc::new(self)],
            f: Arc::new(move |args| !args[0]),
        }
    }
}

impl std::ops::BitAnd<Op> for Op {
    type Output = Self;
    fn bitand(self, rhs: Op) -> Self::Output {
        Self {
            args: vec![Arc::new(self), Arc::new(rhs)],
            f: Arc::new(move |args| args[0] & args[1]),
        }
    }
}

impl std::ops::BitOr<Op> for Op {
    type Output = Self;
    fn bitor(self, rhs: Op) -> Self::Output {
        Self {
            args: vec![Arc::new(self), Arc::new(rhs)],
            f: Arc::new(move |args| args[0] | args[1]),
        }
    }
}

impl std::ops::BitXor<Op> for Op {
    type Output = Self;
    fn bitxor(self, rhs: Op) -> Self::Output {
        Self {
            args: vec![Arc::new(self), Arc::new(rhs)],
            f: Arc::new(move |args| args[0] ^ args[1]),
        }
    }
}
