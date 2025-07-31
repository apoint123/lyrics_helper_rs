//! LYS 歌词格式生成器

use std::fmt::Write;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LyricLine, LyricSyllable, lys_properties},
};

/// LYS 生成的主入口函数。
pub fn generate_lys(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
) -> Result<String, ConvertError> {
    let mut lys_output = String::with_capacity(lines.len() * 120);

    writeln!(lys_output, "{}", metadata_store.generate_lrc_header())?;

    for line in lines {
        if !line.main_syllables.is_empty() {
            let property = match line.agent.as_deref() {
                // 如果 agent 是 "v2"，视为右。
                Some("v2") => lys_properties::MAIN_RIGHT,
                // 其他任何情况，都默认视为左。
                Some(_) | None => lys_properties::MAIN_LEFT,
            };
            write!(lys_output, "[{property}]")?;

            write_syllables_to_string(&mut lys_output, &line.main_syllables, false)?;
            writeln!(lys_output)?;
        }

        if let Some(bg_section) = &line.background_section
            && !bg_section.syllables.is_empty()
        {
            let bg_property = match line.agent.as_deref() {
                // agent "v2" -> 右。
                Some("v2") => lys_properties::BG_RIGHT,
                // 其他情况 -> 左。
                Some(_) | None => lys_properties::BG_LEFT,
            };
            write!(lys_output, "[{bg_property}]")?;

            write_syllables_to_string(&mut lys_output, &bg_section.syllables, true)?;
            writeln!(lys_output)?;
        }
    }

    Ok(lys_output)
}

/// 辅助函数，将音节列表 (`&[LyricSyllable]`) 格式化为 LYS 行的文本部分。
///
/// # 参数
/// * `output` - 可变的字符串切片，用于写入生成的文本。
/// * `syllables` - 要处理的音节列表。
/// * `is_background` - 布尔值，指示这些音节是否为背景人声。
fn write_syllables_to_string(
    output: &mut String,
    syllables: &[LyricSyllable],
    is_background: bool,
) -> Result<(), std::fmt::Error> {
    for (i, syl) in syllables.iter().enumerate() {
        let duration_ms = syl.end_ms.saturating_sub(syl.start_ms);
        let mut text_to_write = syl.text.as_str();

        let modified_text_buffer;

        if is_background {
            if i == 0 && i == syllables.len() - 1 {
                modified_text_buffer = format!("({text_to_write})");
                text_to_write = &modified_text_buffer;
            } else if i == 0 {
                modified_text_buffer = format!("({text_to_write}");
                text_to_write = &modified_text_buffer;
            } else if i == syllables.len() - 1 {
                modified_text_buffer = format!("{text_to_write})");
                text_to_write = &modified_text_buffer;
            }
        }

        write!(
            output,
            "{}({},{})",
            text_to_write, syl.start_ms, duration_ms
        )?;

        if syl.ends_with_space && i < syllables.len() - 1 {
            write!(output, " (0,0)")?;
        }
    }

    Ok(())
}
