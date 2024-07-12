use std::fmt::{Debug, Display};

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bit(bool);

impl Bit {
    pub const HI: Self = Self(true);
    pub const LO: Self = Self(false);

    pub const fn as_u8(self) -> u8 {
        if self.0 {
            1
        } else {
            0
        }
    }

    pub fn as_bool(self) -> bool {
        self.0
    }

    pub fn as_bool_mut(&mut self) -> &mut bool {
        &mut self.0
    }

    pub const fn not(self) -> Self {
        Self(!self.0)
    }

    pub const fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn or(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn xor(self, other: Self) -> Self {
        Self(self.0 ^ other.0)
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
