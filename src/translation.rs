use crate::{glyph, script};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

const MONEY_TEXT_BYTE: u8 = 0x0C;
const EXPECTED_SHOP_ENTRY_COUNT: usize = 12;
const GG_VISIBLE_TEXT_COLUMNS: usize = 20;
const GG_ROM_BANK_SIZE: usize = 0x4000;
const GG_SLOT2_BASE: usize = 0x8000;
const KO_DYNAMIC_PUNCTUATION: [char; 2] = [',', '~'];

#[derive(Debug, Clone, Deserialize)]
pub struct TranslationOverride {
    pub id: String,
    #[serde(default, alias = "jp_preview")]
    pub jp: String,
    #[serde(default)]
    pub bytes_hex: String,
    #[serde(default)]
    pub source_crc32: String,
    #[serde(default, alias = "translation")]
    pub ko: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub skip: bool,
}

#[derive(Debug, Deserialize)]
struct TranslationFile {
    entries: Vec<TranslationOverride>,
}

#[derive(Debug, Clone)]
pub struct KoEncoding {
    ordered_chars: Vec<char>,
    char_to_code: HashMap<char, [u8; 2]>,
}

/// 빈도 rank(0부터)를 `[prefix, base]` 코드로 변환한다.
///
/// prefix = `KO_PREFIXES[rank / PER_PREFIX]`, base = `0x01 + rank % PER_PREFIX`.
/// 따라서 glyph# = `prefix_index * PER_PREFIX + (base - 1) = rank` — 글리프 뱅크는 rank
/// 순서로 저장되고 핸들러가 (prefix, base)로 이 rank를 복원한다. 캡 지연 모드의 초과분은
/// prefix를 순환 배정한다(길이 검사만 유효).
fn code_for_rank(rank: usize) -> [u8; 2] {
    let pidx = (rank / glyph::PER_PREFIX) % glyph::KO_PREFIXES.len();
    let base = 0x01 + (rank % glyph::PER_PREFIX) as u8;
    [glyph::KO_PREFIXES[pidx], base]
}

impl KoEncoding {
    pub fn from_texts(texts: &[String]) -> Result<Self> {
        Self::from_texts_capped(texts, true)
    }

    /// `enforce_cap=false`이면 다중 프리픽스 용량(`KO_MULTI_PREFIX_CAP`) 초과를 오류로 만들지
    /// 않는다.
    ///
    /// 번역 정확성 검증(charmap·태그·길이)을 빌드 용량 캡과 분리해 돌리기 위한 모드다. 초과분
    /// 글리프에는 prefix를 순환 배정한다 — 한글 음절은 code 값과 무관하게 항상 2바이트라 인코딩
    /// **길이** 검사(overlength/shop-fit)는 정확하다. 실제 ROM 빌드 경로는 `from_texts`(캡 강제)를
    /// 쓴다. 현재 전체 corpus(한글 735 + 전용 문장부호)는 캡(1000) 이내라 빌드가 열려 있다.
    pub fn from_texts_capped(texts: &[String], enforce_cap: bool) -> Result<Self> {
        let mut freq: HashMap<char, usize> = HashMap::new();
        let mut punctuation = BTreeSet::new();
        for text in texts {
            for token in parse_text(text)? {
                if let TextToken::Char(ch) = token {
                    if is_hangul_syllable(ch) {
                        *freq.entry(ch).or_default() += 1;
                    } else if KO_DYNAMIC_PUNCTUATION.contains(&ch) {
                        punctuation.insert(ch);
                    }
                }
            }
        }

        let mut counted: Vec<(char, usize)> = freq.into_iter().collect();
        counted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let glyph_count = counted.len() + punctuation.len();
        anyhow::ensure!(
            !enforce_cap || glyph_count <= glyph::KO_MULTI_PREFIX_CAP,
            "KO 글리프 {}개가 다중 프리픽스 용량 {}개({} prefixes × {})를 초과함",
            glyph_count,
            glyph::KO_MULTI_PREFIX_CAP,
            glyph::KO_PREFIXES.len(),
            glyph::PER_PREFIX
        );

        let mut char_to_code = HashMap::new();
        let mut ordered_chars = Vec::new();
        for (i, (ch, _)) in counted.into_iter().enumerate() {
            char_to_code.insert(ch, code_for_rank(i));
            ordered_chars.push(ch);
        }
        // 한글의 기존 빈도 rank를 흔들지 않도록 문장부호는 고정 순서로 맨 뒤에 붙인다.
        for ch in KO_DYNAMIC_PUNCTUATION {
            if punctuation.contains(&ch) {
                let rank = ordered_chars.len();
                char_to_code.insert(ch, code_for_rank(rank));
                ordered_chars.push(ch);
            }
        }

        Ok(Self {
            ordered_chars,
            char_to_code,
        })
    }

    pub fn glyph_count(&self) -> usize {
        self.ordered_chars.len()
    }

    pub fn glyph_chars(&self) -> &[char] {
        &self.ordered_chars
    }

    /// 음절의 `[prefix, base]` 코드(없으면 None).
    fn code_for(&self, ch: char) -> Option<[u8; 2]> {
        self.char_to_code.get(&ch).copied()
    }
}

#[derive(Debug)]
pub struct EncodedOverride {
    pub id: String,
    pub bytes: Vec<u8>,
    pub source: Option<TranslationSource>,
}

#[derive(Debug, Clone)]
pub struct TranslationSource {
    pub bytes: Vec<u8>,
    pub crc32: Option<u32>,
}

#[derive(Debug)]
pub struct TranslationPlan {
    pub encoding: KoEncoding,
    pub encoded: Vec<EncodedOverride>,
}

pub fn cmd_check_glyphs(translations_path: &Path) -> Result<()> {
    let plan = load_translation_plan(translations_path)?;
    println!("번역 검사 완료: {}", translations_path.display());
    println!("active entries: {}", plan.encoded.len());
    println!(
        "KO glyphs: {}/{}",
        plan.encoding.glyph_count(),
        glyph::KO_MULTI_PREFIX_CAP
    );
    if !plan.encoding.glyph_chars().is_empty() {
        let chars: String = plan.encoding.glyph_chars().iter().collect();
        println!("glyph order: {chars}");
    }
    for encoded in &plan.encoded {
        println!("{}: {} bytes", encoded.id, encoded.bytes.len());
    }
    Ok(())
}

pub fn cmd_check_terms(terms_path: &Path, raw_path: &Path) -> Result<()> {
    let terms = load_terms(terms_path)?;
    let raw_entries = load_raw_text_entries(raw_path)?;
    let raw_by_id: BTreeMap<String, String> = raw_entries.into_iter().collect();

    let mut seen = BTreeMap::new();
    for entry in &terms.entries {
        validate_term_entry(entry)?;
        let key = (entry.category.clone(), entry.jp.clone());
        if let Some(previous) = seen.insert(key, entry.ko.clone()) {
            anyhow::bail!(
                "중복 용어: category={} jp={} ({} and {})",
                entry.category,
                entry.jp,
                previous,
                entry.ko
            );
        }

        for reference in &entry.refs {
            let raw_jp = raw_by_id
                .get(reference)
                .with_context(|| format!("{}: raw ref not found: {}", entry.jp, reference))?;
            let normalized_raw = normalize_term_text(raw_jp);
            let matched = entry.match_terms().any(|term| {
                let normalized_term = normalize_term_text(term);
                !normalized_term.is_empty() && normalized_raw.contains(&normalized_term)
            });
            anyhow::ensure!(
                matched,
                "{}: ref {} does not contain jp or aliases",
                entry.jp,
                reference
            );
        }
    }

    println!("용어집 검사 완료: {}", terms_path.display());
    println!("raw input: {}", raw_path.display());
    println!("terms: {}", terms.entries.len());
    println!("raw entries: {}", raw_by_id.len());
    for entry in &terms.entries {
        let count = raw_by_id
            .values()
            .filter(|raw_jp| {
                let normalized_raw = normalize_term_text(raw_jp);
                entry.match_terms().any(|term| {
                    let normalized_term = normalize_term_text(term);
                    !normalized_term.is_empty() && normalized_raw.contains(&normalized_term)
                })
            })
            .count();
        println!(
            "{} / {} -> {} (refs {}, raw entries {})",
            entry.category,
            entry.jp,
            entry.ko,
            entry.refs.len(),
            count
        );
    }
    Ok(())
}

