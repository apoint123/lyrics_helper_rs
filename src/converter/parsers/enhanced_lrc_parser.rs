//! 增强型 LRC (Enhanced LRC) 格式解析器。
//!
//! 支持两种逐字时间戳格式：
//! 1. `[line_time]...<word_time>word<word_time>word<end_time>`
//! 2. `[line_time]<word_time>word<word_time>word...` (无末尾时间戳)
//!
//! 同时处理行起始时间与第一个词起始时间不一致的情况，并以后者为准。

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::converter::{
    types::{ConvertError, LyricLine, LyricSyllable, ParsedSourceData},
    utils::parse_and_store_metadata,
};

/// 用于匹配行时间标签，例如 [00:12.34]
static LINE_TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{2,}):(\d{2})[.:](\d{2,3})\]").unwrap());
/// 用于匹配逐字时间标签，例如 <00:12.34>
static WORD_TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(\d{2,}):(\d{2})[.:](\d{2,3})>").unwrap());
/// 用于匹配 [mm:ss.xx] 或 <mm:ss.xx> 格式，并捕获其中的数字。
static TIME_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[<\[](\d{2,}):(\d{2})[.:](\d{2,3})[>\]]").unwrap());

const DEFAULT_LINE_DURATION_MS: u64 = 5000;

/// 解析增强型 LRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_enhanced_lrc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    for (line_num, line_str) in content.lines().enumerate() {
        let line_num_one_based = line_num + 1;
        let line_str_trimmed = line_str.trim();

        if line_str_trimmed.is_empty() {
            continue;
        }

        if parse_and_store_metadata(line_str_trimmed, &mut raw_metadata) {
            continue;
        } else if let Some(line_time_match) = LINE_TIME_RE.find(line_str_trimmed) {
            let line_start_ms = match parse_lrc_time_tag(line_time_match.as_str()) {
                Ok(Some(time)) => time,
                _ => {
                    warnings.push(format!(
                        "第 {line_num_one_based} 行: 无法解析行时间戳，已跳过。"
                    ));
                    continue;
                }
            };

            let line_content = &line_str_trimmed[line_time_match.end()..];

            let syllables = parse_syllables_from_line(
                line_content,
                line_start_ms,
                &mut warnings,
                line_num_one_based,
            );

            if !syllables.is_empty() {
                // 如果行开始时间与第一个词的开始时间不同，使用第一个词的开始时间
                let final_line_start_ms = syllables.first().map_or(line_start_ms, |s| s.start_ms);

                let line_text = syllables
                    .iter()
                    .map(|s| {
                        if s.ends_with_space {
                            format!("{} ", s.text)
                        } else {
                            s.text.clone()
                        }
                    })
                    .collect::<String>();

                lines.push(LyricLine {
                    start_ms: final_line_start_ms,
                    end_ms: 0, // 在第二遍处理中填充
                    line_text: Some(line_text),
                    main_syllables: syllables,
                    ..Default::default()
                });
            } else if !line_content.trim().is_empty() {
                // 如果行内没有逐字时间戳，则作为普通LRC行处理
                lines.push(LyricLine {
                    start_ms: line_start_ms,
                    end_ms: 0,
                    line_text: Some(crate::converter::utils::normalize_text_whitespace(
                        line_content,
                    )),
                    ..Default::default()
                });
            }
        } else {
            warnings.push(format!(
                "第 {} 行: 无法识别的行格式，已忽略: '{}'",
                line_num_one_based,
                line_str_trimmed.chars().take(50).collect::<String>()
            ));
        }
    }

    // 第二遍处理：填充行和音节的结束时间
    finalize_end_times(&mut lines, &mut warnings);

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: crate::converter::types::LyricFormat::EnhancedLrc,
        is_line_timed_source: false,
        ..Default::default()
    })
}

