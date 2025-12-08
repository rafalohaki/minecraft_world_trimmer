use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parse::parse_tag;
use crate::nbt::tag::Tag;

pub fn parse_compound_tag(reader: &mut BinaryReader) -> Vec<Tag> {
    let mut values = Vec::new();

    loop {
        match parse_tag(reader) {
            Ok(next_tag) => {
                if next_tag == Tag::End {
                    break;
                }
                values.push(next_tag);
            }
            Err(_) => {
                // If we can't parse a tag, skip it and continue
                // This maintains graceful degradation
                break;
            }
        }
    }

    values
}
