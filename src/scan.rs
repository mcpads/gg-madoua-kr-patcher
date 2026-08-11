use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const BANK_SIZE: usize = 0x4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRange {
    pub file: String,
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteRun {
    pub value: u8,
    pub offset: usize,
    pub len: usize,
}

pub fn cmd_scan_prefix(rom_path: &Path, script_dir: Option<&Path>) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    println!("ROM: {} ({} bytes)", rom_path.display(), rom.len());

    let ranges = if let Some(dir) = script_dir {
        let ranges = read_script_ranges(dir)?;
        println!(
            "#STARTMSG ranges: {} across {} files",
            ranges.len(),
            ranges
                .iter()
                .map(|r| r.file.as_str())
                .collect::<BTreeSet<_>>()
                .len()
        );
        ranges
    } else {
        println!("script-dir 미지정: ROM 전체 byte 빈도만 집계");
        vec![ScriptRange {
            file: rom_path.display().to_string(),
            offset: 0,
            len: rom.len(),
        }]
    };

    let freq = byte_frequency(&rom, &ranges)?;
    let total: usize = freq.values().sum();
    let used: BTreeSet<u8> = freq.keys().copied().collect();
    let free: Vec<u8> = (0u16..=255)
        .map(|v| v as u8)
        .filter(|b| !used.contains(b))
        .collect();
    let engine_control = BTreeSet::from([0x00, 0xFB, 0xFC, 0xFD, 0xFE, 0xFF]);
    let free_safe: Vec<u8> = free
        .iter()
        .copied()
        .filter(|b| !engine_control.contains(b))
        .collect();

    println!("total bytes scanned: {total}");
    println!(
        "distinct byte values used: {} / free: {}",
        used.len(),
        free.len()
    );
    println!(
        "free & not engine-control(00,FB,FC,FD,FE,FF): {}",
        free_safe.len()
    );
    println!("free-safe byte ranges: {}", format_byte_ranges(&free_safe));

    let mut top: Vec<(u8, usize)> = freq.iter().map(|(&b, &n)| (b, n)).collect();
    top.sort_by_key(|&(b, n)| (std::cmp::Reverse(n), b));
    let top_text = top
        .iter()
        .take(12)
        .map(|(b, n)| format!("{b:02X}:{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("top 12 used bytes: {top_text}");
    Ok(())
}

pub fn cmd_scan_freespace(rom_path: &Path, min_run: usize) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    println!(
        "ROM size: {} bytes = {}KB = {} banks",
        rom.len(),
        rom.len() / 1024,
        rom.len() / BANK_SIZE
    );

    let mut total = 0usize;
    for value in [0xFF, 0x00] {
        let runs = find_runs(&rom, value, min_run);
        let sum: usize = runs.iter().map(|r| r.len).sum();
        total += sum;
        println!();
        println!(
            "=== 0x{value:02X} runs >={min_run}B: {} runs, {} bytes total ===",
            runs.len(),
            sum
        );
        let mut sorted = runs;
        sorted.sort_by_key(|r| std::cmp::Reverse(r.len));
        for r in sorted.iter().take(20) {
            let bank = r.offset / BANK_SIZE;
            let slot2 = (r.offset - bank * BANK_SIZE) + 0x8000;
            println!(
                "  0x{:05X}..0x{:05X}  {:5}B  bank {:2} (slot2 logical ${:04X})",
                r.offset,
                r.offset + r.len,
                r.len,
                bank,
                slot2
            );
        }
    }
    println!();
    println!(
        "TOTAL free (FF+00, >={min_run}B runs): {total} bytes = {}KB",
        total / 1024
    );
    println!("glyphs at 8B each in current free space: ~{}", total / 8);
    Ok(())
}

pub fn read_script_ranges(script_dir: &Path) -> Result<Vec<ScriptRange>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(script_dir)
        .with_context(|| format!("script dir 읽기 실패: {}", script_dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    files.sort();

    let mut ranges = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("script 읽기 실패: {}", path.display()))?;
        let file = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        ranges.extend(parse_startmsg_ranges(&text, &file));
    }
    Ok(ranges)
}

