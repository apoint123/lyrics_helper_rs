//! LRC 格式生成器

use std::fmt::Write as FmtWrite;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{
        BackgroundSection, ConvertError, LrcEndTimeOutputMode, LrcGenerationOptions,
        LrcSubLinesOutputMode, LyricLine,
    },
};

/// LRC 生成的主入口函数。
pub fn generate_lrc(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
    options: &LrcGenerationOptions,
) -> Result<String, ConvertError> {
    let mut lrc_output = String::with_capacity(lines.len() * 50);

    let lrc_header = metadata_store.generate_lrc_header();
    if !lrc_header.is_empty() {
        writeln!(lrc_output, "{}", lrc_header.trim_end_matches('\n'))?;
    }

    for (i, line) in lines.iter().enumerate() {
        match options.sub_lines_output_mode {
            LrcSubLinesOutputMode::Ignore => {
                write_main_line(&mut lrc_output, line)?;
            }
            LrcSubLinesOutputMode::MergeWithParentheses => {
                write_merged_line(&mut lrc_output, line)?;
            }
            LrcSubLinesOutputMode::SeparateLines => {
                write_main_line(&mut lrc_output, line)?;
                if let Some(sub_line) = &line.background_section {
                    write_sub_line(&mut lrc_output, sub_line)?;
                }
            }
        }

        let next_line_start_ms = lines.get(i + 1).map(|l| l.start_ms);
        handle_end_time_output(
            &mut lrc_output,
            line,
            options.end_time_output_mode,
            next_line_start_ms,
        )?;
    }

    let trimmed_output = lrc_output.trim_end();
    Ok(format!("{trimmed_output}\n"))
}

/// 获取一行的文本，优先使用 line_text，否则拼接 syllables。
fn get_line_text(line: &LyricLine) -> Option<String> {
    line.line_text.clone().or_else(|| {
        if !line.main_syllables.is_empty() {
            Some(
                line.main_syllables
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>(),
            )
        } else {
            None
        }
    })
}

/// 写入一个简单的、带时间戳的 LRC 行。
fn write_simple_lrc_line(
    output: &mut String,
    start_ms: u64,
    text: &str,
) -> Result<(), std::fmt::Error> {
    if !text.trim().is_empty() {
        writeln!(output, "{}{}", format_lrc_time_ms(start_ms), text)?;
    }
    Ok(())
}

/// 将主歌词作为独立的 LRC 行写入。
fn write_main_line(output: &mut String, line: &LyricLine) -> Result<(), std::fmt::Error> {
    if let Some(text) = get_line_text(line) {
        write_simple_lrc_line(output, line.start_ms, &text)?;
    }
    Ok(())
}

/// 将背景人声部分作为独立的 LRC 行写入。
fn write_sub_line(
    output: &mut String,
    sub_line: &BackgroundSection,
) -> Result<(), std::fmt::Error> {
    let text: String = sub_line.syllables.iter().map(|s| s.text.as_str()).collect();
    write_simple_lrc_line(output, sub_line.start_ms, &text)?;
    Ok(())
}

/// 将主歌词和背景人声合并为一行写入。
fn write_merged_line(output: &mut String, line: &LyricLine) -> Result<(), std::fmt::Error> {
    let main_text = get_line_text(line);

    if let Some(sub_section) = &line.background_section {
        let sub_text: String = sub_section
            .syllables
            .iter()
            .map(|s| s.text.as_str())
            .collect();

        let merged_text = match main_text {
            Some(mt) if !mt.trim().is_empty() => format!("{} ({})", mt.trim(), sub_text.trim()),
            _ => format!("({})", sub_text.trim()),
        };

        let merged_start_ms = line.start_ms.min(sub_section.start_ms);
        write_simple_lrc_line(output, merged_start_ms, &merged_text)?;
    } else if let Some(text) = main_text {
        write_simple_lrc_line(output, line.start_ms, &text)?;
    }
    Ok(())
}

/// 根据选项处理是否输出行结束时间戳。
fn handle_end_time_output(
    output: &mut String,
    current_line: &LyricLine,
    mode: LrcEndTimeOutputMode,
    next_line_start_ms: Option<u64>,
) -> Result<(), std::fmt::Error> {
    let current_end_ms = current_line
        .background_section
        .as_ref()
        .map_or(current_line.end_ms, |sub| {
            sub.end_ms.max(current_line.end_ms)
        });

    if current_end_ms == 0 {
        return Ok(());
    }

    match mode {
        LrcEndTimeOutputMode::Never => { /* 什么也不做 */ }
        LrcEndTimeOutputMode::Always => {
            writeln!(output, "{}", format_lrc_time_ms(current_end_ms))?;
        }
        LrcEndTimeOutputMode::OnLongPause { threshold_ms } => {
            if let Some(next_start) = next_line_start_ms {
                // 如果与下一行的间隔超过阈值
                if next_start.saturating_sub(current_end_ms) > threshold_ms {
                    writeln!(output, "{}", format_lrc_time_ms(current_end_ms))?;
                }
            } else {
                // 这是文件的最后一行歌词，也输出结束标记
                writeln!(output, "{}", format_lrc_time_ms(current_end_ms))?;
            }
        }
    }
    Ok(())
}

/// 将毫秒时间格式化为 LRC 时间字符串 `[mm:ss.xxx]` 或 `[mm:ss.xx]`。
/// 此函数输出毫秒 (xxx)。
///
/// # 参数
/// * `ms` - 需要格式化的总毫秒数。
///
/// # 返回
/// `String` - 格式化后的 LRC 时间标签字符串。
pub fn format_lrc_time_ms(ms: u64) -> String {
    let minutes = ms / 60000;
    let seconds = (ms % 60000) / 1000;
    let milliseconds = ms % 1000;
    format!("[{minutes:02}:{seconds:02}.{milliseconds:03}]")
}
