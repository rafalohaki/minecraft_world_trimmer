use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parsers::parse_with_type::{NbtError, parse_with_type_depth};
use crate::nbt::tag::Tag;

/// Parses compound children until the `End` byte. Any read or sub-parse
/// error propagates: a truncated/corrupt payload must NEVER yield a
/// partially-built compound (a partial tree would re-serialize shorter than
/// the original, silently dropping the chunk's tail on rewrite).
pub fn parse_compound_tag(
    reader: &mut BinaryReader,
    parent_depth: u32,
) -> Result<Vec<Tag>, NbtError> {
    let mut values = Vec::new();
    let depth = parent_depth + 1;

    loop {
        // The child type byte is read here so a clean End can be told apart
        // from an EOF in the middle of the payload.
        match reader.read_type() {
            Ok(0) => return Ok(values),
            Ok(child_type) => values.push(parse_with_type_depth(reader, child_type, false, depth)?),
            // EOF exactly at buffer end = legacy file missing its final End
            // byte; tolerated as clean termination. Any other EOF position is
            // corruption and must fail.
            Err(_) if reader.at_clean_end() => return Ok(values),
            Err(e) => return Err(e.into()),
        }
    }
}

