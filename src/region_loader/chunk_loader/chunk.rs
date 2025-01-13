use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parse::parse_tag;
use crate::nbt::tag::Tag;
use crate::region_loader::chunk_loader::compression_scheme::CompressionScheme;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::read::{GzDecoder, ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use std::io::Read;

#[derive(PartialEq, Debug, Clone)]
pub struct Chunk {
    pub nbt: Tag,
    pub location: Location,
}

impl Chunk {
    const STATUS_FULL: &'static str = "minecraft:full";
    const BLOCK_STATES: &'static str = "block_states";
    const PALETTE: &'static str = "palette";
    const NAME: &'static str = "Name";

    pub fn from_location(buf: &[u8], location: Location) -> Result<Self, &'static str> {
        // Chunk header parsing
        // First get the chunk size in bytes
        let offset = location.get_offset() as usize;
        let chunk_size = get_u32(buf, offset) as usize;

        // Then get the compression scheme
        let compression_scheme_index = offset + 4;
        let compression_scheme = CompressionScheme::from_u8(buf[compression_scheme_index])?;

        // Get the raw chunk data
        let header_size = 5; // This can be a const
        let start = offset + header_size;
        let end = start + chunk_size - 1; // Remove 1 because the compression_scheme is included in the size
        let raw_first_chunk = &buf[start..end];

        // Depending on the compression scheme, read the data
        let decoded_bytes = match compression_scheme {
            CompressionScheme::Gzip => {
                let mut decoder = GzDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                decoder.read_to_end(&mut bytes).map(|_| bytes)
            }
            CompressionScheme::Zlib => {
                let mut decoder = ZlibDecoder::new(raw_first_chunk);
                let mut bytes = Vec::new();
                decoder.read_to_end(&mut bytes).map(|_| bytes)
            }
        };

        // Convert to string
        let nbt = decoded_bytes
            .map(|bytes| {
                let mut binary_reader = BinaryReader::new(&bytes);
                parse_tag(&mut binary_reader)
            })
            .map_err(|_| "Error while parsing NBT")?;

        Ok(Self { nbt, location })
    }

    pub fn to_bytes(&self, compression: Compression) -> Vec<u8> {
        let decoded_bytes = self.nbt.to_bytes();

        let mut encoder = ZlibEncoder::new(&decoded_bytes[..], compression);
        let mut bytes = Vec::new();
        if let Ok(encoded_bytes) = encoder.read_to_end(&mut bytes).map(|_| bytes) {
            self.to_bytes_compression_scheme(CompressionScheme::Zlib, &encoded_bytes)
        } else {
            self.to_bytes_compression_scheme(CompressionScheme::Zlib, &decoded_bytes)
        }
    }

    pub fn get_position(&self) -> Result<(i32, i32), &'static str> {
        let x_pos_tag = self.nbt.find_tag("xPos").and_then(|v| v.get_int());
        let z_pos_tag = self.nbt.find_tag("zPos").and_then(|v| v.get_int());

        match (x_pos_tag, z_pos_tag) {
            (Some(x), Some(z)) => Ok((*x, *z)),
            _ => Err("No position for this chunk"),
        }
    }

    /// Checks if a chunk is not fully generated or if it has never been inhabited
    pub fn should_delete(&self) -> bool {
        !self.is_fully_generated() || !self.has_been_inhabited()
    }

    fn is_fully_generated(&self) -> bool {
        self.nbt
            .find_tag("Status")
            .and_then(|tag| tag.get_string())
            .map(|status| status == Chunk::STATUS_FULL)
            .unwrap_or(false) // if the tag is not present, we can assume that the chunk is not fully generated
    }

    fn has_been_inhabited(&self) -> bool {
        // The InhabitedTime value seems to be incremented for all 8 chunks around a player (including the one the player is standing in)
        let inhabited_time = self
            .nbt
            .find_tag("InhabitedTime")
            .and_then(|tag| tag.get_long())
            .copied()
            .unwrap_or(0); // If the tag is not present, we can assume that the chunk has never been inhabited

        inhabited_time > 0
    }

    fn to_bytes_compression_scheme(
        &self,
        compression_scheme: CompressionScheme,
        nbt_bytes: &[u8],
    ) -> Vec<u8> {
        let size = (nbt_bytes.len() + 1/* including the compression scheme byte */) as u32;
        let mut result = Vec::from(size.to_be_bytes());
        result.push(compression_scheme.to_u8()); // adding the compression scheme byte
        result.extend_from_slice(nbt_bytes);
        result
    }

    pub fn count_block(&self, block_id: &str) -> u32 {
        let block_states = self.nbt.find_tag(Self::BLOCK_STATES);
        if let Some(Tag::Compound {
            value: block_states_value,
            ..
        }) = block_states
        {
            let palette = block_states_value
                .iter()
                .find(|tag| tag.get_name().as_deref() == Some(Self::PALETTE));
            if let Some(Tag::List {
                value: palette_value,
                ..
            }) = palette
            {
                return palette_value
                    .iter()
                    .filter(|palette_entry| {
                        if let Tag::Compound {
                            value: entry_value, ..
                        } = palette_entry
                        {
                            return entry_value.iter().any(|tag| {
                                tag.get_name().as_deref() == Some(Self::NAME)
                                    && tag.get_string().map(|s| s.as_str()) == Some(block_id)
                            });
                        }
                        false
                    })
                    .count() as u32;
            }
        }
        0
    }
}
