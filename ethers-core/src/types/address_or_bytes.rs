use crate::types::{Address, Bytes};
use std::fmt::Debug;

#[derive(Clone, PartialEq, Eq)]
/// A type that can either be an `Address` or `Bytes`.
pub enum AddressOrBytes {
    /// An address type
    Address(Address),
    /// A bytes type
    Bytes(Bytes),
}

impl Debug for AddressOrBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressOrBytes::Address(addr) => write!(f, "{addr:?}"),
            AddressOrBytes::Bytes(bytes) => write!(f, "0x{}", hex::encode(bytes)),
        }
    }
}

impl From<Address> for AddressOrBytes {
    fn from(s: Address) -> Self {
        Self::Address(s)
    }
}

impl From<Bytes> for AddressOrBytes {
    fn from(s: Bytes) -> Self {
        Self::Bytes(s)
    }
}
