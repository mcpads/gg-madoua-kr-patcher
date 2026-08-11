//! Minimal BPS patch writer.
//!
//! The writer intentionally emits only SourceRead and TargetRead actions. This is
//! less compact than a full delta optimizer, but it is deterministic, small, and
//! enough for source-verified cartridge patch distribution.

use anyhow::{Context, Result};
use std::path::Path;

const SOURCE_READ: u64 = 0;
const TARGET_READ: u64 = 1;
const MIN_SOURCE_READ_RUN: usize = 4;

pub fn write_bps(source: &[u8], target: &[u8], output_path: &Path) -> Result<()> {
    let patch = create_bps(source, target, &[])?;
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(output_path, patch)
        .with_context(|| format!("BPS 출력 쓰기 실패: {}", output_path.display()))
}

pub fn create_bps(source: &[u8], target: &[u8], metadata: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        source.len() <= u64::MAX as usize && target.len() <= u64::MAX as usize,
        "BPS size exceeds supported u64 range"
    );
    let mut out = Vec::new();
    out.extend_from_slice(b"BPS1");
    encode_number(source.len() as u64, &mut out);
    encode_number(target.len() as u64, &mut out);
    encode_number(metadata.len() as u64, &mut out);
    out.extend_from_slice(metadata);

    let mut offset = 0usize;
    while offset < target.len() {
        let same_len = source_read_len(source, target, offset);
        if same_len >= MIN_SOURCE_READ_RUN {
            emit_action(SOURCE_READ, same_len, &mut out)?;
            offset += same_len;
            continue;
        }

        let start = offset;
        offset += same_len.max(1);
        while offset < target.len() {
            let run = source_read_len(source, target, offset);
            if run >= MIN_SOURCE_READ_RUN {
                break;
            }
            offset += run.max(1);
        }
        emit_action(TARGET_READ, offset - start, &mut out)?;
        out.extend_from_slice(&target[start..offset]);
    }

    append_u32_le(&mut out, crc32fast::hash(source));
    append_u32_le(&mut out, crc32fast::hash(target));
    let patch_crc = crc32fast::hash(&out);
    append_u32_le(&mut out, patch_crc);
    Ok(out)
}

fn source_read_len(source: &[u8], target: &[u8], offset: usize) -> usize {
    let max = source.len().min(target.len());
    if offset >= max {
        return 0;
    }
    let mut len = 0usize;
    while offset + len < max && source[offset + len] == target[offset + len] {
        len += 1;
    }
    len
}

fn emit_action(action: u64, len: usize, out: &mut Vec<u8>) -> Result<()> {
    anyhow::ensure!(len > 0, "BPS action length must be nonzero");
    let encoded = ((len as u64 - 1) << 2) | action;
    encode_number(encoded, out);
    Ok(())
}

fn encode_number(mut data: u64, out: &mut Vec<u8>) {
    loop {
        let x = (data & 0x7f) as u8;
        data >>= 7;
        if data == 0 {
            out.push(0x80 | x);
            break;
        }
        out.push(x);
        data -= 1;
    }
}

fn append_u32_le(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_number(input: &[u8], cursor: &mut usize) -> u64 {
        let mut data = 0u64;
        let mut shift = 1u64;
        loop {
            let x = input[*cursor];
            *cursor += 1;
            data += ((x & 0x7f) as u64) * shift;
            if x & 0x80 != 0 {
                break;
            }
            shift <<= 7;
            data += shift;
        }
        data
    }

    fn apply_bps(source: &[u8], patch: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(patch.len() >= 16, "patch too short");
        anyhow::ensure!(&patch[0..4] == b"BPS1", "bad BPS magic");
        let stored_patch_crc =
            u32::from_le_bytes(patch[patch.len() - 4..patch.len()].try_into().unwrap());
        anyhow::ensure!(
            crc32fast::hash(&patch[..patch.len() - 4]) == stored_patch_crc,
            "patch CRC mismatch"
        );

        let mut cursor = 4usize;
        let source_size = decode_number(patch, &mut cursor) as usize;
        let target_size = decode_number(patch, &mut cursor) as usize;
        let metadata_size = decode_number(patch, &mut cursor) as usize;
        cursor += metadata_size;
        anyhow::ensure!(source.len() == source_size, "source size mismatch");

        let body_end = patch.len() - 12;
        let mut target = Vec::with_capacity(target_size);
        while cursor < body_end {
            let action = decode_number(patch, &mut cursor);
            let command = action & 3;
            let len = ((action >> 2) + 1) as usize;
            match command {
                SOURCE_READ => {
                    let start = target.len();
                    target.extend_from_slice(&source[start..start + len]);
                }
                TARGET_READ => {
                    target.extend_from_slice(&patch[cursor..cursor + len]);
                    cursor += len;
                }
                _ => anyhow::bail!("unexpected copy action in minimal test decoder"),
            }
        }
        anyhow::ensure!(target.len() == target_size, "target size mismatch");

        let source_crc = u32::from_le_bytes(patch[body_end..body_end + 4].try_into().unwrap());
        let target_crc = u32::from_le_bytes(patch[body_end + 4..body_end + 8].try_into().unwrap());
        anyhow::ensure!(crc32fast::hash(source) == source_crc, "source CRC mismatch");
        anyhow::ensure!(
            crc32fast::hash(&target) == target_crc,
            "target CRC mismatch"
        );
        Ok(target)
    }

    #[test]
    fn number_encoding_matches_bps_roundtrip() {
        for value in [0, 1, 2, 127, 128, 129, 16_383, 16_384, 1_000_000] {
            let mut encoded = Vec::new();
            encode_number(value, &mut encoded);
            let mut cursor = 0usize;
            assert_eq!(decode_number(&encoded, &mut cursor), value);
            assert_eq!(cursor, encoded.len());
        }
    }

    #[test]
    fn bps_patch_roundtrips_target() {
        let source = b"abcdefghijabcdefghij";
        let target = b"abcXYZghijabcQQQghij!";
        let patch = create_bps(source, target, b"test").unwrap();
        assert_eq!(&patch[0..4], b"BPS1");
        assert_eq!(apply_bps(source, &patch).unwrap(), target);
    }

    #[test]
    fn bps_patch_rejects_wrong_source() {
        let source = b"abcdefghij";
        let target = b"abcXYZghij";
        let patch = create_bps(source, target, &[]).unwrap();
        let err = apply_bps(b"abcdxfghij", &patch).unwrap_err();
        assert!(err.to_string().contains("source CRC mismatch"));
    }
}
