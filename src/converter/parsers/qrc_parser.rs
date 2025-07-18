//! # QRC (Lyricify 标准) 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        BackgroundSection, ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
    },
    utils::parse_lrc_metadata_tag,
};

/// 匹配 QRC 行级时间戳 `[start,duration]`
static QRC_LINE_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?P<start>\d+),(?P<duration>\d+)\]")
        .expect("编译 QRC_LINE_TIMESTAMP_REGEX 失败")
});

/// 匹配音节后的时间戳 `(start,duration)`
static WORD_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((?P<start>\d+),(?P<duration>\d+)\)").expect("编译 WORD_TIMESTAMP_REGEX 失败")
});

/// 解析单行QRC歌词，但不处理背景人声逻辑，仅返回原始行数据和是否像背景人声的标志。
fn parse_qrc_line_raw(line_str: &str, line_num: usize) -> Result<(bool, LyricLine), ConvertError> {
    let line_ts_cap = QRC_LINE_TIMESTAMP_REGEX.captures(line_str).ok_or_else(|| {
        ConvertError::InvalidLyricFormat(format!(
            "第 {line_num} 行: 行首缺少行级时间戳 `[开始时间,持续时间]`。"
        ))
    })?;

    // 将原始行时间先存入临时变量作为备用
    let original_line_start_ms: u64 = line_ts_cap["start"]
        .parse()
        .map_err(ConvertError::ParseInt)?;
    let original_line_duration_ms: u64 = line_ts_cap["duration"]
        .parse()
        .map_err(ConvertError::ParseInt)?;

    let content_after_line_ts = if let Some(m) = line_ts_cap.get(0) {
        &line_str[m.end()..]
    } else {
        return Err(ConvertError::InvalidLyricFormat(
            "未能获取 QRC 行级时间戳的匹配项".into(),
        ));
    };

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_match_end = 0;

    // 遍历所有音节时间戳，提取其前面的文本作为音节
    for captures in WORD_TIMESTAMP_REGEX.captures_iter(content_after_line_ts) {
        let full_tag_match = captures.get(0).unwrap();
        let mut text_slice = &content_after_line_ts[last_match_end..full_tag_match.start()];
        let syl_start_ms: u64 = captures["start"].parse().map_err(ConvertError::ParseInt)?;
        let syl_duration_ms: u64 = captures["duration"]
            .parse()
            .map_err(ConvertError::ParseInt)?;

        if text_slice.starts_with(' ') {
            if let Some(last_syllable) = syllables.last_mut() {
                last_syllable.ends_with_space = true;
            }
            text_slice = text_slice.trim_start();
        }

        let mut current_ends_with_space = false;
        if text_slice.ends_with(' ') {
            current_ends_with_space = true;
            text_slice = text_slice.trim_end();
        }

        if !text_slice.is_empty() {
            syllables.push(LyricSyllable {
                text: text_slice.to_string(),
                start_ms: syl_start_ms,
                end_ms: syl_start_ms + syl_duration_ms,
                duration_ms: Some(syl_duration_ms),
                ends_with_space: current_ends_with_space,
            });
        }
        last_match_end = full_tag_match.end();
    }

    syllables.retain(|s| !s.text.is_empty());

    if syllables.is_empty() && !content_after_line_ts.trim().is_empty() {
        return Err(ConvertError::InvalidLyricFormat(format!(
            "第 {line_num} 行: 未能从内容 '{content_after_line_ts}' 中解析出任何有效的音节"
        )));
    }

    // 根据实际解析出的音节，重新计算行的精确起止时间
    let (final_start_ms, final_end_ms) = if !syllables.is_empty() {
        // unwrap是安全的，因为已经检查过syllables不为空
        let start_ms = syllables.first().unwrap().start_ms;
        let end_ms = syllables.last().unwrap().end_ms;
        (start_ms, end_ms)
    } else {
        // 如果没有音节（纯时间戳行），则回退使用原始的行时间
        (
            original_line_start_ms,
            original_line_start_ms + original_line_duration_ms,
        )
    };

    let full_line_text: String = syllables
        .iter()
        .map(|s| {
            if s.ends_with_space {
                s.text.clone() + " "
            } else {
                s.text.clone()
            }
        })
        .collect::<String>()
        .trim()
        .to_string();

    // 通过检查文本是否由括号包裹来判断是否为背景人声
    let is_background_candidate = (full_line_text.starts_with('(')
        || full_line_text.starts_with('（'))
        && (full_line_text.ends_with(')') || full_line_text.ends_with('）'));

    let lyric_line = LyricLine {
        start_ms: final_start_ms,
        end_ms: final_end_ms,
        line_text: Some(full_line_text),
        main_syllables: syllables,
        ..Default::default()
    };

    Ok((is_background_candidate, lyric_line))
}

/// 解析 QRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_qrc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut last_main_line_index: Option<usize> = None;

    for (i, line_str) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed_line = line_str.trim();

        if trimmed_line.is_empty() {
            continue;
        }

        // 解析元数据
        if parse_lrc_metadata_tag(trimmed_line, &mut raw_metadata) {
            continue;
        }

        // 解析歌词行
        if QRC_LINE_TIMESTAMP_REGEX.is_match(trimmed_line) {
            match parse_qrc_line_raw(trimmed_line, line_num) {
                Ok((is_background, mut parsed_line)) => {
                    if is_background {
                        // 尝试关联到上一行主歌词
                        if let Some(main_line_idx) =
                            last_main_line_index.filter(|idx| lines.get_mut(*idx).is_some())
                        {
                            let main_line = &mut lines[main_line_idx];

                            if main_line.background_section.is_none() {
                                // 第一个背景行，正常关联，并移除括号
                                for syl in &mut parsed_line.main_syllables {
                                    syl.text = syl
                                        .text
                                        .trim_matches(|c| {
                                            c == '(' || c == '（' || c == ')' || c == '）'
                                        })
                                        .to_string();
                                }
                                parsed_line.main_syllables.retain(|s| !s.text.is_empty());

                                main_line.background_section = Some(BackgroundSection {
                                    start_ms: parsed_line.start_ms,
                                    end_ms: parsed_line.end_ms,
                                    syllables: parsed_line.main_syllables,
                                    ..Default::default()
                                });
                            } else {
                                // 后续的背景行，提升为主歌词行，保留括号
                                warnings.push(format!(
                                    "第 {line_num} 行: 发现连续的背景行，将提升为主歌词行。"
                                ));
                                lines.push(parsed_line);
                                last_main_line_index = Some(lines.len() - 1);
                            }
                        } else {
                            // 孤立的背景行，也提升为主歌词行，保留括号
                            warnings.push(format!(
                                "第 {line_num} 行: 在任何主歌词行之前发现背景行，将提升为主歌词行。"
                            ));
                            lines.push(parsed_line);
                            last_main_line_index = Some(lines.len() - 1);
                        }
                    } else {
                        // 主歌词行
                        lines.push(parsed_line);
                        last_main_line_index = Some(lines.len() - 1);
                    }
                }
                Err(e) => {
                    warnings.push(format!("第 {line_num} 行: 解析失败。错误: {e}"));
                }
            }
        } else {
            warnings.push(format!("第 {line_num} 行: 未能识别的行格式。"));
        }
    }

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: LyricFormat::Qrc,
        is_line_timed_source: false,
        ..Default::default()
    })
}
