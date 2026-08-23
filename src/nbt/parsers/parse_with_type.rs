use crate::nbt::binary_reader::{BinaryReader, ReaderError};
use crate::nbt::parsers::parse_compound_tag::parse_compound_tag;
use crate::nbt::tag::Tag;
use thiserror::Error;


/// Maximum NBT nesting depth. Vanilla chunk data stays far below this
/// (deepest observed structures are ~10 levels); the cap only exists so a
/// crafted or corrupt payload cannot drive the recursive parser into a stack
/// overflow. Reference implementations guard similarly (fastnbt bounds
/// sequence sizes; its streaming parser is iterative).
pub const MAX_NBT_DEPTH: u32 = 128;

#[derive(Error, Debug)]
pub enum NbtError {
    #[error("Unsupported NBT tag type: {0}")]
    UnsupportedTag(u8),
    #[error("NBT nesting deeper than {MAX_NBT_DEPTH}")]
    DepthLimit,
    #[error("Reader error: {0}")]
    ReaderError(#[from] ReaderError),
}

/// Parses a single tag of `tag_type` (name included unless `skip_name`).
pub fn parse_with_type(
    reader: &mut BinaryReader,
    tag_type: u8,
    skip_name: bool,
) -> Result<Tag, NbtError> {
    parse_with_type_depth(reader, tag_type, skip_name, 0)
}

pub fn parse_with_type_depth(
    reader: &mut BinaryReader,
    tag_type: u8,
    skip_name: bool,
    depth: u32,
) -> Result<Tag, NbtError> {
    if depth > MAX_NBT_DEPTH {
        return Err(NbtError::DepthLimit);
    }

    let name = if skip_name || tag_type == 0 {
        None
    } else {
        reader.read_name()
    };

    match tag_type {
        0 => Ok(Tag::End),
        1 => {
            let value = reader.read_i8()?;
            Ok(Tag::Byte { name, value })
        }
        2 => {
            let value = reader.read_i16()?;
            Ok(Tag::Short { name, value })
        }
        3 => {
            let value = reader.read_i32()?;
            Ok(Tag::Int { name, value })
        }
        4 => {
            let value = reader.read_i64()?;
            Ok(Tag::Long { name, value })
        }
        5 => {
            let value = reader.read_f32()?;
            Ok(Tag::Float { name, value })
        }
        6 => {
            let value = reader.read_f64()?;
            Ok(Tag::Double { name, value })
        }
        7 => Ok(Tag::ByteArray {
            name,
            value: reader.read_byte_array()?,
        }),
        8 => {
            let value = reader.read_string()?;
            Ok(Tag::String { name, value })
        }
        9 => {
            let (list_elem_type, value) =
                parse_list_nested(reader, depth)?;
            Ok(Tag::List {
                name,
                value,
                tag_type: list_elem_type,
            })
        }
        10 => {
            let value = parse_compound_tag(reader, depth)?;
            Ok(Tag::Compound { name, value })
        }
        11 => Ok(Tag::IntArray {
            name,
            value: reader.read_int_array()?,
        }),
        12 => Ok(Tag::LongArray {
            name,
            value: reader.read_long_array()?,
        }),
        _ => Err(NbtError::UnsupportedTag(tag_type)),
    }
}

fn parse_list_nested(
    reader: &mut BinaryReader,
    parent_depth: u32,
) -> Result<(u8, Vec<Tag>), NbtError> {
    let mut values = Vec::new();

    let elem_type = reader.read_type()?;
    let list_length = reader.read_i32()?;
    if list_length < 0 || (list_length > 0 && elem_type == 0) {
        // Negative length, or a non-empty list of End tags: malformed.
        return Err(NbtError::UnsupportedTag(elem_type));
    }

    for _ in 0..list_length {
        values.push(parse_with_type_depth(
            reader,
            elem_type,
            true,
            parent_depth + 1,
        )?);
    }

    Ok((elem_type, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::binary_reader::BinaryReader;

    #[test]
    fn test_unsupported_nbt_tag() {
        let data = [15]; // Non-existent NBT tag type
        let mut reader = BinaryReader::new(&data);
        let result = parse_with_type(&mut reader, 15, true);
        assert!(result.is_err());
        match result.unwrap_err() {
            NbtError::UnsupportedTag(tag) => assert_eq!(tag, 15),
            _ => panic!("Expected UnsupportedTag error"),
        }
    }

    #[test]
    fn test_unsupported_nbt_tag_with_message() {
        let data = [99]; // Another non-existent tag
        let mut reader = BinaryReader::new(&data);
        let result = parse_with_type(&mut reader, 99, true);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(matches!(error, NbtError::UnsupportedTag(99)));
    }
}

