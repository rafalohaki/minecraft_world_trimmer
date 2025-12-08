use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::binary_reader::ReaderError;
use crate::nbt::parsers::parse_compound_tag::parse_compound_tag;
use crate::nbt::parsers::parse_list_tag::parse_list_tag;
use crate::nbt::tag::Tag;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NbtError {
    #[error("Unsupported NBT tag type: {0}")]
    UnsupportedTag(u8),
    #[error("Reader error: {0}")]
    ReaderError(#[from] ReaderError),
}

pub fn parse_with_type(reader: &mut BinaryReader, tag_type: u8, skip_name: bool) -> Result<Tag, NbtError> {
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
        7 => {
            let value = reader.read_byte_array();
            Ok(Tag::ByteArray { name, value })
        }
        8 => {
            let value = reader.read_string()?;
            Ok(Tag::String { name, value })
        }
        9 => {
            let (tag_type, value) = parse_list_tag(reader);
            Ok(Tag::List {
                name,
                value,
                tag_type,
            })
        }
        10 => {
            let value = parse_compound_tag(reader);
            Ok(Tag::Compound { name, value })
        }
        11 => {
            let value = reader.read_int_array();
            Ok(Tag::IntArray { name, value })
        }
        12 => {
            let value = reader.read_long_array();
            Ok(Tag::LongArray { name, value })
        }
        _ => Err(NbtError::UnsupportedTag(tag_type)),
    }
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