/// 从单行文本中解析出所有音节
fn parse_syllables_from_line(
    line_content: &str,
    line_start_ms: u64,
    warnings: &mut Vec<String>,
    line_num: usize,
) -> Vec<LyricSyllable> {
    let time_tags: Vec<(u64, std::ops::Range<usize>)> = WORD_TIME_RE
        .find_iter(line_content)
        .filter_map(|mat| {
            parse_lrc_time_tag(mat.as_str())
                .ok()
                .flatten()
                .map(|time| (time, mat.range()))
        })
        .collect();

    if time_tags.is_empty() {
        return Vec::new();
    }

    if let Some((first_word_time, _)) = time_tags.first()
        && line_start_ms != *first_word_time
    {
        warnings.push(format!(
                "第 {line_num} 行: 行时间戳 [{line_start_ms}] 与第一个音节时间戳 <{first_word_time}> 不匹配，已以后者为准。"
            ));
    }

    let mut syllables = Vec::new();
    for i in 0..time_tags.len() {
        let (current_time, current_range) = (&time_tags[i].0, &time_tags[i].1);

        let text_start = current_range.end;
        let text_end = time_tags
            .get(i + 1)
            .map_or(line_content.len(), |next_tag| next_tag.1.start);

        let raw_text_slice = &line_content[text_start..text_end];

        let ends_with_space = raw_text_slice.ends_with(' ');
        let text = crate::converter::utils::normalize_text_whitespace(raw_text_slice);

        if !text.is_empty() {
            let next_time = time_tags.get(i + 1).map(|t| t.0);

            if let Some(nt) = next_time
                && nt < *current_time
            {
                warnings.push(format!(
                    "第 {line_num} 行: 检测到时间戳乱序或回溯 (<{current_time}> -> <{nt}>)。"
                ));
            }

            let end_ms = next_time.unwrap_or(0);
            let duration_ms = if end_ms > 0 {
                Some(end_ms.saturating_sub(*current_time))
            } else {
                None
            };

            syllables.push(LyricSyllable {
                text,
                start_ms: *current_time,
                end_ms,
                duration_ms,
                ends_with_space,
            });
        }
    }

    syllables
}

/// 第二遍处理，修正所有行和音节的结束时间
fn finalize_end_times(lines: &mut [LyricLine], warnings: &mut Vec<String>) {
    // 首先按开始时间排序，确保时间线是正确的
    lines.sort_by_key(|line| line.start_ms);

    let num_lines = lines.len();
    for i in 0..num_lines {
        let next_line_start_ms = if i + 1 < num_lines {
            Some(lines[i + 1].start_ms)
        } else {
            None
        };

        let current_line_start_ms = lines[i].start_ms;

        if let Some(last_syllable) = lines[i].main_syllables.last_mut() {
            // 如果最后一个音节的结束时间是待定的 (0)
            if last_syllable.end_ms == 0 {
                let mut end_ms = next_line_start_ms
                    .unwrap_or_else(|| last_syllable.start_ms + DEFAULT_LINE_DURATION_MS);

                if end_ms <= last_syllable.start_ms {
                    warnings.push(format!(
                        "行 ( {current_line_start_ms}ms): 最后一个音节的结束时间（由下一行决定）早于或等于其开始时间。结束时间已被修正。"
                    ));
                    end_ms = last_syllable.start_ms + 1000;
                }

                last_syllable.end_ms = end_ms;
                last_syllable.duration_ms = Some(end_ms.saturating_sub(last_syllable.start_ms));
            }
            lines[i].end_ms = last_syllable.end_ms;
        } else if lines[i].line_text.is_some() {
            let mut end_ms = next_line_start_ms
                .unwrap_or_else(|| current_line_start_ms + DEFAULT_LINE_DURATION_MS);

            if end_ms <= current_line_start_ms {
                end_ms = current_line_start_ms + 1000;
            }
            lines[i].end_ms = end_ms;
        }

        if next_line_start_ms.is_none() && num_lines > 0 {
            warnings.push(format!(
                "第 {} 行 ( {}ms): 作为最后一行，其结束时间是估算值。",
                i + 1,
                current_line_start_ms
            ));
        }
    }
}

/// 解析 LRC 格式的时间标签（[mm:ss.xx] 或 <mm:ss.xx>），并返回总毫秒数。
///
/// # 参数
/// * `tag_str` - 包含时间标签的字符串，例如 "[01:23.45]" 或 "<01:23.456>"。
///
/// # 返回
/// * `Ok(Some(u64))` - 成功解析，返回总毫秒数。
/// * `Ok(None)` - 输入字符串不匹配时间标签格式。
/// * `Err(ConvertError)` - 标签内的数字解析失败，这是一个内部错误。
pub fn parse_lrc_time_tag(tag_str: &str) -> Result<Option<u64>, ConvertError> {
    if let Some(caps) = TIME_TAG_RE.captures(tag_str) {
        let minutes: u64 = caps.get(1).map_or(Ok(0), |m| m.as_str().parse())?;
        let seconds: u64 = caps.get(2).map_or(Ok(0), |m| m.as_str().parse())?;
        let fraction_str = caps.get(3).map_or("0", |m| m.as_str());
        let mut fraction: u64 = fraction_str.parse()?;

        // 如果是厘秒 (xx)，则乘以10转换为毫秒 (xxx)
        if fraction_str.len() == 2 {
            fraction *= 10;
        }

        let total_ms = minutes * 60 * 1000 + seconds * 1000 + fraction;
        Ok(Some(total_ms))
    } else {
        // 如果正则表达式不匹配，则这不是一个有效的时间标签
        Ok(None)
    }
}