pub fn cmd_check_stage(
    stage_path: &Path,
    raw_path: &Path,
    terms_path: &Path,
    defer_glyph_cap: bool,
) -> Result<()> {
    let stage = stage_name_for_path(stage_path);
    let files = stage_input_files(stage_path)?;
    let raw_entries = load_raw_stage_entries(raw_path)?;
    let raw_by_id: BTreeMap<String, RawTextEntry> = raw_entries
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect();
    let terms = load_terms(terms_path)?;

    let mut entries = Vec::new();
    for file_path in &files {
        let text = std::fs::read_to_string(file_path)
            .with_context(|| format!("stage JSON 읽기 실패: {}", file_path.display()))?;
        let file: StageTranslationFile = serde_json::from_str(&text)
            .with_context(|| format!("stage JSON 파싱 실패: {}", file_path.display()))?;
        for entry in file.entries {
            entries.push((file_path.clone(), entry));
        }
    }
    anyhow::ensure!(
        !entries.is_empty(),
        "stage entries가 비어 있음: {}",
        stage_path.display()
    );

    let active_texts: Vec<String> = entries
        .iter()
        .filter(|(_, entry)| !entry.skip && !entry.ko.trim().is_empty())
        .map(|(_, entry)| entry.ko.clone())
        .collect();
    let encoding = KoEncoding::from_texts_capped(&active_texts, !defer_glyph_cap)?;

    let mut seen = BTreeMap::new();
    let mut translated = 0usize;
    let mut overlength = 0usize;
    let mut shop_layout_verified = 0usize;
    let mut screen_ceiling_verified = 0usize;
    for (file_path, entry) in &entries {
        if let Some(previous) = seen.insert(entry.id.clone(), file_path.clone()) {
            anyhow::bail!(
                "중복 stage entry id: {} ({} and {})",
                entry.id,
                previous.display(),
                file_path.display()
            );
        }
        let raw = raw_by_id
            .get(&entry.id)
            .with_context(|| format!("{}: raw baseline entry not found", entry.id))?;
        validate_stage_entry(file_path, &stage, entry, raw, &terms, &encoding)?;
        if !entry.skip && !entry.ko.trim().is_empty() {
            translated += 1;
            let bytes = encode_translation_text(&entry.ko, &encoding)
                .with_context(|| format!("{} 인코딩 실패", entry.id))?;
            if raw.kind.as_deref() == Some("shop") {
                let raw_bytes = parse_hex_bytes(&raw.bytes_hex)
                    .with_context(|| format!("{} raw shop bytes_hex", entry.id))?;
                let raw_len = raw
                    .len
                    .with_context(|| format!("{}: raw shop entry has no len", entry.id))?;
                validate_shop_layout(&entry.id, &raw.jp, &entry.ko, &raw_bytes, &bytes, raw_len)?;
                shop_layout_verified += 1;
            } else {
                validate_visible_line_ceiling(&entry.id, &entry.ko, GG_VISIBLE_TEXT_COLUMNS)?;
                screen_ceiling_verified += 1;
                if let Some(raw_len) = raw.len
                    && bytes.len() > raw_len
                {
                    overlength += 1;
                }
            }
        }
    }

    println!("stage 검사 완료: {}", stage_path.display());
    println!("stage: {}", stage.as_deref().unwrap_or("unknown"));
    println!("files: {}", files.len());
    println!("entries: {}", entries.len());
    println!("translated entries: {}", translated);
    if defer_glyph_cap && encoding.glyph_count() > glyph::KO_MULTI_PREFIX_CAP {
        println!(
            "KO glyphs: {}/{} (캡 초과 — 프리픽스 추가 필요, 정확성만 검증)",
            encoding.glyph_count(),
            glyph::KO_MULTI_PREFIX_CAP
        );
    } else {
        println!(
            "KO glyphs: {}/{}",
            encoding.glyph_count(),
            glyph::KO_MULTI_PREFIX_CAP
        );
    }
    println!("overlength non-shop entries: {}", overlength);
    println!("shop layout-preserving entries: {}", shop_layout_verified);
    println!(
        "non-shop screen-ceiling entries ({} tiles): {}",
        GG_VISIBLE_TEXT_COLUMNS, screen_ceiling_verified
    );
    Ok(())
}

fn validate_visible_line_ceiling(entry_id: &str, text: &str, max_tiles: usize) -> Result<()> {
    let mut column = 0usize;
    for token in parse_text(text)? {
        match token {
            TextToken::Char(_) | TextToken::Money | TextToken::Raw(_) => {
                column += 1;
                anyhow::ensure!(
                    column <= max_tiles,
                    "{}: visible line exceeds {} tiles (got at least {})",
                    entry_id,
                    max_tiles,
                    column
                );
            }
            TextToken::Br | TextToken::Wait | TextToken::End => column = 0,
            TextToken::Flags(_) => {}
        }
    }
    Ok(())
}

