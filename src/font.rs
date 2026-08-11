//! TTF → 8×8 1bpp 글리프 렌더 파이프라인.
//!
//! 픽셀 폰트(Galmuri7 등)를 8×8 셀에 래스터라이즈·이진화해 글리프당 8바이트
//! (행당 1바이트, MSB=좌측, 1=전경)를 만든다. 게임 폰트와 동일 포맷이라
//! 0x9A3E 디컴프 경로로 그대로 렌더된다.

use anyhow::{Context, Result};
use fontdue::{Font, FontSettings};
use std::path::Path;

pub struct GlyphFont {
    font: Font,
    px: f32,
    threshold: u8,
    x_off: i32,
    y_off: i32,
}

impl GlyphFont {
    pub fn load(path: &Path, px: f32, threshold: u8, x_off: i32, y_off: i32) -> Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("폰트 읽기 실패: {}", path.display()))?;
        let font = Font::from_bytes(bytes, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("폰트 파싱 실패: {e}"))?;
        Ok(Self {
            font,
            px,
            threshold,
            x_off,
            y_off,
        })
    }

    /// 문자 하나를 8×8 1bpp(8바이트)로 렌더한다.
    pub fn render(&self, ch: char) -> [u8; 8] {
        let (m, cov) = self.font.rasterize(ch, self.px);
        let mut out = [0u8; 8];
        // 셀 안 가로·세로 중앙 정렬(gg_madou_1식). y_off는 중앙 기준 추가 미세 조정.
        // 고정 top 앵커(옛 방식)는 받침 있는 8행 글리프의 아래를 잘랐다.
        let base_x = (8 - m.width as i32) / 2 + self.x_off;
        let base_y = (8 - m.height as i32) / 2 + self.y_off;
        for gy in 0..m.height {
            for gx in 0..m.width {
                if cov[gy * m.width + gx] < self.threshold {
                    continue;
                }
                let cx = base_x + gx as i32;
                let cy = base_y + gy as i32;
                if (0..8).contains(&cx) && (0..8).contains(&cy) {
                    out[cy as usize] |= 1 << (7 - cx);
                }
            }
        }
        out
    }
}

/// 8×8 1bpp를 ASCII 아트로.
pub fn to_ascii(g: &[u8; 8]) -> String {
    let mut s = String::new();
    for &b in g {
        for c in 0..8 {
            s.push(if (b >> (7 - c)) & 1 == 1 { '#' } else { '.' });
        }
        s.push('\n');
    }
    s
}

/// `font-render` 커맨드: 음절들을 8×8로 렌더해 ASCII로 나란히 출력(튜닝용).
pub fn cmd_font_render(
    ttf: &Path,
    text: &str,
    px: f32,
    threshold: u8,
    x_off: i32,
    y_off: i32,
) -> Result<()> {
    let f = GlyphFont::load(ttf, px, threshold, x_off, y_off)?;
    println!(
        "폰트 {} px={px} thr={threshold} xoff={x_off} yoff={y_off}",
        ttf.display()
    );
    let chars: Vec<char> = text.chars().collect();
    for row in chars.chunks(8) {
        let glyphs: Vec<[u8; 8]> = row.iter().map(|&c| f.render(c)).collect();
        let labels: String = row.iter().map(|c| format!("{c}        ")).collect();
        println!("{labels}");
        for y in 0..8 {
            let line: Vec<String> = glyphs
                .iter()
                .map(|g| {
                    (0..8)
                        .map(|x| if (g[y] >> (7 - x)) & 1 == 1 { '#' } else { '.' })
                        .collect::<String>()
                })
                .collect();
            println!("{}", line.join("  "));
        }
        println!();
    }
    Ok(())
}
