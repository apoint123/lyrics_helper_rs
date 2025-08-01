//! # SPL (Salt Player Lyrics) 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData, TranslationEntry,
    },
    utils::process_syllable_text,
};

/// 匹配并捕获一个或多个行首时间戳 `[...]`
static SPL_LEADING_TIMESTAMPS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^((\[\d{1,3}:\d{1,2}\.\d{1,6}])+)(.*)$").unwrap());
/// 在任意位置查找一个时间戳（方括号 `[]` 或尖括号 `<>`）
static SPL_ANY_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d{1,3}:\d{1,2}\.\d{1,6})\]|<(\d{1,3}:\d{1,2}\.\d{1,6})>").unwrap()
});

/// 临时结构，用于在解析时暂存一个逻辑块的信息。
#[derive(Debug, Default, Clone)]
struct SplBlock {
    start_times: Vec<u64>,
    main_text: String,
    translations: Vec<String>,
    explicit_end_ms: Option<u64>,
}

/// 解析SPL时间戳字符串（例如 "05:20.22"）到毫秒。
fn parse_spl_timestamp_ms(ts_str: &str) -> Result<u64, ConvertError> {
    let parts: Vec<&str> = ts_str.split([':', '.']).collect();
    if parts.len() != 3 {
        return Err(ConvertError::InvalidTime(format!(
            "无效的SPL时间戳格式: {ts_str}"
        )));
    }
    let minutes: u64 = parts[0].parse().map_err(ConvertError::ParseInt)?;
    let seconds: u64 = parts[1].parse().map_err(ConvertError::ParseInt)?;
    let fraction_str = parts[2];

    // 处理毫秒部分
    let milliseconds: u64 = match fraction_str.len() {
        1 => fraction_str.parse::<u64>()? * 100,
        2 => fraction_str.parse::<u64>()? * 10,
        3..=6 => {
            let valid_part = &fraction_str[..3.min(fraction_str.len())];
            valid_part.parse::<u64>()? * 10u64.pow(3 - valid_part.len() as u32)
        }
        _ => {
            return Err(ConvertError::InvalidTime(format!(
                "无效的毫秒部分: '{fraction_str}'"
            )));
        }
    };
    Ok((minutes * 60 + seconds) * 1000 + milliseconds)
}

/// 解析一行文本中的逐字音节。
fn parse_syllables(
    text: &str,
    line_start_ms: u64,
    line_end_ms: u64,
) -> Result<Vec<LyricSyllable>, ConvertError> {
    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_pos = 0;
    let mut current_time = line_start_ms;

    for m in SPL_ANY_TIMESTAMP_REGEX.find_iter(text) {
        let raw_segment_text = &text[last_pos..m.start()];
        last_pos = m.end();

        let caps = SPL_ANY_TIMESTAMP_REGEX.captures(m.as_str()).unwrap();
        let next_time = if let Some(ts_content) = caps.get(1).or_else(|| caps.get(2)) {
            parse_spl_timestamp_ms(ts_content.as_str())?
        } else {
            current_time
        };

        if let Some((clean_text, ends_with_space)) =
            process_syllable_text(raw_segment_text, &mut syllables)
        {
            syllables.push(LyricSyllable {
                text: clean_text,
                start_ms: current_time,
                end_ms: next_time,
                duration_ms: Some(next_time.saturating_sub(current_time)),
                ends_with_space,
            });
        }
        current_time = next_time;
    }

    let raw_last_segment = &text[last_pos..];
    if let Some((clean_text, _)) = process_syllable_text(raw_last_segment, &mut syllables) {
        syllables.push(LyricSyllable {
            text: clean_text,
            start_ms: current_time,
            end_ms: line_end_ms,
            duration_ms: Some(line_end_ms.saturating_sub(current_time)),
            ends_with_space: false,
        });
    }
    Ok(syllables)
}

