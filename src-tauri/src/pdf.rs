use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use printpdf::*;

use crate::error::{AppError, AppResult};

const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "C:\\Windows\\Fonts\\msyh.ttf",
    "C:\\Windows\\Fonts\\simsun.ttc",
];

pub fn cjk_font_path() -> Option<String> {
    FONT_CANDIDATES.iter().find(|path| Path::new(path).is_file()).map(|path| (*path).to_string())
}

pub fn write_text_pdf(path: &Path, title: &str, lines: &[String]) -> AppResult<()> {
    let font_path = cjk_font_path().ok_or_else(|| {
        AppError::msg(
            "没有找到可嵌入的中文字体（DroidSansFallback、Noto Sans CJK、微软雅黑或宋体）。请安装其中一种，或继续使用 HTML 导出后打印。",
        )
    })?;
    let (doc, page, layer) = PdfDocument::new(title, Mm(210.0), Mm(297.0), "正文");
    let font = doc
        .add_external_font(File::open(&font_path).map_err(|err| AppError::msg(err.to_string()))?)
        .map_err(|err| AppError::msg(format!("字体无法嵌入: {err}")))?;
    let mut page = page;
    let mut layer = layer;
    let mut y = 280.0;
    let mut current = doc.get_page(page).get_layer(layer);
    current.use_text(title, 14.0, Mm(12.0), Mm(y), &font);
    y -= 8.0;
    for line in lines {
        if y < 16.0 {
            let (next_page, next_layer) = doc.add_page(Mm(210.0), Mm(297.0), "正文");
            page = next_page;
            layer = next_layer;
            current = doc.get_page(page).get_layer(layer);
            y = 280.0;
        }
        let text = clip_line(line, 80);
        current.use_text(text, 9.0, Mm(12.0), Mm(y), &font);
        y -= 5.0;
    }
    let file = File::create(path).map_err(|err| AppError::msg(err.to_string()))?;
    doc.save(&mut BufWriter::new(file)).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(())
}

fn clip_line(line: &str, max_chars: usize) -> String {
    let count = line.chars().count();
    if count <= max_chars {
        line.to_string()
    } else {
        let mut text: String = line.chars().take(max_chars.saturating_sub(1)).collect();
        text.push('…');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clips_long_lines() {
        let text = "活着".repeat(50);
        let clipped = clip_line(&text, 10);
        assert_eq!(clipped.chars().count(), 10);
        assert!(clipped.ends_with('…'));
    }
}
