pub fn get_u32(table: &[u8], index: usize) -> u32 {
    debug_assert!(index + 4 <= table.len(), "get_u32: index {index} out of bounds (len={})", table.len());
    u32::from_be_bytes(table[index..index + 4].try_into().unwrap())
}
