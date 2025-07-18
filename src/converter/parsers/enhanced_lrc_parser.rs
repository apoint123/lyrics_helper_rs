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

use crate::converter::types::{ConvertError, LyricLine, LyricSyllable, ParsedSourceData};

/// 用于匹配元数据标签，例如 [ar:Artist]
static METADATA_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[([a-zA-Z_][a-zA-Z0-9_]*):(.*?)\]$").unwrap());
/// 用于匹配行时间标签，例如 [00:12.34]
static LINE_TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{2,}):(\d{2})[.:](\d{2,3})\]").unwrap());
/// 用于匹配逐字时间标签，例如 <00:12.34>
static WORD_TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(\d{2,}):(\d{2})[.:](\d{2,3})>").unwrap());
/// 用于匹配 [mm:ss.xx] 或 <mm:ss.xx> 格式，并捕获其中的数字。
static TIME_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[<\[](\d{2,}):(\d{2})[.:](\d{2,3})[>\]]").unwrap());

/// 解析增强型 LRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_enhanced_lrc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    for (line_num, line_str) in content.lines().enumerate() {
        let line_num_one_based = line_num + 1;
        let line_str_trimmed = line_str.trim();

        if line_str.trim().is_empty() {
            continue;
        }

        // 解析元数据
        if let Some(caps) = METADATA_RE.captures(line_str) {
            let key = caps.get(1).unwrap().as_str().to_string();
            let value = caps.get(2).unwrap().as_str().trim().to_string();
            raw_metadata.entry(key).or_default().push(value);
            continue;
        }

        // 解析歌词行
        if let Some(line_time_caps) = LINE_TIME_RE.captures(line_str) {
            let line_start_ms = parse_lrc_time_tag(&line_time_caps[0])?.unwrap_or(0);
            let line_content = LINE_TIME_RE.replace(line_str, "").to_string();

            if line_content.trim().is_empty() {
                continue;
            }

            let syllables = parse_syllables_from_line(
                &line_content,
                line_start_ms,
                &WORD_TIME_RE,
                &mut warnings,
                line_num,
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
                    .collect::<String>()
                    .trim_end()
                    .to_string();

                lines.push(LyricLine {
                    start_ms: final_line_start_ms,
                    end_ms: 0, // 在第二遍处理中填充
                    line_text: Some(line_text),
                    main_syllables: syllables,
                    ..Default::default()
                });
            } else {
                // 如果行内没有逐字时间戳，则作为普通LRC行处理
                lines.push(LyricLine {
                    start_ms: line_start_ms,
                    end_ms: line_start_ms, // 简单处理
                    line_text: Some(line_content.trim().to_string()),
                    ..Default::default()
                });
            }
        }

        if !METADATA_RE.is_match(line_str_trimmed) && !LINE_TIME_RE.is_match(line_str_trimmed) {
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
        ..Default::default()
    })
}

