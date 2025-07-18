//! 增强型 LRC 格式生成器。

use tracing::warn;

use crate::converter::{
    LyricSyllable,
    generators::lrc_generator::format_lrc_time_ms,
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LrcGenerationOptions, LrcSubLinesOutputMode, LyricLine},
};

/// 增强型 LRC 生成的主入口函数。
pub fn generate_enhanced_lrc(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
    options: &LrcGenerationOptions,
) -> Result<String, ConvertError> {
    let header = metadata_store.generate_lrc_header();
    let mut lyric_lines = Vec::new();

    for line in lines {
        if line.main_syllables.is_empty() {
            if let Some(text) = &line.line_text
                && !text.trim().is_empty()
            {
                lyric_lines.push(format!("{}{}", format_lrc_time_ms(line.start_ms), text));
            }
        } else {
            lyric_lines.push(build_enhanced_lrc_line(
                &line.main_syllables,
                line.start_ms,
                line.end_ms,
            ));
        }

        if let Some(bg_section) = &line.background_section {
            match options.sub_lines_output_mode {
                LrcSubLinesOutputMode::Ignore => {
                    // 不做任何事
                }
                LrcSubLinesOutputMode::SeparateLines => {
                    if !bg_section.syllables.is_empty() {
                        lyric_lines.push(build_enhanced_lrc_line(
                            &bg_section.syllables,
                            bg_section.start_ms,
                            bg_section.end_ms,
                        ));
                    }
                }
                LrcSubLinesOutputMode::MergeWithParentheses => {
                    // 对于逐字的增强型LRC，将背景人声用括号合并到主歌词中非常复杂且容易出错。
                    // 暂时忽略以避免产生格式不正确的输出。
                    warn!(
                        "[增强型LRC生成器] MergeWithParentheses 模式过于复杂，生成器不支持该模式。此行歌词将被忽略。"
                    );
                }
            }
        }
    }

    let final_output = format!("{}{}", header, lyric_lines.join("\n"));
    Ok(final_output)
}

/// 辅助函数，根据音节列表构建单行增强型LRC歌词
fn build_enhanced_lrc_line(syllables: &[LyricSyllable], start_ms: u64, end_ms: u64) -> String {
    let mut line_builder = String::new();
    line_builder.push_str(&format_lrc_time_ms(start_ms));

    for syllable in syllables {
        line_builder.push_str(&format_word_time(syllable.start_ms));
        line_builder.push_str(&syllable.text);
    }

    // 始终为最后一个词添加行的结束时间戳
    if end_ms > 0 {
        line_builder.push_str(&format_word_time(end_ms));
    }

    line_builder
}

/// 将毫秒时间格式化为逐字时间标签 `<mm:ss.xxx>`。
fn format_word_time(ms: u64) -> String {
    let minutes = ms / 60000;
    let seconds = (ms % 60000) / 1000;
    let milliseconds = ms % 1000;
    format!("<{minutes:02}:{seconds:02}.{milliseconds:03}>")
}