fn validate_shop_layout(
    entry_id: &str,
    raw_text: &str,
    translated_text: &str,
    raw: &[u8],
    encoded: &[u8],
    expected_len: usize,
) -> Result<()> {
    anyhow::ensure!(
        raw.len() == expected_len,
        "{}: raw shop bytes len {} != metadata len {}",
        entry_id,
        raw.len(),
        expected_len
    );
    anyhow::ensure!(
        encoded.len() == expected_len,
        "{}: shop translation must preserve exact source len {} (got {})",
        entry_id,
        expected_len,
        encoded.len()
    );

    let money_positions = byte_positions(raw, MONEY_TEXT_BYTE);
    anyhow::ensure!(
        money_positions.len() == 1,
        "{}: raw shop entry must contain exactly one money byte, got {}",
        entry_id,
        money_positions.len()
    );
    let encoded_money_positions = byte_positions(encoded, MONEY_TEXT_BYTE);
    anyhow::ensure!(
        encoded_money_positions.len() == 1,
        "{}: encoded shop entry must contain exactly one money byte, got {}",
        entry_id,
        encoded_money_positions.len()
    );
    let raw_money = money_positions[0];
    let encoded_money = encoded_money_positions[0];
    let raw_padding = raw[raw_money + 1..]
        .iter()
        .take_while(|&&byte| byte == 0x01)
        .count();
    let encoded_padding = encoded[encoded_money + 1..]
        .iter()
        .take_while(|&&byte| byte == 0x01)
        .count();
    anyhow::ensure!(
        encoded_padding == raw_padding,
        "{}: post-money price padding changed: raw {}, encoded {}",
        entry_id,
        raw_padding,
        encoded_padding
    );

    let raw_layout = shop_visible_layout(entry_id, raw_text)?;
    let encoded_layout = shop_visible_layout(entry_id, translated_text)?;
    anyhow::ensure!(
        encoded_layout.money.len() == raw_layout.money.len(),
        "{}: shop money screen anchor count changed: raw {}, encoded {}",
        entry_id,
        raw_layout.money.len(),
        encoded_layout.money.len()
    );
    for ((raw_line, raw_column), (encoded_line, encoded_column)) in
        raw_layout.money.iter().zip(&encoded_layout.money)
    {
        anyhow::ensure!(
            encoded_line == raw_line,
            "{}: shop money moved from line {} to {}",
            entry_id,
            raw_line,
            encoded_line
        );
        anyhow::ensure!(
            encoded_column <= raw_column,
            "{}: shop money moved right from column {} to {}",
            entry_id,
            raw_column,
            encoded_column
        );
    }
    anyhow::ensure!(
        encoded_layout.line_widths.len() == raw_layout.line_widths.len(),
        "{}: shop visible line count changed: raw {}, encoded {}",
        entry_id,
        raw_layout.line_widths.len(),
        encoded_layout.line_widths.len()
    );
    for (line, (&raw_width, &encoded_width)) in raw_layout
        .line_widths
        .iter()
        .zip(&encoded_layout.line_widths)
        .enumerate()
    {
        anyhow::ensure!(
            encoded_width <= raw_width,
            "{}: shop line {} grew from {} to {} visible tiles",
            entry_id,
            line,
            raw_width,
            encoded_width
        );
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct ShopVisibleLayout {
    line_widths: Vec<usize>,
    money: Vec<(usize, usize)>,
}

fn shop_visible_layout(entry_id: &str, text: &str) -> Result<ShopVisibleLayout> {
    let mut line_widths = vec![0usize];
    let mut money = Vec::new();
    let mut line = 0usize;
    let mut column = 0usize;
    let mut end_count = 0usize;
    for token in parse_text(text)? {
        match token {
            TextToken::Char(ch) => {
                if ch == '金' {
                    money.push((line, column));
                }
                column += 1;
                line_widths[line] = column;
            }
            TextToken::Money => {
                money.push((line, column));
                column += 1;
                line_widths[line] = column;
            }
            TextToken::Br => {
                line += 1;
                column = 0;
                line_widths.push(0);
            }
            TextToken::End => end_count += 1,
            TextToken::Wait | TextToken::Flags(_) => {}
            TextToken::Raw(_) => {
                column += 1;
                line_widths[line] = column;
            }
        }
    }
    anyhow::ensure!(
        end_count == 1,
        "{}: shop entry must have exactly one end tag, got {}",
        entry_id,
        end_count
    );

    Ok(ShopVisibleLayout { line_widths, money })
}

fn byte_positions(bytes: &[u8], needle: u8) -> Vec<usize> {
    bytes
        .iter()
        .enumerate()
        .filter_map(|(index, &byte)| (byte == needle).then_some(index))
        .collect()
}

pub fn cmd_check_surfaces(
    catalog_path: &Path,
    raw_path: &Path,
    require_release_ready: bool,
) -> Result<()> {
    let catalog = load_surface_catalog(catalog_path)?;
    let raw_entries = load_raw_stage_entries(raw_path)?;
    let raw_by_id: BTreeMap<String, RawTextEntry> = raw_entries
        .iter()
        .map(|entry| (entry.id.clone(), entry.clone()))
        .collect();
    let raw_shop_ids: BTreeSet<String> = raw_entries
        .iter()
        .filter(|entry| entry.kind.as_deref() == Some("shop"))
        .map(|entry| entry.id.clone())
        .collect();
    anyhow::ensure!(
        raw_shop_ids.len() == 12,
        "raw shop entry count changed: expected 12, got {}",
        raw_shop_ids.len()
    );

    let mut seen_ids = BTreeSet::new();
    let mut shop_coverage: BTreeMap<String, usize> = BTreeMap::new();
    let mut unverified_graphics = 0usize;
    let mut release_blockers = Vec::new();
    for surface in &catalog.surfaces {
        validate_surface_entry(surface)?;
        anyhow::ensure!(
            seen_ids.insert(surface.id.clone()),
            "중복 surface id: {}",
            surface.id
        );

        let source_ref = surface
            .source_ref
            .as_deref()
            .map(str::trim)
            .filter(|source_ref| !source_ref.is_empty());
        match source_ref {
            Some(source_ref) => {
                let raw = raw_by_id.get(source_ref).with_context(|| {
                    format!("{}: raw source_ref not found: {}", surface.id, source_ref)
                })?;
                if surface.kind == "shop_text" {
                    anyhow::ensure!(
                        raw.kind.as_deref() == Some("shop"),
                        "{}: shop_text source_ref must point to raw kind=shop",
                        surface.id
                    );
                    anyhow::ensure!(
                        surface.policy == "fixed_len_only",
                        "{}: shop_text must stay policy=fixed_len_only until relocation/UI policy is proven",
                        surface.id
                    );
                    anyhow::ensure!(
                        raw.len.is_some(),
                        "{}: shop_text raw source must carry len",
                        surface.id
                    );
                    *shop_coverage.entry(source_ref.to_string()).or_default() += 1;
                }
            }
            None => {
                anyhow::ensure!(
                    surface.kind != "shop_text",
                    "{}: shop_text requires source_ref",
                    surface.id
                );
                anyhow::ensure!(
                    surface.policy != "fixed_len_only",
                    "{}: fixed_len_only surface requires source_ref",
                    surface.id
                );
            }
        }

        if matches!(
            surface.kind.as_str(),
            "hud_or_graphics_text" | "graphics_text"
        ) && surface.status == "unverified"
        {
            unverified_graphics += 1;
        }
        if require_release_ready {
            if surface.status != "verified" {
                release_blockers.push(format!(
                    "{}: status={} is not release-ready",
                    surface.id, surface.status
                ));
            }
            if matches!(
                surface.policy.as_str(),
                "inventory_required" | "do_not_build"
            ) {
                release_blockers.push(format!(
                    "{}: policy={} is not release-ready",
                    surface.id, surface.policy
                ));
            }
        }
    }

    for shop_id in &raw_shop_ids {
        let count = shop_coverage.get(shop_id).copied().unwrap_or_default();
        anyhow::ensure!(
            count == 1,
            "{}: raw shop entry needs exactly one shop_text surface, got {}",
            shop_id,
            count
        );
    }

    println!("surface catalog 검사 완료: {}", catalog_path.display());
    println!("raw input: {}", raw_path.display());
    println!("surfaces: {}", catalog.surfaces.len());
    println!(
        "shop raw entries covered: {}/{}",
        shop_coverage.len(),
        raw_shop_ids.len()
    );
    println!("unverified graphics/HUD surfaces: {}", unverified_graphics);
    if require_release_ready {
        anyhow::ensure!(
            release_blockers.is_empty(),
            "surface release readiness failed:\n{}",
            release_blockers.join("\n")
        );
        println!("surface release readiness: ok");
    }
    Ok(())
}

pub fn cmd_check_money_sources(rom_path: &Path, raw_path: &Path) -> Result<()> {
    let rom = std::fs::read(rom_path)
        .with_context(|| format!("ROM 읽기 실패: {}", rom_path.display()))?;
    let raw_entries = load_raw_stage_entries(raw_path)?;
    let shop_entries: Vec<&RawTextEntry> = raw_entries
        .iter()
        .filter(|entry| entry.kind.as_deref() == Some("shop"))
        .collect();
    anyhow::ensure!(
        shop_entries.len() == EXPECTED_SHOP_ENTRY_COUNT,
        "raw shop entry count changed: expected {}, got {}",
        EXPECTED_SHOP_ENTRY_COUNT,
        shop_entries.len()
    );

    let mut entries_with_money = 0usize;
    let mut money_occurrences = 0usize;
    for entry in &shop_entries {
        let bytes =
            parse_hex_bytes(&entry.bytes_hex).with_context(|| format!("{} bytes_hex", entry.id))?;
        let count = bytes
            .iter()
            .filter(|&&byte| byte == MONEY_TEXT_BYTE)
            .count();
        anyhow::ensure!(
            count > 0,
            "{}: shop raw entry does not contain money text byte 0x{MONEY_TEXT_BYTE:02X}",
            entry.id
        );
        anyhow::ensure!(
            entry.jp.contains('金'),
            "{}: shop raw jp preview does not contain 金",
            entry.id
        );
        entries_with_money += 1;
        money_occurrences += count;
    }

    let glyph_index = glyph::font_index(MONEY_TEXT_BYTE);
    let glyph_offset = glyph::FONT_BASE + glyph_index * glyph::GLYPH_BYTES;
    anyhow::ensure!(
        glyph_offset + glyph::GLYPH_BYTES <= rom.len(),
        "money glyph candidate offset 0x{glyph_offset:05X} is outside ROM"
    );
    let glyph_bytes: [u8; glyph::GLYPH_BYTES] = rom
        [glyph_offset..glyph_offset + glyph::GLYPH_BYTES]
        .try_into()
        .unwrap();
    anyhow::ensure!(
        glyph_bytes.iter().any(|&byte| byte != 0),
        "money glyph candidate at 0x{glyph_offset:05X} is blank"
    );
    let exact_matches = find_exact_glyph_matches(&rom, &glyph_bytes);
    anyhow::ensure!(
        exact_matches.contains(&glyph_offset),
        "money glyph candidate scan did not include main-font offset 0x{glyph_offset:05X}"
    );

    println!("money source 정적 검사 완료: {}", raw_path.display());
    println!("ROM: {}", rom_path.display());
    println!("money text byte: 0x{MONEY_TEXT_BYTE:02X} (金)");
    println!("main-font candidate glyph index: 0x{glyph_index:02X}");
    println!("main-font candidate ROM offset: 0x{glyph_offset:05X}");
    println!("candidate glyph bytes: {}", format_hex_bytes(&glyph_bytes));
    println!(
        "candidate glyph preview:\n{}",
        glyph::to_ascii(&glyph_bytes)
    );
    println!("exact glyph byte matches in ROM: {}", exact_matches.len());
    for offset in &exact_matches {
        let bank = offset / GG_ROM_BANK_SIZE;
        let bank_offset = offset % GG_ROM_BANK_SIZE;
        let slot2_logical = slot2_logical_address(*offset);
        let marker = if *offset == glyph_offset {
            " main-font-candidate"
        } else {
            ""
        };
        println!(
            "  match rom=0x{offset:05X} bank=0x{bank:02X} bank_offset=0x{bank_offset:04X} slot2_logical=0x{slot2_logical:04X}{marker}"
        );
    }
    println!("shop entries with money byte: {entries_with_money}/{EXPECTED_SHOP_ENTRY_COUNT}");
    println!("shop money byte occurrences: {money_occurrences}");
    println!(
        "status: HUD source and shop fixed-layout policy resolved; deterministic natural-route shop runtime scene still required"
    );
    Ok(())
}

/// 디렉토리(배포) 빌드는 모든 엔트리가 원문 대조용 source 메타데이터(bytes_hex + source_crc32)를
/// 가져야 한다. source 없는 overlay는 단일 JSON PoC 파일 경로에서만 허용된다 — 이 가드가 없으면
/// stale 번역이 바뀐 추출 데이터 위에 조용히 적용될 수 있다.
pub(crate) fn require_source_metadata(encoded: &[EncodedOverride]) -> Result<()> {
    for e in encoded {
        anyhow::ensure!(
            e.source.is_some(),
            "{}: 디렉토리(배포) 빌드는 source 메타데이터(bytes_hex + source_crc32)가 필수다 — source 없는 overlay는 단일 JSON PoC 파일로만 넘긴다",
            e.id
        );
    }
    Ok(())
}

pub fn load_translation_plan(path: &Path) -> Result<TranslationPlan> {
    let files = translation_input_files(path)?;
    load_translation_plan_from_files(files, None)
}

/// 사람 최종 검수 전의 전체 번역을 QA ROM으로만 확인하기 위한 명시적 경로다.
/// 기본 `load_translation_plan`은 계속 `complete/`만 읽으므로 배포 입력 자격은 완화되지 않는다.
pub fn load_human_review_preview_plan(path: &Path) -> Result<TranslationPlan> {
    let files = human_review_input_files(path)?;
    load_translation_plan_from_files(files, Some("needs_human_review"))
}

fn load_translation_plan_from_files(
    files: Vec<PathBuf>,
    required_status: Option<&str>,
) -> Result<TranslationPlan> {
    let overrides = load_translation_overrides(files)?;
    if let Some(required_status) = required_status {
        for entry in &overrides {
            anyhow::ensure!(
                entry.status == required_status,
                "{}: QA preview requires status={required_status}, got {}",
                entry.id,
                entry.status
            );
        }
    }
    let active_texts: Vec<String> = overrides
        .iter()
        .filter(|entry| !entry.skip && !entry.ko.trim().is_empty())
        .map(|entry| entry.ko.clone())
        .collect();
    let encoding = KoEncoding::from_texts(&active_texts)?;
    let mut encoded = Vec::new();
    for entry in &overrides {
        validate_source_metadata(entry)?;
        if entry.skip || entry.ko.trim().is_empty() {
            continue;
        }
        validate_korean_punctuation(&entry.id, &entry.ko)?;
        let bytes = encode_translation_text(&entry.ko, &encoding)
            .with_context(|| format!("{} 인코딩 실패", entry.id))?;
        encoded.push(EncodedOverride {
            id: entry.id.clone(),
            bytes,
            source: source_guard(entry)?,
        });
    }

    Ok(TranslationPlan { encoding, encoded })
}

fn load_translation_overrides(files: Vec<PathBuf>) -> Result<Vec<TranslationOverride>> {
    let mut seen = BTreeMap::new();
    let mut entries = Vec::new();
    for file_path in files {
        let text = std::fs::read_to_string(&file_path)
            .with_context(|| format!("번역 JSON 읽기 실패: {}", file_path.display()))?;
        let file: TranslationFile = serde_json::from_str(&text)
            .with_context(|| format!("번역 JSON 파싱 실패: {}", file_path.display()))?;
        for entry in file.entries {
            anyhow::ensure!(
                !entry.id.trim().is_empty(),
                "{}: 빈 translation id",
                file_path.display()
            );
            if let Some(previous) = seen.insert(entry.id.clone(), file_path.clone()) {
                anyhow::bail!(
                    "중복 translation id: {} ({} and {})",
                    entry.id,
                    previous.display(),
                    file_path.display()
                );
            }
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn translation_input_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    anyhow::ensure!(
        path.is_dir(),
        "번역 입력 경로가 파일/디렉토리가 아님: {}",
        path.display()
    );
    let complete_dir = complete_stage_dir(path)?;
    let mut files = Vec::new();
    collect_json_files(&complete_dir, &mut files)?;
    files.sort();
    anyhow::ensure!(
        !files.is_empty(),
        "{} 안에 complete JSON이 없음",
        complete_dir.display()
    );
    Ok(files)
}

fn human_review_input_files(path: &Path) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        path.is_dir(),
        "needs_human_review QA preview는 stage 디렉토리 입력만 허용함: {}",
        path.display()
    );
    let stage_dir = named_stage_dir(path, "needs_human_review")?;
    let mut files = Vec::new();
    collect_json_files(&stage_dir, &mut files)?;
    files.sort();
    anyhow::ensure!(
        !files.is_empty(),
        "{} 안에 needs_human_review JSON이 없음",
        stage_dir.display()
    );
    Ok(files)
}

fn complete_stage_dir(path: &Path) -> Result<PathBuf> {
    if path.file_name().is_some_and(|name| name == "complete") {
        return Ok(path.to_path_buf());
    }
    let scripts_complete = path.join("scripts").join("complete");
    if scripts_complete.is_dir() {
        return Ok(scripts_complete);
    }
    let direct_complete = path.join("complete");
    if direct_complete.is_dir() {
        return Ok(direct_complete);
    }
    anyhow::bail!(
        "{} is a directory translation input, but no scripts/complete or complete directory exists; pass a specific JSON file for gate overlays",
        path.display()
    )
}

fn named_stage_dir(path: &Path, stage: &str) -> Result<PathBuf> {
    if path.file_name().is_some_and(|name| name == stage) {
        return Ok(path.to_path_buf());
    }
    let scripts_stage = path.join("scripts").join(stage);
    if scripts_stage.is_dir() {
        return Ok(scripts_stage);
    }
    let direct_stage = path.join(stage);
    if direct_stage.is_dir() {
        return Ok(direct_stage);
    }
    anyhow::bail!(
        "{} is not a {stage} translation input; pass that stage directly or its workflow root",
        path.display()
    )
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("디렉토리 읽기 실패: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TermsFile {
    format: String,
    entries: Vec<TermEntry>,
}

#[derive(Debug, Deserialize)]
struct TermEntry {
    category: String,
    jp: String,
    ko: String,
    status: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    refs: Vec<String>,
    #[serde(default)]
    match_refs_only: bool,
}

impl TermEntry {
    fn match_terms(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.jp.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    fn is_stable(&self) -> bool {
        matches!(self.status.as_str(), "approved_series" | "project_decision")
    }
}

#[derive(Debug, Deserialize)]
struct StageTranslationFile {
    entries: Vec<StageTranslationEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct StageTranslationEntry {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    region: Option<usize>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    offset: Option<String>,
    #[serde(default)]
    len: Option<usize>,
    #[serde(default, alias = "jp_preview")]
    jp: String,
    #[serde(default)]
    bytes_hex: String,
    #[serde(default)]
    source_crc32: String,
    #[serde(default, alias = "translation")]
    ko: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    skip: bool,
}

impl StageTranslationEntry {
    fn as_override(&self) -> TranslationOverride {
        TranslationOverride {
            id: self.id.clone(),
            jp: self.jp.clone(),
            bytes_hex: self.bytes_hex.clone(),
            source_crc32: self.source_crc32.clone(),
            ko: self.ko.clone(),
            status: self.status.clone(),
            notes: self.notes.clone(),
            skip: self.skip,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawTextFile {
    entries: Vec<RawTextEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTextEntry {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    region: Option<usize>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    offset: Option<String>,
    #[serde(default)]
    len: Option<usize>,
    #[serde(default, alias = "jp_preview")]
    jp: String,
    #[serde(default)]
    bytes_hex: String,
    #[serde(default)]
    source_crc32: String,
}

#[derive(Debug, Deserialize)]
struct SurfaceCatalog {
    format: String,
    surfaces: Vec<SurfaceEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct SurfaceEntry {
    id: String,
    kind: String,
    #[serde(default)]
    source_ref: Option<String>,
    policy: String,
    status: String,
    #[serde(default)]
    risks: Vec<String>,
    #[serde(default)]
    notes: String,
}

fn load_surface_catalog(path: &Path) -> Result<SurfaceCatalog> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("surface catalog 읽기 실패: {}", path.display()))?;
    let catalog: SurfaceCatalog = serde_json::from_str(&text)
        .with_context(|| format!("surface catalog JSON 파싱 실패: {}", path.display()))?;
    anyhow::ensure!(
        catalog.format == "madoua-surface-inventory-v1",
        "지원하지 않는 surface catalog format: {}",
        catalog.format
    );
    anyhow::ensure!(
        !catalog.surfaces.is_empty(),
        "surface catalog surfaces가 비어 있음"
    );
    Ok(catalog)
}

fn validate_surface_entry(entry: &SurfaceEntry) -> Result<()> {
    anyhow::ensure!(!entry.id.trim().is_empty(), "빈 surface id");
    anyhow::ensure!(
        matches!(
            entry.kind.as_str(),
            "shop_text" | "hud_or_graphics_text" | "graphics_text"
        ),
        "{}: 알 수 없는 surface kind {}",
        entry.id,
        entry.kind
    );
    anyhow::ensure!(
        matches!(
            entry.policy.as_str(),
            "fixed_len_only" | "inventory_required" | "do_not_build" | "normal_text"
        ),
        "{}: 알 수 없는 surface policy {}",
        entry.id,
        entry.policy
    );
    anyhow::ensure!(
        matches!(
            entry.status.as_str(),
            "cataloged" | "blocked_until_policy" | "unverified" | "verified"
        ),
        "{}: 알 수 없는 surface status {}",
        entry.id,
        entry.status
    );
    anyhow::ensure!(
        !entry.notes.trim().is_empty(),
        "{}: surface notes가 비어 있음",
        entry.id
    );
    for risk in &entry.risks {
        anyhow::ensure!(!risk.trim().is_empty(), "{}: 빈 surface risk", entry.id);
    }
    Ok(())
}

fn load_terms(path: &Path) -> Result<TermsFile> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("용어집 읽기 실패: {}", path.display()))?;
    let terms: TermsFile = serde_json::from_str(&text)
        .with_context(|| format!("용어집 JSON 파싱 실패: {}", path.display()))?;
    anyhow::ensure!(
        terms.format == "madoua-terms-v1",
        "지원하지 않는 terms format: {}",
        terms.format
    );
    anyhow::ensure!(!terms.entries.is_empty(), "용어집 entries가 비어 있음");
    Ok(terms)
}

fn validate_term_entry(entry: &TermEntry) -> Result<()> {
    anyhow::ensure!(
        !entry.category.trim().is_empty(),
        "{}: 빈 category",
        entry.jp
    );
    anyhow::ensure!(!entry.jp.trim().is_empty(), "빈 jp 용어");
    anyhow::ensure!(!entry.ko.trim().is_empty(), "{}: 빈 ko 용어", entry.jp);
    anyhow::ensure!(
        !entry.refs.is_empty(),
        "{}: raw refs가 비어 있음; A 추출물에 실제 등장하는 용어만 seed에 둔다",
        entry.jp
    );
    let _source = entry.source.trim();
    let _notes = entry.notes.trim();
    let valid_status = matches!(
        entry.status.as_str(),
        "approved_series" | "project_decision" | "needs_review" | "tentative"
    );
    anyhow::ensure!(
        valid_status,
        "{}: 알 수 없는 status {}",
        entry.jp,
        entry.status
    );
    Ok(())
}

fn load_raw_text_entries(path: &Path) -> Result<Vec<(String, String)>> {
    Ok(load_raw_stage_entries(path)?
        .into_iter()
        .map(|entry| (entry.id, entry.jp))
        .collect())
}

fn load_raw_stage_entries(path: &Path) -> Result<Vec<RawTextEntry>> {
    let files = raw_text_input_files(path)?;
    let mut entries = Vec::new();
    for file_path in files {
        let text = std::fs::read_to_string(&file_path)
            .with_context(|| format!("raw JSON 읽기 실패: {}", file_path.display()))?;
        let file: RawTextFile = serde_json::from_str(&text)
            .with_context(|| format!("raw JSON 파싱 실패: {}", file_path.display()))?;
        for entry in file.entries {
            anyhow::ensure!(
                !entry.id.trim().is_empty(),
                "{}: 빈 raw entry id",
                file_path.display()
            );
            entries.push(entry);
        }
    }
    anyhow::ensure!(
        !entries.is_empty(),
        "raw entries가 비어 있음: {}",
        path.display()
    );
    Ok(entries)
}

fn raw_text_input_files(path: &Path) -> Result<Vec<PathBuf>> {
    let raw_dir = if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    } else if path.file_name().is_some_and(|name| name == "raw") {
        path.to_path_buf()
    } else if path.join("scripts").join("raw").is_dir() {
        path.join("scripts").join("raw")
    } else if path.join("raw").is_dir() {
        path.join("raw")
    } else {
        anyhow::bail!(
            "{} is a raw text input directory, but no scripts/raw or raw directory exists",
            path.display()
        );
    };

    let mut files = Vec::new();
    collect_json_files(&raw_dir, &mut files)?;
    files.sort();
    anyhow::ensure!(
        !files.is_empty(),
        "{} 안에 raw JSON이 없음",
        raw_dir.display()
    );
    Ok(files)
}

fn stage_input_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }
    anyhow::ensure!(
        path.is_dir(),
        "stage 입력 경로가 파일/디렉토리가 아님: {}",
        path.display()
    );
    let mut files = Vec::new();
    collect_json_files(path, &mut files)?;
    files.sort();
    anyhow::ensure!(
        !files.is_empty(),
        "{} 안에 stage JSON이 없음",
        path.display()
    );
    Ok(files)
}

fn stage_name_for_path(path: &Path) -> Option<String> {
    let names = [
        "raw",
        "in_progress",
        "needs_review",
        "needs_human_review",
        "complete",
    ];
    let mut current = if path.is_file() {
        path.parent()
    } else {
        Some(path)
    };
    while let Some(dir) = current {
        if let Some(name) = dir.file_name().and_then(|name| name.to_str())
            && names.contains(&name)
        {
            return Some(name.to_string());
        }
        current = dir.parent();
    }
    None
}

fn validate_stage_entry(
    file_path: &Path,
    stage: &Option<String>,
    entry: &StageTranslationEntry,
    raw: &RawTextEntry,
    terms: &TermsFile,
    encoding: &KoEncoding,
) -> Result<()> {
    compare_protected_fields(file_path, entry, raw)?;
    validate_stage_status(file_path, stage.as_deref(), entry)?;
    validate_source_metadata(&entry.as_override())
        .with_context(|| format!("{}: source/control validation failed", entry.id))?;
    validate_no_japanese_residue(entry)?;
    validate_korean_punctuation(&entry.id, &entry.ko)?;
    validate_stage_terms(entry, terms)?;

    if !entry.skip && !entry.ko.trim().is_empty() {
        encode_translation_text(&entry.ko, encoding)
            .with_context(|| format!("{} 인코딩 실패", entry.id))?;
    }
    Ok(())
}

fn compare_protected_fields(
    file_path: &Path,
    entry: &StageTranslationEntry,
    raw: &RawTextEntry,
) -> Result<()> {
    compare_optional_field(
        file_path,
        &entry.id,
        "kind",
        entry.kind.as_deref(),
        raw.kind.as_deref(),
    )?;
    compare_optional_field(
        file_path,
        &entry.id,
        "region",
        entry.region.map(|value| value.to_string()).as_deref(),
        raw.region.map(|value| value.to_string()).as_deref(),
    )?;
    compare_optional_field(
        file_path,
        &entry.id,
        "index",
        entry.index.map(|value| value.to_string()).as_deref(),
        raw.index.map(|value| value.to_string()).as_deref(),
    )?;
    compare_optional_field(
        file_path,
        &entry.id,
        "offset",
        entry.offset.as_deref(),
        raw.offset.as_deref(),
    )?;
    compare_optional_field(
        file_path,
        &entry.id,
        "len",
        entry.len.map(|value| value.to_string()).as_deref(),
        raw.len.map(|value| value.to_string()).as_deref(),
    )?;
    compare_required_field(file_path, &entry.id, "jp", &entry.jp, &raw.jp)?;
    compare_required_field(
        file_path,
        &entry.id,
        "bytes_hex",
        &entry.bytes_hex,
        &raw.bytes_hex,
    )?;
    compare_required_field(
        file_path,
        &entry.id,
        "source_crc32",
        &entry.source_crc32,
        &raw.source_crc32,
    )?;
    Ok(())
}

fn compare_optional_field(
    file_path: &Path,
    id: &str,
    field: &str,
    actual: Option<&str>,
    expected: Option<&str>,
) -> Result<()> {
    if let Some(actual) = actual {
        anyhow::ensure!(
            expected == Some(actual),
            "{}: {} protected field mismatch in {}",
            id,
            field,
            file_path.display()
        );
    }
    Ok(())
}

fn compare_required_field(
    file_path: &Path,
    id: &str,
    field: &str,
    actual: &str,
    expected: &str,
) -> Result<()> {
    anyhow::ensure!(
        !actual.trim().is_empty(),
        "{}: {} is required in {}",
        id,
        field,
        file_path.display()
    );
    anyhow::ensure!(
        actual == expected,
        "{}: {} protected field mismatch in {}",
        id,
        field,
        file_path.display()
    );
    Ok(())
}

fn validate_stage_status(
    file_path: &Path,
    stage: Option<&str>,
    entry: &StageTranslationEntry,
) -> Result<()> {
    let status = entry.status.trim();
    let ko_empty = entry.ko.trim().is_empty();
    match stage {
        Some("raw") => {
            anyhow::ensure!(
                status == "untranslated",
                "{}: raw stage must keep status=untranslated",
                entry.id
            );
            anyhow::ensure!(ko_empty, "{}: raw stage must keep ko empty", entry.id);
        }
        Some("in_progress") => {
            anyhow::ensure!(
                matches!(status, "in_progress" | "done" | "needs_review"),
                "{}: invalid in_progress status {}",
                entry.id,
                status
            );
        }
        Some("needs_review") => {
            anyhow::ensure!(
                matches!(status, "done" | "needs_review"),
                "{}: needs_review stage requires status=done or needs_review",
                entry.id
            );
            anyhow::ensure!(!ko_empty, "{}: needs_review entry has empty ko", entry.id);
            anyhow::ensure!(
                !entry.notes.trim().is_empty(),
                "{}: needs_review entry must carry notes",
                entry.id
            );
        }
        Some("needs_human_review") => {
            anyhow::ensure!(
                status == "needs_human_review",
                "{}: needs_human_review stage requires matching status",
                entry.id
            );
            anyhow::ensure!(
                !ko_empty,
                "{}: needs_human_review entry has empty ko",
                entry.id
            );
            anyhow::ensure!(
                !entry.notes.trim().is_empty(),
                "{}: needs_human_review entry must carry notes",
                entry.id
            );
        }
        Some("complete") => {
            anyhow::ensure!(
                matches!(status, "complete" | "done"),
                "{}: complete stage requires status=complete or done",
                entry.id
            );
            anyhow::ensure!(!ko_empty, "{}: complete entry has empty ko", entry.id);
            anyhow::ensure!(
                !entry.notes.trim().is_empty(),
                "{}: complete entry must carry notes",
                entry.id
            );
        }
        _ => {
            anyhow::ensure!(
                !status.is_empty(),
                "{}: status is required in {}",
                entry.id,
                file_path.display()
            );
        }
    }
    Ok(())
}

fn validate_no_japanese_residue(entry: &StageTranslationEntry) -> Result<()> {
    if entry.skip || entry.ko.trim().is_empty() {
        return Ok(());
    }
    for ch in chars_outside_tags(&entry.ko) {
        anyhow::ensure!(
            !is_japanese_text_char(ch),
            "{}: ko contains Japanese residue '{}'",
            entry.id,
            ch
        );
    }
    Ok(())
}

fn validate_korean_punctuation(id: &str, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    anyhow::ensure!(
        !text.contains('、'),
        "{id}: ko uses Japanese comma '、'; normalize to Western comma ','"
    );
    anyhow::ensure!(
        !text.contains('。'),
        "{id}: ko uses Japanese period '。'; drop it or promote by context"
    );
    anyhow::ensure!(
        !text.contains('～') && !text.contains('〜'),
        "{id}: ko uses fullwidth/wave dash; normalize to ASCII tilde '~'"
    );
    anyhow::ensure!(
        !text.contains("...") && !text.contains("・・・") && !text.contains("．．．"),
        "{id}: ko uses multi-cell ellipsis; normalize to single U+2026 '…'"
    );
    Ok(())
}

fn validate_stage_terms(entry: &StageTranslationEntry, terms: &TermsFile) -> Result<()> {
    if entry.skip || entry.ko.trim().is_empty() {
        return Ok(());
    }
    let normalized_jp = normalize_term_text(&entry.jp);
    let normalized_ko = normalize_term_text(&entry.ko);
    for term in terms.entries.iter().filter(|term| term.is_stable()) {
        let appears = if term.match_refs_only {
            term.refs.iter().any(|term_ref| term_ref == &entry.id)
        } else {
            term.match_terms().any(|jp| {
                let normalized_term = normalize_term_text(jp);
                !normalized_term.is_empty() && normalized_jp.contains(&normalized_term)
            })
        };
        if appears {
            let normalized_approved = normalize_term_text(&term.ko);
            anyhow::ensure!(
                normalized_ko.contains(&normalized_approved),
                "{}: stable term {} must use {}",
                entry.id,
                term.jp,
                term.ko
            );
        }
    }
    Ok(())
}

fn chars_outside_tags(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut in_tag = false;
    for ch in text.chars() {
        if in_tag {
            if ch == ']' {
                in_tag = false;
            }
            continue;
        }
        if ch == '[' {
            in_tag = true;
            continue;
        }
        out.push(ch);
    }
    out
}

fn is_japanese_text_char(ch: char) -> bool {
    let code = ch as u32;
    (0x3040..=0x30ff).contains(&code) || (0x4e00..=0x9fff).contains(&code)
}

fn normalize_term_text(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        if in_tag {
            if ch == ']' {
                in_tag = false;
            }
            continue;
        }
        if ch == '[' {
            in_tag = true;
            continue;
        }
        if ch.is_whitespace() || ch == '\u{3000}' {
            continue;
        }
        out.push(ch);
    }
    out
}

fn validate_source_metadata(entry: &TranslationOverride) -> Result<()> {
    let _status = entry.status.trim();
    let _notes = entry.notes.trim();
    validate_control_tags(entry)?;

    if entry.bytes_hex.trim().is_empty() {
        anyhow::ensure!(
            entry.source_crc32.trim().is_empty(),
            "{}: source_crc32가 있으면 bytes_hex도 있어야 함",
            entry.id
        );
        return Ok(());
    }

    let bytes =
        parse_hex_bytes(&entry.bytes_hex).with_context(|| format!("{} bytes_hex", entry.id))?;
    if let Some(expected) = parse_optional_crc32(&entry.source_crc32)
        .with_context(|| format!("{} source_crc32", entry.id))?
    {
        let actual = crc32fast::hash(&bytes);
        anyhow::ensure!(
            actual == expected,
            "{} source_crc32 mismatch: metadata {expected:08X} vs bytes {actual:08X}",
            entry.id
        );
    }
    Ok(())
}

fn validate_control_tags(entry: &TranslationOverride) -> Result<()> {
    if entry.jp.trim().is_empty() || entry.ko.trim().is_empty() {
        return Ok(());
    }
    let jp = control_tag_counts(&entry.jp);
    let ko = control_tag_counts(&entry.ko);
    anyhow::ensure!(
        jp == ko,
        "{} control tag mismatch: jp {:?} vs ko {:?}",
        entry.id,
        jp,
        ko
    );
    let jp_sequence = control_tag_sequence(&entry.jp)?;
    let ko_sequence = control_tag_sequence(&entry.ko)?;
    anyhow::ensure!(
        jp_sequence == ko_sequence,
        "{} control tag order mismatch: jp {:?} vs ko {:?}",
        entry.id,
        jp_sequence,
        ko_sequence
    );
    Ok(())
}

fn control_tag_sequence(text: &str) -> Result<Vec<String>> {
    let mut sequence = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '[' {
            continue;
        }
        let tag: String = chars.by_ref().take_while(|&c| c != ']').collect();
        let (name, argument) = tag
            .split_once(':')
            .map(|(name, argument)| (name, Some(argument)))
            .unwrap_or((tag.as_str(), None));
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name == "money" {
            continue;
        }
        let normalized = match (name.as_str(), argument) {
            ("flags" | "raw", Some(argument)) => {
                format!("{name}:{:02x}", parse_hex_u8(argument)?)
            }
            (_, Some(argument)) => {
                format!("{name}:{}", argument.trim().to_ascii_lowercase())
            }
            (_, None) => name,
        };
        sequence.push(normalized);
    }
    Ok(sequence)
}

fn control_tag_counts(text: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '[' {
            continue;
        }
        let tag: String = chars.by_ref().take_while(|&c| c != ']').collect();
        let name = tag
            .split_once(':')
            .map(|(name, _)| name)
            .unwrap_or(tag.as_str())
            .to_ascii_lowercase();
        // [money] is a visible semantic token for the original 0x0C price
        // anchor, not a flow-control tag that must also occur in JP preview.
        if !name.is_empty() && name != "money" {
            *counts.entry(name).or_default() += 1;
        }
    }
    counts
}