/// 从单行文本中解析出所有音节
fn parse_syllables_from_line(
    line_content: &str,
    line_start_ms: u64,
    word_time_re: &Regex,
    warnings: &mut Vec<String>,
    line_num: usize,
) -> Vec<LyricSyllable> {
    let mut syllables = Vec::new();

    // 查找所有时间戳及其位置
    let time_tags: Vec<(u64, (usize, usize))> = word_time_re
        .find_iter(line_content)
        .filter_map(|m| {
            parse_lrc_time_tag(m.as_str())
                .ok()
                .flatten()
                .map(|time| (time, (m.start(), m.end())))
        })
        .collect();

    if !time_tags.is_empty() {
        let first_syllable_time = time_tags[0].0;
        if line_start_ms != first_syllable_time {
            warnings.push(format!(
                "第 {line_num} 行: 行时间戳 [{line_start_ms}] 与第一个音节时间戳 <{first_syllable_time}> 不匹配，已以后者为准。"
            ));
        }
    }

    // 如果没有逐字时间戳，则直接返回空
    if time_tags.is_empty() {
        return syllables;
    }

    let mut current_time = line_start_ms;

    // 处理第一个时间戳之前的文本（如果有）
    let first_tag_start = time_tags[0].1.0;
    if first_tag_start > 0 {
        let text = line_content[0..first_tag_start].to_string();
        syllables.push(LyricSyllable {
            text,
            start_ms: current_time,
            end_ms: time_tags[0].0,
            duration_ms: Some(time_tags[0].0.saturating_sub(current_time)),
            ..Default::default()
        });
    }

    // 根据规则，行的实际开始时间应为第一个词的开始时间
    // 如果第一个词有时间戳，则使用它；否则使用行的 [t] 时间
    current_time = time_tags[0].0;

    // 迭代处理每个时间戳和它后面的文本
    for i in 0..time_tags.len() {
        let (tag_time, (_, tag_end)) = time_tags[i];

        // 规则应用：如果这是第一个标签，则用它的时间覆盖行的初始时间
        if i == 0 {
            current_time = tag_time;
        }

        let text_start = tag_end;
        let text_end = if i + 1 < time_tags.len() {
            time_tags[i + 1].1.0 // 下一个标签的开始
        } else {
            line_content.len() // 或字符串的末尾
        };

        let text = line_content[text_start..text_end].to_string();

        let end_time = if i + 1 < time_tags.len() {
            let next_tag_time = time_tags[i + 1].0;
            if next_tag_time < tag_time {
                warnings.push(format!(
                    "第 {line_num} 行: 检测到时间戳乱序或回溯 (<{tag_time}> -> <{next_tag_time}>)。"
                ));
            }
            next_tag_time
        } else {
            0 // 待定
        };

        if !text.is_empty() {
            syllables.push(LyricSyllable {
                text,
                start_ms: current_time,
                end_ms: end_time,
                duration_ms: if end_time > 0 {
                    Some(end_time.saturating_sub(current_time))
                } else {
                    None
                },
                ..Default::default()
            });
            current_time = end_time;
        } else if i > 0 {
            warnings.push(format!(
                "第 {line_num} 行: 在时间戳 <{current_time}> 之后发现了一个空的音节，已忽略。"
            ));
        }
    }

    syllables
}

/// 第二遍处理，修正所有行和音节的结束时间
fn finalize_end_times(lines: &mut [LyricLine], warnings: &mut Vec<String>) {
    // 首先按开始时间排序，确保时间线是正确的
    lines.sort_by_key(|line| line.start_ms);

    for i in 0..lines.len() {
        let next_line_start_ms = if i + 1 < lines.len() {
            Some(lines[i + 1].start_ms)
        } else {
            None
        };

        if let Some(last_syllable) = lines[i].main_syllables.last_mut() {
            // 如果最后一个音节的结束时间是待定的 (0)
            if last_syllable.end_ms == 0 {
                // 用下一行的开始时间作为结束时间
                if let Some(next_start) = next_line_start_ms {
                    if next_start <= last_syllable.start_ms {
                        warnings.push(format!(
                            "行 ({}): 最后一个音节的结束时间（由下一行决定）早于或等于其开始时间。结束时间已被修正。",
                            lines[i].start_ms
                        ));
                        last_syllable.end_ms = last_syllable.start_ms + 1000;
                    } else {
                        last_syllable.end_ms = next_start;
                    }
                }

                if next_line_start_ms.is_none() {
                    warnings.push(format!(
                        "第 {} 行 ({}): 作为最后一行，其结束时间是估算值。",
                        i + 1,
                        lines[i].start_ms
                    ));
                }

                last_syllable.duration_ms =
                    Some(last_syllable.end_ms.saturating_sub(last_syllable.start_ms));
            }
        } else if lines[i].line_text.is_some() && next_line_start_ms.is_none() {
            warnings.push(format!(
                "第 {} 行 ({}): 作为最后一行，其结束时间是估算值。",
                i + 1,
                lines[i].start_ms
            ));
        }

        // 更新整行的结束时间
        if let Some(final_syllable) = lines[i].main_syllables.last() {
            lines[i].end_ms = final_syllable.end_ms;
        } else if lines[i].line_text.is_some() {
            if let Some(next_start) = next_line_start_ms {
                lines[i].end_ms = next_start;
            } else {
                lines[i].end_ms = lines[i].start_ms + 2000;
            }
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
        // 从捕获组中解析分钟、秒和毫秒/厘秒
        // get(n).unwrap() 是安全的，因为如果整个表达式匹配，这些组也必然存在。
        let minutes: u64 = caps.get(1).unwrap().as_str().parse()?;
        let seconds: u64 = caps.get(2).unwrap().as_str().parse()?;
        let fraction_str = caps.get(3).unwrap().as_str();
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
