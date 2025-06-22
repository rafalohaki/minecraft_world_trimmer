#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompressionScheme {
    Gzip,
    Zlib,
    Lz4,
    Unknown(u8),
}

impl CompressionScheme {
    pub fn from_u8(byte: u8) -> Result<Self, &'static str> {
        match byte {
            1 => Ok(CompressionScheme::Gzip),
            2 => Ok(CompressionScheme::Zlib),
            4 => Ok(CompressionScheme::Lz4),
            _ => Ok(CompressionScheme::Unknown(byte)),
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            CompressionScheme::Gzip => 1,
            CompressionScheme::Zlib => 2,
            CompressionScheme::Lz4 => 4,
            CompressionScheme::Unknown(v) => *v,
        }
    }
}