fn source_guard(entry: &TranslationOverride) -> Result<Option<TranslationSource>> {
    if entry.bytes_hex.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(TranslationSource {
        bytes: parse_hex_bytes(&entry.bytes_hex)
            .with_context(|| format!("{} bytes_hex", entry.id))?,
        crc32: parse_optional_crc32(&entry.source_crc32)
            .with_context(|| format!("{} source_crc32", entry.id))?,
    }))
}

pub fn encode_translation_text(text: &str, encoding: &KoEncoding) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for token in parse_text(text)? {
        match token {
            TextToken::Char(ch) if is_ko_dynamic_char(ch) => {
                let code = encoding
                    .code_for(ch)
                    .with_context(|| format!("인코딩에 없는 KO 글리프: {ch}"))?;
                out.extend_from_slice(&code);
            }
            TextToken::Char(ch) => {
                let bytes = script::encode_jp_char(ch)
                    .with_context(|| format!("지원하지 않는 문자 '{ch}' (U+{:04X})", ch as u32))?;
                out.extend(bytes);
            }
            TextToken::Money => out.push(MONEY_TEXT_BYTE),
            TextToken::Br => out.push(0xFF),
            TextToken::Wait => out.push(0xFD),
            TextToken::End => out.push(0x00),
            TextToken::Flags(value) => {
                out.push(0xFE);
                out.push(value);
            }
            TextToken::Raw(value) => out.push(value),
        }
    }
    if out.last() != Some(&0x00) {
        out.push(0x00);
    }
    Ok(out)
}

