// region.rs
use crate::region_loader::chunk_loader::chunk::Chunk;
use crate::region_loader::get_u32::get_u32;
use crate::region_loader::location::Location;
use flate2::Compression;
use std::{fs::File, io::Read, path::PathBuf};
use thiserror::Error;

const LOCATION_TABLE_SIZE: usize = 4096;
const TIMESTAMP_TABLE_SIZE: usize = 4096;
const HEADER_SIZE: usize = LOCATION_TABLE_SIZE + TIMESTAMP_TABLE_SIZE;

#[derive(PartialEq, Debug)]
pub struct Region {
    chunks: Vec<Chunk>,
    is_modified: bool,
}

#[derive(Error, Debug)]
pub enum ParseRegionError {
    #[error("error while reading the file")]
    ReadError,
    #[error("cannot read header of region file")]
    HeaderError,
}

impl Region {
    pub fn from_file_name(file_name: &PathBuf) -> Result<Self, ParseRegionError> {
        let bytes = try_read_bytes(file_name).map_err(|_| ParseRegionError::ReadError)?;
        Region::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, ParseRegionError> {
        if bytes.len() < HEADER_SIZE {
            return Err(ParseRegionError::HeaderError);
        }

        let location_table = &bytes[0..LOCATION_TABLE_SIZE];
        let timestamp_table = &bytes[LOCATION_TABLE_SIZE..HEADER_SIZE];

        let mut chunks = Vec::with_capacity(1024);
        for i in (0..LOCATION_TABLE_SIZE).step_by(4) {
            let l = get_u32(location_table, i);
            let timestamp = get_u32(timestamp_table, i);
            let location = Location::from_bytes(l, timestamp);

            if location.is_valid() {
                if let Ok(chunk) = Chunk::from_location(bytes, location) {
                    chunks.push(chunk);
                }
                // Handle unsupported compression schemes here if needed.
            }
        }

        Ok(Self {
            chunks,
            is_modified: false,
        })
    }

    pub fn to_bytes(&self, compression: Compression) -> Vec<u8> {
        let mut data = Vec::new();
        let mut location_table = [0_u8; LOCATION_TABLE_SIZE];
        let mut timestamp_table = [0_u8; TIMESTAMP_TABLE_SIZE];

        for chunk in &self.chunks {
            // Serialize the chunk to bytes
            let mut serialized = chunk.to_bytes(compression);
            align_vec_size(&mut serialized);

            // Build the new location
            let new_position = (data.len() + HEADER_SIZE) as u32;
            let new_size = serialized.len() as u32;
            let original_timestamp = chunk.location.get_timestamp();
            let new_location = Location::new(new_position, new_size, original_timestamp)
                .expect("Location creation failed");

            if let Ok((x, z)) = chunk.get_position() {
                // Add the location to the header table
                let position_in_table = get_position_in_table(x, z);

                // Append to the location table
                let location_bytes = new_location.to_location_bytes();
                location_table[position_in_table..(4 + position_in_table)]
                    .copy_from_slice(&location_bytes);

                // Append to the timestamp table
                let timestamp_bytes = new_location.to_timestamp_bytes();
                timestamp_table[position_in_table..(4 + position_in_table)]
                    .copy_from_slice(&timestamp_bytes);
            }

            // Append serialized chunk data
            data.extend(serialized);
        }

        // Create final result vector with capacity for all data
        let mut result = Vec::with_capacity(data.len() + HEADER_SIZE);
        result.extend_from_slice(&location_table);
        result.extend_from_slice(&timestamp_table);
        result.extend(data);

        result
    }

    pub fn get_chunks(&self) -> &Vec<Chunk> {
        &self.chunks
    }

    pub fn get_chunk(&self, x: i32, z: i32) -> Option<&Chunk> {
        self.chunks.iter().find(|chunk| {
            if let Ok(position) = chunk.get_position() {
                position.0 == x && position.1 == z
            } else {
                false
            }
        })
    }

    pub fn get_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn remove_chunk_by_index(&mut self, index: usize) {
        self.chunks.remove(index);
        self.mark_as_modified(); // Mark as modified when a chunk is removed
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn is_modified(&self) -> bool {
        self.is_modified
    }

    pub fn mark_as_modified(&mut self) {
        self.is_modified = true;
    }
}

fn align_vec_size(vec: &mut Vec<u8>) {
    let aligned_size = ((vec.len() + 4095) / 4096) * 4096;
    vec.resize(aligned_size, 0);
}

fn get_position_in_table(x: i32, z: i32) -> usize {
    (4 * ((x & 31) + (z & 31) * 32)) as usize
}

fn try_read_bytes(file_path: &PathBuf) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::<u8>::new();
    File::open(file_path).and_then(|mut file| file.read_to_end(&mut buf))?;
    Ok(buf)
}