pub fn parse_startmsg_ranges(text: &str, file: &str) -> Vec<ScriptRange> {
    let mut ranges = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find("#STARTMSG(") {
        rest = &rest[pos + "#STARTMSG(".len()..];
        let Some(end) = rest.find(')') else {
            break;
        };
        let args = &rest[..end];
        let fields: Vec<&str> = args.split(',').map(str::trim).collect();
        if fields.len() >= 2 {
            if let (Ok(offset), Ok(len)) = (parse_int(fields[0]), parse_int(fields[1])) {
                ranges.push(ScriptRange {
                    file: file.to_string(),
                    offset,
                    len,
                });
            }
        }
        rest = &rest[end + 1..];
    }
    ranges
}

pub fn byte_frequency(rom: &[u8], ranges: &[ScriptRange]) -> Result<HashMap<u8, usize>> {
    let mut freq = HashMap::new();
    for r in ranges {
        let end = r
            .offset
            .checked_add(r.len)
            .with_context(|| format!("range overflow: {} 0x{:X}+{}", r.file, r.offset, r.len))?;
        anyhow::ensure!(
            end <= rom.len(),
            "{} range out of ROM: 0x{:X}..0x{:X} (rom len 0x{:X})",
            r.file,
            r.offset,
            end,
            rom.len()
        );
        for &b in &rom[r.offset..end] {
            *freq.entry(b).or_insert(0) += 1;
        }
    }
    Ok(freq)
}

pub fn find_runs(data: &[u8], value: u8, min_run: usize) -> Vec<ByteRun> {
    let mut runs = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        if data[i] != value {
            i += 1;
            continue;
        }
        let start = i;
        while i < data.len() && data[i] == value {
            i += 1;
        }
        let len = i - start;
        if len >= min_run {
            runs.push(ByteRun {
                value,
                offset: start,
                len,
            });
        }
    }
    runs
}

fn format_byte_ranges(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(none)".to_string();
    }
    let mut sorted = bytes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut out = Vec::new();
    let mut i = 0usize;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] == end.wrapping_add(1) {
            i += 1;
            end = sorted[i];
        }
        if start == end {
            out.push(format!("{start:02X}"));
        } else {
            out.push(format!("{start:02X}-{end:02X}"));
        }
        i += 1;
    }
    out.join(", ")
}

fn parse_int(s: &str) -> Result<usize, std::num::ParseIntError> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16)
    } else {
        s.parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_startmsg_ranges() {
        let text = "#STARTMSG(0x1C03E, 2, 1)\n#STARTMSG(123, 4, 0)";
        let ranges = parse_startmsg_ranges(text, "script.txt");
        assert_eq!(
            ranges,
            vec![
                ScriptRange {
                    file: "script.txt".to_string(),
                    offset: 0x1C03E,
                    len: 2,
                },
                ScriptRange {
                    file: "script.txt".to_string(),
                    offset: 123,
                    len: 4,
                }
            ]
        );
    }

    #[test]
    fn finds_runs_at_boundaries() {
        let data = [0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00];
        assert_eq!(
            find_runs(&data, 0xFF, 2),
            vec![ByteRun {
                value: 0xFF,
                offset: 0,
                len: 2,
            }]
        );
        assert_eq!(
            find_runs(&data, 0x00, 3),
            vec![ByteRun {
                value: 0x00,
                offset: 3,
                len: 3,
            }]
        );
    }

    #[test]
    fn tallies_bytes_from_ranges() {
        let rom = [0x10, 0x20, 0x10, 0x30];
        let ranges = [ScriptRange {
            file: "x".to_string(),
            offset: 1,
            len: 3,
        }];
        let freq = byte_frequency(&rom, &ranges).unwrap();
        assert_eq!(freq.get(&0x10), Some(&1));
        assert_eq!(freq.get(&0x20), Some(&1));
        assert_eq!(freq.get(&0x30), Some(&1));
    }
}
