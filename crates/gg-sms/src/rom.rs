//! ROM loading and access with collision detection
//!
//! `TrackedRom`은 `Deref<[u8]>`만 구현하고 `DerefMut`는 없다. 라벨없는 직접쓰기는 컴파일 불가:
//!
//! ```compile_fail
//! use gg_sms::rom::TrackedRom;
//! let mut rom = TrackedRom::new(vec![0u8; 16]);
//! rom[0] = 1; // DerefMut 미구현 — 컴파일 실패해야 정상
//! ```

#[derive(Debug)]
pub struct WriteOp {
    pub label: String,
    pub offset: usize,
    pub len: usize,
}

/// ROM 쓰기 사전조건.
#[derive(Clone, Debug)]
pub enum Expect<'a> {
    /// 대상 영역이 모두 지정된 바이트(보통 0xFF)여야 함
    FreeSpace(u8),
    /// 대상 영역의 원본 바이트가 정확히 일치해야 함
    Bytes(&'a [u8]),
}

pub struct TrackedRom {
    data: Vec<u8>,
    writes: Vec<WriteOp>,
}

#[derive(Debug, thiserror::Error)]
pub enum TrackedRomError {
    #[error(
        "write collision: '{new_label}' at {new_offset:#X}+{new_len} overlaps '{existing_label}' at {existing_offset:#X}+{existing_len}"
    )]
    Collision {
        new_label: String,
        new_offset: usize,
        new_len: usize,
        existing_label: String,
        existing_offset: usize,
        existing_len: usize,
    },
    #[error("write out of bounds: offset {offset:#X} + len {len} exceeds ROM size {rom_size}")]
    OutOfBounds {
        offset: usize,
        len: usize,
        rom_size: usize,
    },
    #[error("[{label}] expectation failed at {offset:#X}: {detail}")]
    Expectation {
        label: String,
        offset: usize,
        detail: String,
    },
}

