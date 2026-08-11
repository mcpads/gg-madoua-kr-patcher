// VRAM tile management — VDP constants and tile types

pub const VDP_DATA_PORT: u8 = 0xBE;
pub const VDP_CTRL_PORT: u8 = 0xBF;
pub const V_COUNTER_PORT: u8 = 0x7E;
pub const H_COUNTER_PORT: u8 = 0x7F;
pub const PSG_PORT: u8 = 0x7F;

/// 4bpp tile (8×8 pixels, 32 bytes).
///
/// Layout: 8 rows × 4 bytes per row (bitplanes 0-3 interleaved).
/// Within each row, bit 7 = leftmost pixel, bit 0 = rightmost pixel.
/// Each pixel's color index (0-15) is assembled from the same bit position
/// across the four bitplane bytes: color = b0 | (b1<<1) | (b2<<2) | (b3<<3).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Tile4bpp {
    pub data: [u8; 32],
}

impl Tile4bpp {
    pub const SIZE: usize = 32;

    /// Address in VRAM for tile with given index (32 bytes per tile).
    pub fn tile_vram_addr(index: u16) -> u16 {
        index * 32
    }

    /// Get pixel color (0-15) at (x, y) where x=0 is leftmost pixel.
    pub fn get_pixel(&self, x: u8, y: u8) -> u8 {
        assert!(x < 8 && y < 8);
        let row_base = (y as usize) * 4;
        let bit = 7 - x;
        let b0 = (self.data[row_base] >> bit) & 1;
        let b1 = (self.data[row_base + 1] >> bit) & 1;
        let b2 = (self.data[row_base + 2] >> bit) & 1;
        let b3 = (self.data[row_base + 3] >> bit) & 1;
        b0 | (b1 << 1) | (b2 << 2) | (b3 << 3)
    }

    /// Set pixel color (0-15) at (x, y) where x=0 is leftmost pixel.
    pub fn set_pixel(&mut self, x: u8, y: u8, color: u8) {
        assert!(x < 8 && y < 8 && color < 16);
        let row_base = (y as usize) * 4;
        let bit = 7 - x;
        let mask = !(1u8 << bit);
        for plane in 0..4usize {
            let bit_val = (color >> plane) & 1;
            self.data[row_base + plane] = (self.data[row_base + plane] & mask) | (bit_val << bit);
        }
    }
}

#[cfg(test)]
#[path = "vram_tests.rs"]
mod tests;