fn parse_text(text: &str) -> Result<Vec<TextToken>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '[' {
            let tag: String = chars.by_ref().take_while(|&c| c != ']').collect();
            anyhow::ensure!(!tag.is_empty(), "빈 control tag");
            tokens.push(parse_tag(&tag)?);
        } else {
            tokens.push(TextToken::Char(ch));
        }
    }
    Ok(tokens)
}

fn parse_tag(tag: &str) -> Result<TextToken> {
    let lower = tag.to_ascii_lowercase();
    match lower.as_str() {
        "br" => Ok(TextToken::Br),
        "wait" => Ok(TextToken::Wait),
        "end" => Ok(TextToken::End),
        "money" => Ok(TextToken::Money),
        _ if lower.starts_with("flags:") => Ok(TextToken::Flags(parse_hex_u8(&tag[6..])?)),
        _ if lower.starts_with("raw:") => Ok(TextToken::Raw(parse_hex_u8(&tag[4..])?)),
        _ => anyhow::bail!("알 수 없는 control tag: [{tag}]"),
    }
}

fn parse_hex_u8(raw: &str) -> Result<u8> {
    let trimmed = raw.trim().trim_start_matches('$').trim_start_matches("0x");
    anyhow::ensure!(trimmed.len() <= 2, "u8 범위를 넘는 hex 값: {raw}");
    u8::from_str_radix(trimmed, 16).with_context(|| format!("hex 파싱 실패: {raw}"))
}