impl TrackedRom {
    pub fn new(data: Vec<u8>) -> Self {
        TrackedRom {
            data,
            writes: Vec::new(),
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn read(&self, offset: usize, len: usize) -> Option<&[u8]> {
        self.data.get(offset..offset.checked_add(len)?)
    }

    /// Write data at offset with a label. Errors if region overlaps a previous write.
    pub fn write(
        &mut self,
        label: &str,
        offset: usize,
        data: &[u8],
    ) -> Result<(), TrackedRomError> {
        let len = data.len();
        // bounds check
        if offset + len > self.data.len() {
            return Err(TrackedRomError::OutOfBounds {
                offset,
                len,
                rom_size: self.data.len(),
            });
        }
        // collision check
        self.check_collision(offset, len).map_err(|e| match e {
            TrackedRomError::Collision {
                existing_label,
                existing_offset,
                existing_len,
                ..
            } => TrackedRomError::Collision {
                new_label: label.to_string(),
                new_offset: offset,
                new_len: len,
                existing_label,
                existing_offset,
                existing_len,
            },
            other => other,
        })?;
        // perform write
        self.data[offset..offset + len].copy_from_slice(data);
        self.writes.push(WriteOp {
            label: label.to_string(),
            offset,
            len,
        });
        Ok(())
    }

    /// Get all write operations for audit
    pub fn write_reports(&self) -> &[WriteOp] {
        &self.writes
    }

    /// Check if writing at offset+len would collide with existing writes
    fn check_collision(&self, offset: usize, len: usize) -> Result<(), TrackedRomError> {
        let new_end = offset + len;
        for op in &self.writes {
            let existing_end = op.offset + op.len;
            // overlap if ranges intersect: not (new_end <= op.offset || offset >= existing_end)
            if !(new_end <= op.offset || offset >= existing_end) {
                return Err(TrackedRomError::Collision {
                    new_label: String::new(), // will be filled in by write()
                    new_offset: offset,
                    new_len: len,
                    existing_label: op.label.clone(),
                    existing_offset: op.offset,
                    existing_len: op.len,
                });
            }
        }
        Ok(())
    }

    /// Consume and return the ROM data
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    /// Write at bank:offset. physical = bank*BANK_SIZE + off_in_bank.
    pub fn write_bank(
        &mut self,
        label: &str,
        bank: u8,
        off_in_bank: usize,
        data: &[u8],
    ) -> Result<(), TrackedRomError> {
        let pc = bank as usize * crate::mapper::BANK_SIZE + off_in_bank;
        self.write(label, pc, data)
    }

    /// Verify a precondition, then write. Errors (not panic) on mismatch.
    pub fn write_expect(
        &mut self,
        label: &str,
        offset: usize,
        data: &[u8],
        expect: &Expect,
    ) -> Result<(), TrackedRomError> {
        let len = data.len();
        let end = offset + len;
        if end > self.data.len() {
            return Err(TrackedRomError::OutOfBounds {
                offset,
                len,
                rom_size: self.data.len(),
            });
        }
        match expect {
            Expect::FreeSpace(fill) => {
                if !self.data[offset..end].iter().all(|&b| b == *fill) {
                    return Err(TrackedRomError::Expectation {
                        label: label.to_string(),
                        offset,
                        detail: format!(
                            "expected free space (0x{fill:02X}), found {:02X?}",
                            &self.data[offset..end.min(offset + 16)]
                        ),
                    });
                }
            }
            Expect::Bytes(expected) => {
                if expected.len() != len {
                    return Err(TrackedRomError::Expectation {
                        label: label.to_string(),
                        offset,
                        detail: format!(
                            "Expect::Bytes length ({}) must equal write length ({len})",
                            expected.len()
                        ),
                    });
                }
                if &self.data[offset..offset + len] != *expected {
                    return Err(TrackedRomError::Expectation {
                        label: label.to_string(),
                        offset,
                        detail: format!(
                            "expected bytes {:02X?}, found {:02X?}",
                            expected,
                            &self.data[offset..offset + len]
                        ),
                    });
                }
            }
        }
        self.write(label, offset, data)
    }

    /// Compare `original` vs current data; report changed bytes outside any
    /// registered write region. Enforces: no untracked writes.
    pub fn check_untracked_writes(&self, original: &[u8]) -> Result<(), String> {
        let len = original.len().min(self.data.len());
        let mut covered: Vec<(usize, usize)> = self
            .writes
            .iter()
            .map(|w| (w.offset, w.offset + w.len))
            .collect();
        covered.sort_by_key(|&(s, _)| s);
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (s, e) in covered {
            if let Some(last) = merged.last_mut() {
                if s <= last.1 {
                    last.1 = last.1.max(e);
                    continue;
                }
            }
            merged.push((s, e));
        }
        let is_covered = |pc: usize| -> bool {
            merged
                .binary_search_by(|&(s, e)| {
                    if pc < s {
                        std::cmp::Ordering::Greater
                    } else if pc >= e {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .is_ok()
        };
        let mut untracked: Vec<(usize, usize)> = Vec::new();
        let mut run_start: Option<usize> = None;
        for i in 0..len {
            if original[i] != self.data[i] {
                if !is_covered(i) {
                    if run_start.is_none() {
                        run_start = Some(i);
                    }
                } else if let Some(start) = run_start.take() {
                    untracked.push((start, i));
                }
            } else if let Some(start) = run_start.take() {
                untracked.push((start, i));
            }
        }
        if let Some(start) = run_start {
            untracked.push((start, len));
        }
        if untracked.is_empty() {
            return Ok(());
        }
        let total: usize = untracked.iter().map(|(s, e)| e - s).sum();
        let mut report = format!(
            "Untracked ROM writes detected ({} region(s), {} bytes total):\n",
            untracked.len(),
            total
        );
        for (s, e) in &untracked {
            report.push_str(&format!(
                "  UNTRACKED: [0x{:06X}..0x{:06X}) ({} bytes)\n",
                s,
                e,
                e - s
            ));
        }
        Err(report)
    }
}

impl std::ops::Deref for TrackedRom {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.data
    }
}
// NOTE: DerefMut is intentionally NOT implemented — label-less writes
// (`rom[x] = y`) must not compile. All writes go through write*/fill.

#[cfg(test)]
#[path = "rom_tests.rs"]
mod tests;
