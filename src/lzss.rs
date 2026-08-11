//! 게임 고유 LZSS 코덱 (UI 그래픽 압축).
//!
//! A의 UI 그래픽(나침반·메뉴 버튼·타이틀 등)은 이 LZSS로 압축돼 런타임에 디컴프 후 VRAM에
//! 업로드된다. 이 모듈은 코덱을 양방향으로 소유해, 한글로 재조판한 4bpp 타일을 **같은 압축
//! 포맷으로 재압축**해 빌드 시점에 결정적으로 패치할 수 있게 한다(디컴프→재조판→재압축→
//! in-place 또는 같은 뱅크 relocate). ASM 훅이 필요 없다.
//!
//! 자매 `gg_madou_3`의 `ui_graphics.rs` 코덱을 이식했다(포맷이 동일함을 A의 실제 압축
//! 블록 10개 라운드트립으로 확인 — `docs/journal/2026-07-06-track-b-lzss-codec.md`). EN
//! 패치의 `madou_decmp`(`libsms MadouCmp::decmpMadou`)와도 동일한 토큰 스트림이다.
//!
//! 토큰 스트림:
//! - control `0x00`        → 종료
//! - control `0x01..=0x7F` → 리터럴 런: 소스에서 `ctrl`바이트 그대로 복사
//! - control `0x80..=0xFF` → 백레퍼런스: `len = (ctrl & 0x7F) + 3`, 이어서 distance
//!   바이트 `d`; `out[pos - (d + 1)]`부터 `len`바이트 바이트단위 복사(overlap 허용,
//!   RLE식). 버퍼 시작 이전 참조는 `0x00`으로 디코드된다.

/// 백레퍼런스 최대 길이: `0x7F + 3`.
const MAX_MATCH: usize = 130;
/// 리터럴 런 최대 길이(control 바이트가 `< 0x80` 이고 `!= 0`).
const MAX_LITERAL: usize = 0x7F;
/// 백레퍼런스 최대 거리: distance 바이트 `0..=255`가 `d + 1`을 인코딩.
const MAX_DIST: usize = 256;
const MIN_MATCH: usize = 3;

/// `start`부터 LZSS 블록을 디컴프한다. `(decompressed, consumed)` 반환 — `consumed`는
/// `0x00` 종료 바이트를 포함해 읽은 소스 바이트 수다.
pub fn decompress(data: &[u8], start: usize) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < data.len() {
        let ctrl = data[i];
        i += 1;
        if ctrl == 0x00 {
            break;
        }
        if ctrl < 0x80 {
            let n = ctrl as usize;
            let end = (i + n).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else {
            let len = (ctrl & 0x7F) as usize + MIN_MATCH;
            if i >= data.len() {
                break;
            }
            let dist = data[i] as usize;
            i += 1;
            let src = out.len() as isize - (dist as isize + 1);
            for k in 0..len {
                let sp = src + k as isize;
                let b = if sp >= 0 { out[sp as usize] } else { 0x00 };
                out.push(b);
            }
        }
    }
    (out, i - start)
}

/// `[pos-MAX_DIST, pos)` 안에서 `data[pos..]`의 가장 긴 백레퍼런스 매치. 전체 `data`와
/// 비교해 디코더의 overlap 시맨틱을 정확히 모델링한다.
fn find_match(data: &[u8], pos: usize) -> (usize, usize) {
    let (mut best_len, mut best_dist) = (0usize, 0usize);
    let lo = pos.saturating_sub(MAX_DIST);
    for src in lo..pos {
        let dist = pos - src;
        let mut l = 0usize;
        while l < MAX_MATCH && pos + l < data.len() && data[src + l] == data[pos + l] {
            l += 1;
        }
        if l > best_len {
            best_len = l;
            best_dist = dist;
            if l == MAX_MATCH {
                break;
            }
        }
    }
    (best_len, best_dist)
}

/// 최적(최단 출력) 파싱으로 압축한다. `decompress(compress(x)) == x`가 항상 성립하고,
/// 출력은 원본 압축 크기 이하다(A의 원본 압축기가 다소 suboptimal이라 바이트가 정확히
/// 같지는 않지만 무손실이며 in-place fit이 가능하다).
pub fn compress(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    #[derive(Clone, Copy)]
    enum Tok {
        Lit(usize),
        Match(usize, usize),
    }
    let mut cost = vec![usize::MAX; n + 1];
    let mut choice: Vec<Option<Tok>> = vec![None; n + 1];
    cost[n] = 1; // 종료 바이트
    for i in (0..n).rev() {
        let (blen, bdist) = find_match(data, i);
        let mut best = usize::MAX;
        let mut best_tok = None;
        if blen >= MIN_MATCH {
            for l in MIN_MATCH..=blen {
                let c = cost[i + l].saturating_add(2);
                if c < best {
                    best = c;
                    best_tok = Some(Tok::Match(l, bdist));
                }
            }
        }
        let max_l = MAX_LITERAL.min(n - i);
        for l in 1..=max_l {
            let c = cost[i + l].saturating_add(1 + l);
            if c < best {
                best = c;
                best_tok = Some(Tok::Lit(l));
            }
        }
        cost[i] = best;
        choice[i] = best_tok;
    }
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        match choice[i].expect("no encoding choice") {
            Tok::Match(l, d) => {
                out.push(0x80 | ((l - MIN_MATCH) as u8));
                out.push((d - 1) as u8);
                i += l;
            }
            Tok::Lit(l) => {
                out.push(l as u8);
                out.extend_from_slice(&data[i..i + l]);
                i += l;
            }
        }
    }
    out.push(0x00);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_literal_run() {
        // [len=3][A B C][end]
        let (out, consumed) = decompress(&[0x03, 0xAA, 0xBB, 0xCC, 0x00], 0);
        assert_eq!(out, vec![0xAA, 0xBB, 0xCC]);
        assert_eq!(consumed, 5);
    }

    #[test]
    fn decompress_backref_rle_overlap() {
        // 리터럴 1바이트 [0x11], 그다음 dist=0(직전 바이트) len=3 백레퍼런스 → 0x11 반복.
        // ctrl 0x80 = backref len=(0)+3, dist byte 0x00 = distance 1.
        let (out, _) = decompress(&[0x01, 0x11, 0x80, 0x00, 0x00], 0);
        assert_eq!(out, vec![0x11, 0x11, 0x11, 0x11]);
    }

    #[test]
    fn decompress_pre_buffer_reads_zero() {
        // 첫 토큰이 백레퍼런스면 버퍼가 비어 있어 src<0 → 0x00으로 채운다.
        let (out, _) = decompress(&[0x80, 0x00, 0x00], 0);
        assert_eq!(out, vec![0x00, 0x00, 0x00]);
    }

    #[test]
    fn compress_is_lossless_roundtrip() {
        let cases: &[&[u8]] = &[
            &[],
            &[0x42],
            &[1, 2, 3, 4, 5],
            &[7, 7, 7, 7, 7, 7, 7, 7, 7, 7], // RLE
            &[1, 2, 3, 1, 2, 3, 1, 2, 3, 9, 9, 1, 2, 3],
            &[0u8; 300],  // 긴 0 런 (백레퍼런스 chaining)
            &[0xAB; 130], // MAX_MATCH 경계
        ];
        for data in cases {
            let comp = compress(data);
            let (dec, _) = decompress(&comp, 0);
            assert_eq!(&dec, data, "round-trip mismatch for {data:?}");
        }
    }

    #[test]
    fn compress_output_ends_with_terminator() {
        let comp = compress(&[1, 2, 3]);
        assert_eq!(*comp.last().unwrap(), 0x00);
    }
}
