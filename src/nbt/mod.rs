pub mod binary_reader;
pub mod parse;
mod parsers;
pub mod tag;
mod writers;

#[cfg(test)]
pub use crate::nbt::parsers::parse_with_type::{MAX_NBT_DEPTH, NbtError};
