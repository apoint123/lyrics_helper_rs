//! # LYS 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        BackgroundSection, ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
    },
    utils::{parse_and_store_metadata, process_syllable_text},
};

// 匹配 LYS 行首的属性标签，如 `[4]`
static LYS_PROPERTY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d+)\]").expect("编译 LYS_PROPERTY_REGEX 失败"));

/// 匹配 LYS 音节的时间戳，如 `(100,200)`
static LYS_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\((?P<start>\d+),(?P<duration>\d+)\)").expect("编译 LYS_TIMESTAMP_REGEX 失败")
});

/// 解析单行 LYS 歌词文本，返回其属性和解析后的 `LyricLine`。
fn parse_lys_line(line_str: &str, line_num: usize) -> Result<(u8, LyricLine), ConvertError> {
    let property_cap = LYS_PROPERTY_REGEX.captures(line_str).ok_or_else(|| {
        ConvertError::InvalidLyricFormat(format!("第 {line_num} 行: 行首缺少属性标签 `[数字]`。"))
    })?;
    let property: u8 = property_cap[1].parse().map_err(ConvertError::ParseInt)?;

    let content_after_property = if let Some(m) = property_cap.get(0) {
        &line_str[m.end()..]
    } else {
        return Err(ConvertError::InvalidLyricFormat(
            "未能获取属性标签的匹配项".to_string(),
        ));
    };

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_match_end = 0;
    let mut min_start_ms = u64::MAX;
    let mut max_end_ms = 0;

    // 遍历所有时间戳，并提取其前面的文本作为音节
    for ts_cap_match in LYS_TIMESTAMP_REGEX.find_iter(content_after_property) {
        if let Some(ts_cap) = LYS_TIMESTAMP_REGEX.captures(ts_cap_match.as_str()) {
            let raw_text_slice = &content_after_property[last_match_end..ts_cap_match.start()];

            if let Some((clean_text, ends_with_space)) =
                process_syllable_text(raw_text_slice, &mut syllables)
            {
                let start_ms: u64 = ts_cap["start"].parse().map_err(ConvertError::ParseInt)?;
                let duration_ms: u64 =
                    ts_cap["duration"].parse().map_err(ConvertError::ParseInt)?;

                let end_ms = start_ms + duration_ms;

                syllables.push(LyricSyllable {
                    text: clean_text,
                    start_ms,
                    end_ms,
                    duration_ms: Some(duration_ms),
                    ends_with_space,
                });
                min_start_ms = min_start_ms.min(start_ms);
                max_end_ms = max_end_ms.max(end_ms);
            }
            last_match_end = ts_cap_match.end();
        }
    }

    if syllables.is_empty() && !content_after_property.trim().is_empty() {
        return Err(ConvertError::InvalidLyricFormat(format!(
            "第 {line_num} 行: 发现了内容，但未能解析出任何有效的音节。"
        )));
    }

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

    let line = LyricLine {
        start_ms: if min_start_ms == u64::MAX {
            0
        } else {
            min_start_ms
        },
        end_ms: max_end_ms,
        line_text: Some(line_text),
        main_syllables: syllables,
        ..Default::default()
    };
    Ok((property, line))
}

/// 解析 LYS 格式内容到 `ParsedSourceData` 结构。
pub fn parse_lys(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // 状态变量，用于关联背景行和主歌词行
    let mut last_main_line_index: Option<usize> = None;
    let mut last_main_line_agent: Option<String> = None;

    for (i, line_str) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed_line = line_str.trim();

        if trimmed_line.is_empty() {
            continue;
        }

        // 解析元数据
        if parse_and_store_metadata(trimmed_line, &mut raw_metadata) {
            continue;
        }

        // 解析歌词行
        if LYS_PROPERTY_REGEX.is_match(trimmed_line) {
            match parse_lys_line(trimmed_line, line_num) {
                Ok((property, mut parsed_line)) => {
                    // 6, 7, 8 代表背景人声
                    let is_background = matches!(property, 6..=8);

                    if is_background {
                        if let Some(main_line_idx) = last_main_line_index {
                            let main_line = &mut lines[main_line_idx];
                            // 将背景行关联到最近的主歌词行
                            if main_line.background_section.is_none() {
                                main_line.background_section = Some(BackgroundSection {
                                    start_ms: parsed_line.start_ms,
                                    end_ms: parsed_line.end_ms,
                                    syllables: parsed_line.main_syllables,
                                    ..Default::default()
                                });
                            } else {
                                // 如果主歌词行已有背景，则提升为新的主歌词行
                                warnings.push(format!(
                                    "第 {line_num} 行: 连续的背景行，将提升为新的主歌词行。"
                                ));
                                parsed_line.agent = last_main_line_agent
                                    .clone()
                                    .or_else(|| Some("v1".to_string()));
                                let new_index = lines.len();
                                lines.push(parsed_line);
                                last_main_line_index = Some(new_index);
                                last_main_line_agent =
                                    lines.get(new_index).and_then(|l| l.agent.clone());
                            }
                        } else {
                            warnings.push(format!(
                                "第 {line_num} 行: 背景行出现在任何主歌词行之前，将提升为主歌词行。"
                            ));
                            let agent = Some("v1".to_string());
                            let bg_section = BackgroundSection {
                                start_ms: parsed_line.start_ms,
                                end_ms: parsed_line.end_ms,
                                syllables: parsed_line.main_syllables,
                                ..Default::default()
                            };
                            lines.push(LyricLine {
                                start_ms: bg_section.start_ms,
                                end_ms: bg_section.end_ms,
                                agent: agent.clone(),
                                background_section: Some(bg_section),
                                ..Default::default()
                            });
                            last_main_line_index = Some(lines.len() - 1);
                            last_main_line_agent = agent;
                        }
                    } else {
                        // 这是主歌词行
                        let agent = match property {
                            1 | 4 => Some("v1".to_string()),
                            2 | 5 => Some("v2".to_string()),
                            _ => Some("v1".to_string()),
                        };
                        parsed_line.agent = agent.clone();

                        lines.push(parsed_line);
                        // 更新状态，以便后续背景行可以找到它
                        last_main_line_index = Some(lines.len() - 1);
                        last_main_line_agent = agent;
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

    // 按开始时间对所有行进行排序
    lines.sort_by_key(|l| l.start_ms);

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: LyricFormat::Lys,
        is_line_timed_source: false,
        ..Default::default()
    })
}
