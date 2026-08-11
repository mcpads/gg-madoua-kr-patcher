// TMR SEGA header parsing

pub const HEADER_OFFSET: usize = 0x7FF0;
pub const HEADER_MAGIC: &[u8; 8] = b"TMR SEGA";

/// The 16-byte TMR SEGA header located at ROM offset 0x7FF0.
///
/// Layout (offsets relative to 0x7FF0):
///   [0..8]   magic "TMR SEGA"
///   [8..10]  reserved (padding, usually 0xFF 0xFF)
///   [10..12] checksum (LE u16)
///   [12..15] product code (BCD encoded, 5 nibbles + version nibble)
///   [15]     upper nibble = region code, lower nibble = rom size code
pub struct TmrSegaHeader {
    pub magic: [u8; 8],
    pub reserved: [u8; 2],
    pub checksum: u16,
    /// 20-bit product code stored in BCD across bytes 12-14 of the header.
    /// Stored here as the raw 3 bytes for simplicity.
    pub product_code: u32,
    /// 4-bit version (lower nibble of byte 14 of header)
    pub version: u8,
    /// 4-bit region code (upper nibble of byte 15 of header)
    pub region: u8,
    /// 4-bit ROM size code (lower nibble of byte 15 of header)
    pub rom_size: u8,
}

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("ROM too small: need at least {required} bytes, got {got}")]
    TooSmall { required: usize, got: usize },
    #[error("invalid magic: expected 'TMR SEGA', got {got:?}")]
    InvalidMagic { got: [u8; 8] },
}

impl TmrSegaHeader {
    pub fn parse(rom: &[u8]) -> Result<Self, HeaderError> {
        let required = HEADER_OFFSET + 16;
        if rom.len() < required {
            return Err(HeaderError::TooSmall {
                required,
                got: rom.len(),
            });
        }

        let hdr = &rom[HEADER_OFFSET..HEADER_OFFSET + 16];

        let mut magic = [0u8; 8];
        magic.copy_from_slice(&hdr[0..8]);
        if &magic != HEADER_MAGIC {
            return Err(HeaderError::InvalidMagic { got: magic });
        }

        let mut reserved = [0u8; 2];
        reserved.copy_from_slice(&hdr[8..10]);

        let checksum = u16::from_le_bytes([hdr[10], hdr[11]]);

        // Product code: BCD across bytes 12-14
        // bytes 12-13 hold 4 BCD digits; byte 14 upper nibble = 5th BCD digit
        // byte 14 lower nibble = version
        let product_code =
            ((hdr[14] as u32 & 0xF0) >> 4) << 16 | (hdr[13] as u32) << 8 | (hdr[12] as u32);
        let version = hdr[14] & 0x0F;
        let region = (hdr[15] & 0xF0) >> 4;
        let rom_size = hdr[15] & 0x0F;

        Ok(TmrSegaHeader {
            magic,
            reserved,
            checksum,
            product_code,
            version,
            region,
            rom_size,
        })
    }

    /// Compute checksum: sum of bytes 0x0000 through 0x7FEF (right before the header),
    /// truncated to 16 bits.
    pub fn compute_checksum(rom: &[u8]) -> u16 {
        let end = HEADER_OFFSET.min(rom.len());
        let sum: u32 = rom[..end].iter().map(|&b| b as u32).sum();
        sum as u16
    }

    /// Update the checksum field in ROM data at offset 0x7FFA-0x7FFB.
    pub fn update_checksum(rom: &mut [u8]) {
        let checksum = Self::compute_checksum(rom);
        let [lo, hi] = checksum.to_le_bytes();
        rom[HEADER_OFFSET + 10] = lo;
        rom[HEADER_OFFSET + 11] = hi;
    }
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