/// 解析 SPL 格式内容到 `ParsedSourceData` 结构。
pub fn parse_spl(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut spl_blocks: Vec<SplBlock> = Vec::new();

    // 将原始文本行构建成逻辑块 (SplBlock)
    let mut line_iterator = content.lines().enumerate().peekable();
    while let Some((line_num, line_str)) = line_iterator.next() {
        let line_num = line_num + 1;
        let trimmed_line = line_str.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        let mut current_block = SplBlock::default();

        // 解析行首时间戳
        if let Some(caps) = SPL_LEADING_TIMESTAMPS_REGEX.captures(trimmed_line) {
            if let Some(timestamps_str) = caps.get(1) {
                for ts_cap in SPL_ANY_TIMESTAMP_REGEX.captures_iter(timestamps_str.as_str()) {
                    if let Some(ts_content) = ts_cap.get(1) {
                        match parse_spl_timestamp_ms(ts_content.as_str()) {
                            Ok(ms) => current_block.start_times.push(ms),
                            Err(e) => warnings.push(format!("第 {line_num} 行: {e}")),
                        }
                    }
                }
            }
            let mut text_to_process = caps.get(3).map_or("", |m| m.as_str()).to_string();

            // 检查并提取行尾的显式结束时间戳
            if let Some(last_ts_match) = SPL_ANY_TIMESTAMP_REGEX.find_iter(&text_to_process).last()
                && last_ts_match.end() == text_to_process.len()
            {
                let caps_inner = SPL_ANY_TIMESTAMP_REGEX
                    .captures(last_ts_match.as_str())
                    .unwrap();
                if let Some(ts_content) = caps_inner.get(1) {
                    // 显式结束必须是 [...]
                    if let Ok(ms) = parse_spl_timestamp_ms(ts_content.as_str()) {
                        current_block.explicit_end_ms = Some(ms);
                        text_to_process.truncate(last_ts_match.start());
                    }
                }
            }
            current_block.main_text = text_to_process.trim_end().to_string();

            // 检查并关联翻译行
            while let Some(&(_, next_line_str)) = line_iterator.peek() {
                let next_trimmed = next_line_str.trim();
                if next_trimmed.is_empty() {
                    line_iterator.next();
                    continue;
                }

                // 检查下一行是否为同时间戳翻译
                if let Some(next_caps) = SPL_LEADING_TIMESTAMPS_REGEX.captures(next_trimmed) {
                    let mut next_timestamps = Vec::new();
                    if let Some(timestamps_str) = next_caps.get(1) {
                        for ts_cap in SPL_ANY_TIMESTAMP_REGEX.captures_iter(timestamps_str.as_str())
                        {
                            if let Some(ts_content) = ts_cap.get(1)
                                && let Ok(ms) = parse_spl_timestamp_ms(ts_content.as_str())
                            {
                                next_timestamps.push(ms);
                            }
                        }
                    }
                    if !next_timestamps.is_empty() && next_timestamps == current_block.start_times {
                        current_block
                            .translations
                            .push(next_caps.get(3).map_or("", |m| m.as_str()).to_string());
                        line_iterator.next(); // 消耗翻译行
                    } else {
                        break; // 时间戳不同，不是翻译
                    }
                } else {
                    // 下一行没有时间戳，是隐式翻译
                    current_block.translations.push(next_trimmed.to_string());
                    line_iterator.next();
                }
            }
            spl_blocks.push(current_block);
        } else {
            warnings.push(format!(
                "第 {line_num} 行: 跳过无时间戳的孤立行 '{trimmed_line}'"
            ));
        }
    }

    // 将逻辑块转换为最终的 LyricLine
    for (i, block) in spl_blocks.iter().enumerate() {
        let end_time = if let Some(explicit_end) = block.explicit_end_ms {
            explicit_end
        } else {
            // 隐式结束时间是下一个块的开始时间
            spl_blocks
                .get(i + 1)
                .and_then(|next_block| next_block.start_times.first())
                .copied()
                .unwrap_or_else(|| block.start_times.first().unwrap_or(&0) + 5000)
        };

        let syllables = parse_syllables(
            &block.main_text,
            *block.start_times.first().unwrap_or(&0),
            end_time,
        )?;
        let is_word_timed = !syllables.is_empty() && syllables.len() > 1;

        if block.start_times.len() > 1 && is_word_timed {
            warnings.push(format!(
                "在主歌词 '{}' 中同时使用了重复行和逐字歌词特性，这可能导致非预期的行为。",
                block.main_text
            ));
        }

        // 为每个开始时间戳创建 LyricLine
        for &start_ms in &block.start_times {
            let full_line_text = syllables
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
                start_ms,
                end_ms: end_time,
                line_text: Some(full_line_text.clone()),
                main_syllables: if is_word_timed {
                    syllables.clone()
                } else {
                    Vec::new()
                },
                translations: block
                    .translations
                    .iter()
                    .map(|t| TranslationEntry {
                        text: t.clone(),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            });
        }
    }

    // 按开始时间对所有最终行进行排序
    lines.sort_by_key(|l| l.start_ms);

    Ok(ParsedSourceData {
        lines: lines.clone(),
        raw_metadata: HashMap::new(),
        warnings,
        source_format: LyricFormat::Spl,
        is_line_timed_source: lines.iter().any(|l| l.main_syllables.is_empty()),
        ..Default::default()
    })
}
