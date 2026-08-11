// SEGA mapper (bank switching)

pub const MAPPER_CTRL: u16 = 0xFFFC;
pub const SLOT0_BANK: u16 = 0xFFFD;
pub const SLOT1_BANK: u16 = 0xFFFE;
pub const SLOT2_BANK: u16 = 0xFFFF;
pub const BANK_SIZE: usize = 0x4000; // 16KB

pub struct SegaMapper;

impl SegaMapper {
    /// Logical Z80 address + slot bank numbers → physical ROM offset.
    /// Slot 0 ($0000-$3FFF) is always bank 0.
    /// Slot 1 ($4000-$7FFF) maps to `slot1_bank`.
    /// Slot 2 ($8000-$BFFF) maps to `slot2_bank`.
    /// $C000-$FFFF is RAM — return the offset as-is with bank 0 (not ROM).
    pub fn logical_to_physical(addr: u16, slot1_bank: u8, slot2_bank: u8) -> usize {
        let addr = addr as usize;
        match addr {
            0x0000..=0x3FFF => addr, // slot 0: fixed bank 0
            0x4000..=0x7FFF => (slot1_bank as usize) * BANK_SIZE + (addr - 0x4000), // slot 1
            0x8000..=0xBFFF => (slot2_bank as usize) * BANK_SIZE + (addr - 0x8000), // slot 2
            _ => addr,               // $C000-$FFFF: RAM region, return as-is
        }
    }

    /// Physical ROM offset → (bank number, offset within bank)
    pub fn physical_to_bank(offset: usize) -> (u8, u16) {
        let bank = (offset / BANK_SIZE) as u8;
        let bank_offset = (offset % BANK_SIZE) as u16;
        (bank, bank_offset)
    }

    /// Bank number → physical ROM offset range
    pub fn bank_range(bank: u8) -> std::ops::Range<usize> {
        let start = (bank as usize) * BANK_SIZE;
        start..start + BANK_SIZE
    }
}

#[cfg(test)]
#[path = "mapper_tests.rs"]
mod tests;
