//! QRC (Lyricify 标准) 歌词格式生成器

use std::fmt::Write;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LyricLine, LyricSyllable},
};

/// QRC 生成的主入口函数。
pub fn generate_qrc(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
) -> Result<String, ConvertError> {
    let mut qrc_output = String::new();

    writeln!(qrc_output, "{}", metadata_store.generate_lrc_header())?;

    // 遍历所有 LyricLine，依次生成主歌词行和背景人声行
    for line in lines {
        if !line.main_syllables.is_empty() {
            // 获取第一个和最后一个音节，确定整行的时间范围
            if let (Some(first_syl), Some(last_syl)) =
                (line.main_syllables.first(), line.main_syllables.last())
            {
                let line_start_ms = first_syl.start_ms;
                let line_end_ms = last_syl.end_ms;
                let line_duration_ms = line_end_ms.saturating_sub(line_start_ms);

                write!(qrc_output, "[{line_start_ms},{line_duration_ms}]")?;
                write_syllables_to_qrc_string(&mut qrc_output, &line.main_syllables, false)?;
                writeln!(qrc_output)?;
            }
        }

        if let Some(bg_section) = &line.background_section
            && !bg_section.syllables.is_empty()
            && let (Some(first_syl), Some(last_syl)) =
                (bg_section.syllables.first(), bg_section.syllables.last())
        {
            let line_start_ms = first_syl.start_ms;
            let line_end_ms = last_syl.end_ms;
            let line_duration_ms = line_end_ms.saturating_sub(line_start_ms);

            write!(qrc_output, "[{line_start_ms},{line_duration_ms}]")?;
            write_syllables_to_qrc_string(&mut qrc_output, &bg_section.syllables, true)?;
            writeln!(qrc_output)?;
        }
    }

    Ok(qrc_output)
}

/// 辅助函数，将音节列表格式化为 QRC 行的文本部分。
fn write_syllables_to_qrc_string(
    output: &mut String,
    syllables: &[LyricSyllable],
    is_background: bool,
) -> Result<(), std::fmt::Error> {
    for (i, syl) in syllables.iter().enumerate() {
        let duration_ms = syl.end_ms.saturating_sub(syl.start_ms);
        let is_first = i == 0;
        let is_last = i == syllables.len() - 1;

        if is_background {
            if is_first && is_last {
                // 是背景行，且只有一个音节，则文本前后都加上括号
                write!(output, "({})( {},{})", syl.text, syl.start_ms, duration_ms)?;
            } else if is_first {
                // 第一个音节，在文本前加上左括号
                write!(output, "({}({},{})", syl.text, syl.start_ms, duration_ms)?;
            } else if is_last {
                // 最后一个音节，在文本后加上右括号
                write!(output, "{})( {},{})", syl.text, syl.start_ms, duration_ms)?;
            } else {
                // 背景行的中间音节，照常输出
                write!(output, "{}({},{})", syl.text, syl.start_ms, duration_ms)?;
            }
        } else {
            // 不是背景行，直接输出
            write!(output, "{}({},{})", syl.text, syl.start_ms, duration_ms)?;
        }

        if syl.ends_with_space && !is_last {
            write!(output, " (0,0)")?;
        }
    }

    Ok(())
}
