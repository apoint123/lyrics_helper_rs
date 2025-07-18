//! Lyricify Lines 格式生成器

use std::fmt::Write;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LyricLine},
};

/// LYL 生成的主入口函数。
pub fn generate_lyl(
    lines: &[LyricLine],
    _metadata_store: &MetadataStore,
) -> Result<String, ConvertError> {
    let mut output = String::with_capacity(lines.len() * 50);

    writeln!(output, "[type:LyricifyLines]")?;

    for line in lines {
        // 优先使用 `line.line_text`
        let text_to_write = if let Some(text) = &line.line_text {
            text.clone()
        } else if !line.main_syllables.is_empty() {
            // `line_text` 不存在，则从 `main_syllables` 拼接文本
            line.main_syllables
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<&str>>()
                .join("")
        } else {
            continue;
        };

        writeln!(
            output,
            "[{},{}]{}",
            line.start_ms, line.end_ms, text_to_write
        )?;
    }

    Ok(output)
}
