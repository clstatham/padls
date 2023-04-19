use std::{
    fmt::{Debug, Display},
    sync::Arc,
};

use derive_more::*;

use crate::op::Op;

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

    pub fn id(self) -> Op {
        Op {
            args: vec![],
            f: Arc::new(move |_| self),
        }
    }

    pub fn hi() -> Op {
        Self::HI.id()
    }

    pub fn lo() -> Op {
        Self::LO.id()
    }
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
