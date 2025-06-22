use crate::nbt::binary_reader::BinaryReader;
use crate::nbt::parse::parse_tag;
use crate::nbt::tag::Tag;
use crate::region_loader::chunk_loader::compression_scheme::CompressionScheme;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::read::{GzDecoder, ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use lz4_flex::{block::compress_prepend_size as lz4_compress, block::decompress_size_prepended as lz4_decompress};
use std::io::Read;

#[derive(PartialEq, Debug, Clone)]
pub struct Chunk {
    pub nbt: Option<Tag>,
    pub location: Location,
    pub compression_scheme: CompressionScheme,
    raw_bytes: Option<Vec<u8>>, // Used when the compression scheme is unsupported
}

impl Chunk {
    const STATUS_FULL: &'static str = "minecraft:full";

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
            CompressionScheme::Lz4 => {
                lz4_decompress(raw_first_chunk)
                    .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "lz4 error"))
            }
            CompressionScheme::Unknown(_) => Ok(raw_first_chunk.to_vec()),
        };

        match (decoded_bytes, compression_scheme) {
            (Ok(bytes), CompressionScheme::Unknown(_)) => Ok(Self { nbt: None, location, compression_scheme, raw_bytes: Some(bytes) }),
            (Ok(bytes), scheme) => {
                let mut binary_reader = BinaryReader::new(&bytes);
                let nbt = parse_tag(&mut binary_reader);
                Ok(Self { nbt: Some(nbt), location, compression_scheme: scheme, raw_bytes: None })
            }
            (Err(_), _) => Err("Error while parsing chunk"),
        }
    }

    pub fn to_bytes(&self, compression: Compression) -> Vec<u8> {
        match (&self.nbt, &self.raw_bytes, self.compression_scheme) {
            (Some(nbt), _, CompressionScheme::Gzip) => {
                let decoded_bytes = nbt.to_bytes();
                let mut encoder = flate2::read::GzEncoder::new(&decoded_bytes[..], compression);
                let mut bytes = Vec::new();
                if let Ok(encoded_bytes) = encoder.read_to_end(&mut bytes).map(|_| bytes) {
                    self.to_bytes_compression_scheme(CompressionScheme::Gzip, &encoded_bytes)
                } else {
                    self.to_bytes_compression_scheme(CompressionScheme::Gzip, &decoded_bytes)
                }
            }
            (Some(nbt), _, CompressionScheme::Zlib) => {
                let decoded_bytes = nbt.to_bytes();
                let mut encoder = ZlibEncoder::new(&decoded_bytes[..], compression);
                let mut bytes = Vec::new();
                if let Ok(encoded_bytes) = encoder.read_to_end(&mut bytes).map(|_| bytes) {
                    self.to_bytes_compression_scheme(CompressionScheme::Zlib, &encoded_bytes)
                } else {
                    self.to_bytes_compression_scheme(CompressionScheme::Zlib, &decoded_bytes)
                }
            }
            (Some(nbt), _, CompressionScheme::Lz4) => {
                let decoded_bytes = nbt.to_bytes();
                let encoded = lz4_compress(&decoded_bytes);
                self.to_bytes_compression_scheme(CompressionScheme::Lz4, &encoded)
            }
            (None, Some(raw), scheme) => self.to_bytes_compression_scheme(scheme, raw),
            _ => Vec::new(),
        }
    }

    pub fn get_position(&self) -> Result<(i32, i32), &'static str> {
        let nbt = self.nbt.as_ref().ok_or("No position for this chunk")?;
        let x_pos_tag = nbt.find_tag("xPos").and_then(|v| v.get_int());
        let z_pos_tag = nbt.find_tag("zPos").and_then(|v| v.get_int());

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
        self
            .nbt
            .as_ref()
            .and_then(|nbt| nbt.find_tag("Status"))
            .and_then(|tag| tag.get_string())
            .map(|status| status == Chunk::STATUS_FULL)
            .unwrap_or(false)
    }

    fn has_been_inhabited(&self) -> bool {
        // The InhabitedTime value seems to be incremented for all 8 chunks around a player (including the one the player is standing in)
        let inhabited_time = self
            .nbt
            .as_ref()
            .and_then(|nbt| nbt.find_tag("InhabitedTime"))
            .and_then(|tag| tag.get_long())
            .copied()
            .unwrap_or(0);

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
}
