use crate::bit::Bit;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Gate {
    Identity,
    Not,
    And,
    Or,
    Xor,
}

impl Gate {
    pub const fn arity(&self) -> usize {
        match self {
            Self::Identity | Self::Not => 1,
            Self::And | Self::Or | Self::Xor => 2,
        }
    }

    pub const fn eval(&self, inputs: &[Bit]) -> Bit {
        match self {
            Self::Identity => inputs[0],
            Self::Not => inputs[0].not(),
            Self::And => inputs[0].and(inputs[1]),
            Self::Or => inputs[0].or(inputs[1]),
            Self::Xor => inputs[0].xor(inputs[1]),
        }
    }
}