fn parse_optional_crc32(raw: &str) -> Result<Option<u32>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let hex = trimmed.trim_start_matches('$').trim_start_matches("0x");
    anyhow::ensure!(hex.len() == 8, "CRC32는 8자리 hex여야 함: {raw}");
    Ok(Some(
        u32::from_str_radix(hex, 16).with_context(|| format!("CRC32 파싱 실패: {raw}"))?,
    ))
}

fn parse_hex_bytes(raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    trimmed
        .split_whitespace()
        .map(|part| {
            anyhow::ensure!(part.len() == 2, "invalid byte token: {part}");
            u8::from_str_radix(part, 16).with_context(|| format!("invalid byte token: {part}"))
        })
        .collect()
}

fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn find_exact_glyph_matches(rom: &[u8], glyph_bytes: &[u8; glyph::GLYPH_BYTES]) -> Vec<usize> {
    rom.windows(glyph::GLYPH_BYTES)
        .enumerate()
        .filter_map(|(offset, window)| (window == glyph_bytes).then_some(offset))
        .collect()
}

fn slot2_logical_address(physical_offset: usize) -> u16 {
    (GG_SLOT2_BASE + (physical_offset % GG_ROM_BANK_SIZE)) as u16
}

fn is_hangul_syllable(ch: char) -> bool {
    let code = ch as u32;
    (0xAC00..=0xD7A3).contains(&code)
}

fn is_ko_dynamic_char(ch: char) -> bool {
    is_hangul_syllable(ch) || KO_DYNAMIC_PUNCTUATION.contains(&ch)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextToken {
    Char(char),
    Money,
    Br,
    Wait,
    End,
    Flags(u8),
    Raw(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_metadata_required_rejects_source_less_entry() {
        let encoded = vec![EncodedOverride {
            id: "region/0/000".to_string(),
            bytes: vec![1, 2],
            source: None,
        }];
        assert!(require_source_metadata(&encoded).is_err());
    }

    #[test]
    fn source_metadata_required_accepts_entry_with_source() {
        let encoded = vec![EncodedOverride {
            id: "region/0/000".to_string(),
            bytes: vec![1, 2],
            source: Some(TranslationSource {
                bytes: vec![1, 2],
                crc32: None,
            }),
        }];
        require_source_metadata(&encoded).unwrap();
    }

    #[test]
    fn encodes_hangul_with_frequency_mapping_and_jp_controls() {
        let encoding = KoEncoding::from_texts(&["마도마".to_string()]).unwrap();
        assert_eq!(encoding.glyph_chars(), &['마', '도']);

        let bytes = encode_translation_text("お[br]마도[end]", &encoding).unwrap();
        assert_eq!(
            bytes,
            vec![
                0x13,
                0xFF,
                glyph::KO_PREFIX,
                0x01,
                glyph::KO_PREFIX,
                0x02,
                0x00
            ]
        );
    }

    #[test]
    fn semantic_money_token_encodes_as_original_price_anchor() {
        let encoding = KoEncoding::from_texts(&["[money][end]".to_string()]).unwrap();
        let bytes = encode_translation_text("[money][end]", &encoding).unwrap();
        assert_eq!(bytes, vec![MONEY_TEXT_BYTE, 0x00]);
        assert_eq!(
            control_tag_counts("[money][br][end]"),
            control_tag_counts("金[br][end]")
        );
    }

    #[test]
    fn literal_money_kanji_is_rejected_as_korean_residue() {
        let entry: StageTranslationEntry = serde_json::from_str(
            r#"{
  "id": "shop/test",
  "ko": "金이　모자라요[end]",
  "status": "needs_human_review",
  "notes": "test"
}"#,
        )
        .unwrap();
        let err = validate_no_japanese_residue(&entry).unwrap_err();
        assert!(err.to_string().contains("Japanese residue '金'"));
    }

    #[test]
    fn western_comma_and_tilde_use_dynamic_glyphs_without_reordering_hangul() {
        let encoding = KoEncoding::from_texts(&["마도마,~".to_string()]).unwrap();
        assert_eq!(encoding.glyph_chars(), &['마', '도', ',', '~']);

        let bytes = encode_translation_text("마,도~[end]", &encoding).unwrap();
        assert_eq!(
            bytes,
            vec![
                glyph::KO_PREFIX,
                0x01,
                glyph::KO_PREFIX,
                0x03,
                glyph::KO_PREFIX,
                0x02,
                glyph::KO_PREFIX,
                0x04,
                0x00
            ]
        );
    }

    #[test]
    fn korean_punctuation_policy_rejects_japanese_and_multi_cell_forms() {
        assert!(validate_korean_punctuation("x", "하지만、[end]").is_err());
        assert!(validate_korean_punctuation("x", "끝났다。[end]").is_err());
        assert!(validate_korean_punctuation("x", "그래서〜[end]").is_err());
        assert!(validate_korean_punctuation("x", "잠깐...[end]").is_err());
        validate_korean_punctuation("x", "하지만, 잠깐… 정말~[end]").unwrap();
    }

    #[test]
    fn visible_line_ceiling_accepts_twenty_tiles_and_rejects_twenty_one() {
        let twenty = format!("{}[end]", "가".repeat(20));
        let twenty_one = format!("{}[end]", "가".repeat(21));
        validate_visible_line_ceiling("region/test", &twenty, 20).unwrap();
        let err = validate_visible_line_ceiling("region/test", &twenty_one, 20).unwrap_err();
        assert!(err.to_string().contains("exceeds 20 tiles"));
    }

    #[test]
    fn shop_layout_accepts_exact_anchor_and_price_padding_preservation() {
        let raw = [0x2A, 0x2A, 0xFF, 0x0C, 0x01, 0x01, 0x22, 0x00];
        let encoded = [0xA3, 0x01, 0xFF, 0x0C, 0x01, 0x01, 0xA3, 0x02];
        let mut encoded = encoded.to_vec();
        encoded.push(0x00);

        let mut raw = raw.to_vec();
        raw.insert(raw.len() - 1, 0x01);
        validate_shop_layout(
            "shop/test",
            "ふふ[br]金　　あ　[end]",
            "가[br][money]　　나[end]",
            &raw,
            &encoded,
            9,
        )
        .unwrap();
    }

    #[test]
    fn shop_layout_rejects_moved_money_anchor() {
        let raw = [0x2A, 0xFF, 0x0C, 0x01, 0x01, 0x22, 0x00];
        let encoded = [0x2A, 0xFF, 0x01, 0x0C, 0x01, 0x01, 0x00];

        let err = validate_shop_layout(
            "shop/test",
            "ふ[br]金　　あ[end]",
            "후[br]　[money]　　[end]",
            &raw,
            &encoded,
            raw.len(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("money moved right"));
    }

    #[test]
    fn shop_layout_rejects_shorter_fixed_length_translation() {
        let raw = [0x2A, 0xFF, 0x0C, 0x01, 0x01, 0x22, 0x00];
        let encoded = [0x2A, 0xFF, 0x0C, 0x01, 0x22, 0x00];

        let err = validate_shop_layout(
            "shop/test",
            "ふ[br]金　　あ[end]",
            "후[br][money]　아[end]",
            &raw,
            &encoded,
            raw.len(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("preserve exact source len"));
    }

    #[test]
    fn rejects_unknown_control_tag() {
        let encoding = KoEncoding::from_texts(&[]).unwrap();
        assert!(encode_translation_text("[bad]", &encoding).is_err());
    }

    #[test]
    fn multi_prefix_assigns_prefix_by_frequency_rank() {
        // rank 0..249 → prefix 0xA3, 250..499 → 0xA4. base = 0x01 + rank%250.
        assert_eq!(code_for_rank(0), [glyph::KO_PREFIXES[0], 0x01]);
        assert_eq!(code_for_rank(249), [glyph::KO_PREFIXES[0], 0xFA]);
        assert_eq!(code_for_rank(250), [glyph::KO_PREFIXES[1], 0x01]);
        assert_eq!(code_for_rank(500), [glyph::KO_PREFIXES[2], 0x01]);
        // glyph# 복원: prefix_index*PER + (base-1) == rank
        for rank in [0usize, 1, 249, 250, 499, 500, 734] {
            let [p, b] = code_for_rank(rank);
            let pidx = glyph::KO_PREFIXES.iter().position(|&x| x == p).unwrap();
            assert_eq!(pidx * glyph::PER_PREFIX + (b as usize - 1), rank);
        }
    }

    #[test]
    fn defer_glyph_cap_accepts_over_capacity_and_keeps_length() {
        // 다중 프리픽스 용량(1000) 초과 distinct 한글: enforce는 실패, defer는 통과.
        let n = glyph::KO_MULTI_PREFIX_CAP as u32 + 100;
        let over: String = (0..n)
            .map(|i| char::from_u32(0xAC00 + i).unwrap())
            .collect();
        let texts = vec![over.clone()];

        assert!(KoEncoding::from_texts(&texts).is_err());
        let encoding = KoEncoding::from_texts_capped(&texts, false).unwrap();
        assert_eq!(encoding.glyph_count(), n as usize);

        // 초과분도 prefix가 순환 배정돼 인코딩되고, 한글은 code 값과 무관하게 2바이트다.
        let bytes = encode_translation_text(&over, &encoding).unwrap();
        assert_eq!(bytes.len(), n as usize * 2 + 1); // 각 음절 2바이트 + 말미 0x00
    }

    #[test]
    fn full_format_preserves_source_guard() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("translation.json");
        std::fs::write(
            &path,
            r#"{
  "format": "madoua-translation-v1",
  "entries": [
    {
      "id": "region/0/001",
      "jp": "ほのおで[br]とても[br]あついよ。[wait][end]",
      "bytes_hex": "2C 27 13 FC 21 FF 22 21 31 FF 0F 20 10 34 0E FD 00",
      "source_crc32": "F23A4069",
      "ko": "마도[br]마도[br]마도[wait][end]",
      "status": "done",
      "notes": "test"
    }
  ]
}"#,
        )
        .unwrap();

        let plan = load_translation_plan(&path).unwrap();
        assert_eq!(plan.encoded.len(), 1);
        let source = plan.encoded[0].source.as_ref().unwrap();
        assert_eq!(source.bytes.len(), 17);
        assert_eq!(source.crc32, Some(0xF23A_4069));
    }

    #[test]
    fn rejects_source_crc_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("translation.json");
        std::fs::write(
            &path,
            r#"{
  "entries": [
    {
      "id": "cutscene/002",
      "bytes_hex": "01 00",
      "source_crc32": "00000000",
      "ko": "마도[end]"
    }
  ]
}"#,
        )
        .unwrap();

        let err = load_translation_plan(&path).unwrap_err();
        assert!(err.to_string().contains("source_crc32 mismatch"));
    }

    #[test]
    fn rejects_control_tag_mismatch_when_jp_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("translation.json");
        std::fs::write(
            &path,
            r#"{
  "entries": [
    {
      "id": "cutscene/002",
      "jp": "お[br]ば[end]",
      "ko": "마도[end]"
    }
  ]
}"#,
        )
        .unwrap();

        let err = load_translation_plan(&path).unwrap_err();
        assert!(err.to_string().contains("control tag mismatch"));
    }

    #[test]
    fn rejects_reordered_control_tags_even_when_counts_match() {
        let entry = TranslationOverride {
            id: "cutscene/reordered".to_string(),
            jp: "お[br]ば[wait][end]".to_string(),
            ko: "마[wait]도[br][end]".to_string(),
            bytes_hex: String::new(),
            source_crc32: String::new(),
            status: String::new(),
            notes: String::new(),
            skip: false,
        };
        let err = validate_control_tags(&entry).unwrap_err();
        assert!(err.to_string().contains("control tag order mismatch"));
    }

    #[test]
    fn directory_input_reads_only_scripts_complete() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("scripts").join("raw");
        let complete = dir.path().join("scripts").join("complete");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::create_dir_all(&complete).unwrap();
        std::fs::write(
            raw.join("bad.json"),
            r#"{
  "entries": [
    {
      "id": "raw/should-not-load",
      "ko": "[bad]"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            complete.join("good.json"),
            r#"{
  "entries": [
    {
      "id": "cutscene/002",
      "ko": "마도[end]"
    }
  ]
}"#,
        )
        .unwrap();

        let plan = load_translation_plan(dir.path()).unwrap();
        assert_eq!(plan.encoded.len(), 1);
        assert_eq!(plan.encoded[0].id, "cutscene/002");
    }

    #[test]
    fn directory_input_requires_complete_stage() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts").join("raw")).unwrap();

        let err = load_translation_plan(dir.path()).unwrap_err();
        assert!(err.to_string().contains("scripts/complete"));
    }

    #[test]
    fn human_review_preview_is_explicit_and_status_checked() {
        let dir = tempfile::tempdir().unwrap();
        let stage = dir.path().join("scripts").join("needs_human_review");
        std::fs::create_dir_all(&stage).unwrap();
        let file = stage.join("candidate.json");
        std::fs::write(
            &file,
            r#"{
  "entries": [
    {
      "id": "cutscene/002",
      "ko": "마도[end]",
      "status": "needs_human_review"
    }
  ]
}"#,
        )
        .unwrap();

        let default_err = load_translation_plan(dir.path()).unwrap_err();
        assert!(default_err.to_string().contains("scripts/complete"));
        let preview = load_human_review_preview_plan(dir.path()).unwrap();
        assert_eq!(preview.encoded.len(), 1);

        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, text.replace("needs_human_review", "needs_review")).unwrap();
        let status_err = load_human_review_preview_plan(dir.path()).unwrap_err();
        assert!(
            status_err
                .to_string()
                .contains("requires status=needs_human_review")
        );
    }

    #[test]
    fn check_terms_accepts_raw_stage_refs_and_aliases() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("scripts").join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(
            raw.join("region_01.json"),
            r#"{
  "entries": [
    {
      "id": "region/1/00E",
      "jp": "『ダイア[br]　　キュート』の　じゅもん[wait][end]"
    }
  ]
}"#,
        )
        .unwrap();
        let terms = dir.path().join("terms.json");
        std::fs::write(
            &terms,
            r#"{
  "format": "madoua-terms-v1",
  "entries": [
    {
      "category": "spell",
      "jp": "ダイアキュート",
      "ko": "다이아큐트",
      "status": "approved_series",
      "aliases": ["ダイヤキュート"],
      "refs": ["region/1/00E"]
    }
  ]
}"#,
        )
        .unwrap();

        cmd_check_terms(&terms, dir.path()).unwrap();
    }

    #[test]
    fn check_terms_rejects_missing_or_stale_refs() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(
            raw.join("region_00.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "jp": "ファイヤー[end]"
    }
  ]
}"#,
        )
        .unwrap();
        let terms = dir.path().join("terms.json");
        std::fs::write(
            &terms,
            r#"{
  "format": "madoua-terms-v1",
  "entries": [
    {
      "category": "spell",
      "jp": "アイスストーム",
      "ko": "아이스스톰",
      "status": "approved_series",
      "refs": ["region/0/003"]
    }
  ]
}"#,
        )
        .unwrap();

        let err = cmd_check_terms(&terms, dir.path()).unwrap_err();
        assert!(err.to_string().contains("does not contain"));
    }

    #[test]
    fn check_stage_accepts_needs_review_entry_with_stable_term() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("scripts").join("raw");
        let needs_review = dir.path().join("scripts").join("needs_review");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::create_dir_all(&needs_review).unwrap();
        std::fs::write(
            raw.join("region_00.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "kind": "region",
      "region": 0,
      "index": 3,
      "offset": "0x1C066",
      "len": 6,
      "jp": "ファイヤー[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1",
      "ko": "",
      "status": "untranslated",
      "notes": ""
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            needs_review.join("batch.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "kind": "region",
      "region": 0,
      "index": 3,
      "offset": "0x1C066",
      "len": 6,
      "jp": "ファイヤー[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1",
      "ko": "파이어[end]",
      "status": "needs_review",
      "notes": "stable spell term"
    }
  ]
}"#,
        )
        .unwrap();
        let terms = dir.path().join("terms.json");
        std::fs::write(
            &terms,
            r#"{
  "format": "madoua-terms-v1",
  "entries": [
    {
      "category": "spell",
      "jp": "ファイヤー",
      "ko": "파이어",
      "status": "approved_series",
      "refs": ["region/0/003"]
    }
  ]
}"#,
        )
        .unwrap();

        cmd_check_stage(&needs_review, dir.path(), &terms, false).unwrap();
    }

    #[test]
    fn stable_ref_only_term_avoids_substring_collisions() {
        let terms = TermsFile {
            format: "madoua-terms-v1".to_string(),
            entries: vec![TermEntry {
                category: "character".to_string(),
                jp: "リン".to_string(),
                ko: "린".to_string(),
                status: "project_decision".to_string(),
                source: String::new(),
                notes: String::new(),
                aliases: Vec::new(),
                refs: vec!["region/6/03D".to_string()],
                match_refs_only: true,
            }],
        };
        let entry = |id: &str, jp: &str, ko: &str| StageTranslationEntry {
            id: id.to_string(),
            kind: None,
            region: None,
            index: None,
            offset: None,
            len: None,
            jp: jp.to_string(),
            bytes_hex: String::new(),
            source_crc32: String::new(),
            ko: ko.to_string(),
            status: "needs_review".to_string(),
            notes: "test".to_string(),
            skip: false,
        };

        validate_stage_terms(
            &entry("region/0/007", "ヒーリング[end]", "힐링[end]"),
            &terms,
        )
        .unwrap();
        validate_stage_terms(
            &entry("region/6/03D", "リンちゃん[end]", "린이야[end]"),
            &terms,
        )
        .unwrap();
        let err = validate_stage_terms(
            &entry("region/6/03D", "リンちゃん[end]", "요정이야[end]"),
            &terms,
        )
        .unwrap_err();
        assert!(err.to_string().contains("must use 린"));
    }

    #[test]
    fn check_stage_rejects_protected_field_change() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        let needs_review = dir.path().join("needs_review");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::create_dir_all(&needs_review).unwrap();
        std::fs::write(
            raw.join("region_00.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "jp": "ファイヤー[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            needs_review.join("batch.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "jp": "ヒーリング[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1",
      "ko": "파이어[end]",
      "status": "needs_review",
      "notes": "bad protected field"
    }
  ]
}"#,
        )
        .unwrap();
        let terms = dir.path().join("terms.json");
        std::fs::write(
            &terms,
            r#"{
  "format": "madoua-terms-v1",
  "entries": [
    {
      "category": "spell",
      "jp": "ファイヤー",
      "ko": "파이어",
      "status": "approved_series",
      "refs": ["region/0/003"]
    }
  ]
}"#,
        )
        .unwrap();

        let err = cmd_check_stage(&needs_review, dir.path(), &terms, false).unwrap_err();
        assert!(err.to_string().contains("protected field mismatch"));
    }

    #[test]
    fn check_stage_rejects_stable_term_drift() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        let needs_review = dir.path().join("needs_review");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::create_dir_all(&needs_review).unwrap();
        std::fs::write(
            raw.join("region_00.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "jp": "ファイヤー[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1"
    }
  ]
}"#,
        )
        .unwrap();
        std::fs::write(
            needs_review.join("batch.json"),
            r#"{
  "entries": [
    {
      "id": "region/0/003",
      "jp": "ファイヤー[end]",
      "bytes_hex": "61 74 47 69 82 00",
      "source_crc32": "364851B1",
      "ko": "불꽃[end]",
      "status": "needs_review",
      "notes": "term drift"
    }
  ]
}"#,
        )
        .unwrap();
        let terms = dir.path().join("terms.json");
        std::fs::write(
            &terms,
            r#"{
  "format": "madoua-terms-v1",
  "entries": [
    {
      "category": "spell",
      "jp": "ファイヤー",
      "ko": "파이어",
      "status": "approved_series",
      "refs": ["region/0/003"]
    }
  ]
}"#,
        )
        .unwrap();

        let err = cmd_check_stage(&needs_review, dir.path(), &terms, false).unwrap_err();
        assert!(err.to_string().contains("stable term"));
    }

    #[test]
    fn check_surfaces_accepts_complete_shop_coverage_and_unverified_hud() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json(12)).unwrap();
        let catalog = dir.path().join("surfaces.json");
        std::fs::write(&catalog, surface_catalog_json(12)).unwrap();

        cmd_check_surfaces(&catalog, dir.path(), false).unwrap();
    }

    #[test]
    fn check_surfaces_rejects_missing_shop_surface() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json(12)).unwrap();
        let catalog = dir.path().join("surfaces.json");
        std::fs::write(&catalog, surface_catalog_json(11)).unwrap();

        let err = cmd_check_surfaces(&catalog, dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("shop/11"));
    }

    #[test]
    fn check_surfaces_release_readiness_rejects_blockers() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json(12)).unwrap();
        let catalog = dir.path().join("surfaces.json");
        std::fs::write(&catalog, surface_catalog_json(12)).unwrap();

        let err = cmd_check_surfaces(&catalog, dir.path(), true).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("surface release readiness failed"));
        assert!(err.contains("shop/00"));
        assert!(err.contains("hud/money-unit"));
    }

    #[test]
    fn check_surfaces_release_readiness_accepts_verified_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json(12)).unwrap();
        let catalog = dir.path().join("surfaces.json");
        std::fs::write(&catalog, surface_catalog_release_ready_json(12)).unwrap();

        cmd_check_surfaces(&catalog, dir.path(), true).unwrap();
    }

    #[test]
    fn check_money_sources_accepts_shop_money_byte_and_font_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json(12)).unwrap();

        let mut rom = vec![0u8; glyph::FONT_BASE + glyph::GLYPH_BYTES * 16];
        let off = glyph::FONT_BASE + glyph::font_index(MONEY_TEXT_BYTE) * glyph::GLYPH_BYTES;
        rom[off..off + glyph::GLYPH_BYTES]
            .copy_from_slice(&[0x18, 0x7E, 0x3C, 0xFF, 0x18, 0x3C, 0x66, 0x00]);
        let duplicate = off + glyph::GLYPH_BYTES * 3;
        rom[duplicate..duplicate + glyph::GLYPH_BYTES]
            .copy_from_slice(&[0x18, 0x7E, 0x3C, 0xFF, 0x18, 0x3C, 0x66, 0x00]);
        let rom_path = dir.path().join("rom.gg");
        std::fs::write(&rom_path, rom).unwrap();

        cmd_check_money_sources(&rom_path, dir.path()).unwrap();
    }

    #[test]
    fn money_glyph_inventory_finds_all_exact_matches() {
        let mut rom = vec![0u8; 0x20000];
        let glyph = [0x10, 0x28, 0x7C, 0x92, 0x7C, 0x10, 0x54, 0xFE];
        rom[0x19B55..0x19B55 + glyph::GLYPH_BYTES].copy_from_slice(&glyph);
        rom[0x1A000..0x1A000 + glyph::GLYPH_BYTES].copy_from_slice(&glyph);

        assert_eq!(
            find_exact_glyph_matches(&rom, &glyph),
            vec![0x19B55, 0x1A000]
        );
        assert_eq!(slot2_logical_address(0x19B55), 0x9B55);
        assert_eq!(slot2_logical_address(0x1A000), 0xA000);
    }

    #[test]
    fn check_money_sources_rejects_shop_without_money_byte() {
        let dir = tempfile::tempdir().unwrap();
        let raw = dir.path().join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        std::fs::write(raw.join("shop.json"), shop_raw_json_without_money(12)).unwrap();

        let mut rom = vec![0u8; glyph::FONT_BASE + glyph::GLYPH_BYTES * 16];
        let off = glyph::FONT_BASE + glyph::font_index(MONEY_TEXT_BYTE) * glyph::GLYPH_BYTES;
        rom[off..off + glyph::GLYPH_BYTES]
            .copy_from_slice(&[0x18, 0x7E, 0x3C, 0xFF, 0x18, 0x3C, 0x66, 0x00]);
        let rom_path = dir.path().join("rom.gg");
        std::fs::write(&rom_path, rom).unwrap();

        let err = cmd_check_money_sources(&rom_path, dir.path()).unwrap_err();
        assert!(err.to_string().contains("money text byte"));
    }

    fn shop_raw_json(count: usize) -> String {
        let mut entries = Vec::new();
        for i in 0..count {
            entries.push(format!(
                r#"{{
      "id": "shop/{i:02}",
      "kind": "shop",
      "index": {i},
      "offset": "0x25BB7",
      "len": 23,
      "jp": "金[end]",
      "bytes_hex": "0C 00",
      "source_crc32": "00000000"
    }}"#
            ));
        }
        format!(
            r#"{{
  "entries": [
    {}
  ]
}}"#,
            entries.join(",\n    ")
        )
    }

    fn shop_raw_json_without_money(count: usize) -> String {
        let mut entries = Vec::new();
        for i in 0..count {
            entries.push(format!(
                r#"{{
      "id": "shop/{i:02}",
      "kind": "shop",
      "index": {i},
      "offset": "0x25BB7",
      "len": 23,
      "jp": "ふふふ[end]",
      "bytes_hex": "2A 2A 2A 00",
      "source_crc32": "00000000"
    }}"#
            ));
        }
        format!(
            r#"{{
  "entries": [
    {}
  ]
}}"#,
            entries.join(",\n    ")
        )
    }

    fn surface_catalog_json(shop_count: usize) -> String {
        let mut surfaces = Vec::new();
        for i in 0..shop_count {
            surfaces.push(format!(
                r#"{{
      "id": "shop/{i:02}",
      "kind": "shop_text",
      "source_ref": "shop/{i:02}",
      "policy": "fixed_len_only",
      "status": "blocked_until_policy",
      "risks": ["money_placeholder"],
      "notes": "length-fixed shop surface"
    }}"#
            ));
        }
        surfaces.push(
            r#"{
      "id": "hud/money-unit",
      "kind": "hud_or_graphics_text",
      "source_ref": null,
      "policy": "inventory_required",
      "status": "unverified",
      "risks": ["fresh_scene_required"],
      "notes": "requires fresh-scene inventory"
    }"#
            .to_string(),
        );
        format!(
            r#"{{
  "format": "madoua-surface-inventory-v1",
  "surfaces": [
    {}
  ]
}}"#,
            surfaces.join(",\n    ")
        )
    }

    fn surface_catalog_release_ready_json(shop_count: usize) -> String {
        let mut surfaces = Vec::new();
        for i in 0..shop_count {
            surfaces.push(format!(
                r#"{{
      "id": "shop/{i:02}",
      "kind": "shop_text",
      "source_ref": "shop/{i:02}",
      "policy": "fixed_len_only",
      "status": "verified",
      "risks": [],
      "notes": "length-fixed shop surface has release evidence"
    }}"#
            ));
        }
        surfaces.push(
            r#"{
      "id": "hud/money-unit",
      "kind": "hud_or_graphics_text",
      "source_ref": null,
      "policy": "normal_text",
      "status": "verified",
      "risks": [],
      "notes": "money unit runtime path has release evidence"
    }"#
            .to_string(),
        );
        format!(
            r#"{{
  "format": "madoua-surface-inventory-v1",
  "surfaces": [
    {}
  ]
}}"#,
            surfaces.join(",\n    ")
        )
    }
}
