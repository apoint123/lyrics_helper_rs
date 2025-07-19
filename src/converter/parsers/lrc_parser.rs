//! # LRC 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData, TranslationEntry,
    },
    utils::normalize_text_whitespace,
};

/// 用于匹配一个完整的 LRC 歌词行，捕获时间戳部分和文本部分
static LRC_LINE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:\[\d{2,}:\d{2}[.:]\d{2,3}\])+)(.*)$").expect("未能编译 LRC_LINE_REGEX")
});

/// 用于从一个时间戳组中提取出单个时间戳
static LRC_TIMESTAMP_EXTRACT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d{2,}):(\d{2})[.:](\d{2,3})\]").expect("未能编译 LRC_TIMESTAMP_EXTRACT_REGEX")
});

/// 用于匹配 [key:value] 格式的元数据标签
static LRC_METADATA_TAG_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[([a-zA-Z_][a-zA-Z0-9_]*):(.*?)\]$").expect("未能编译 LRC_METADATA_TAG_REGEX")
});

/// 解析 LRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_lrc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // 临时存储解析出的LRC行，用于后续排序和处理双语。
    // 每个条目代表一个时间戳和对应的文本行。
    struct TempLrcEntry {
        timestamp_ms: u64,
        text: String,
    }
    let mut temp_entries: Vec<TempLrcEntry> = Vec::new();

    for (line_num_zero_based, line_str_raw) in content.lines().enumerate() {
        let line_num_one_based = line_num_zero_based + 1;
        let line_str_trimmed = line_str_raw.trim();

        if line_str_trimmed.is_empty() {
            continue; // 跳过空行
        }

        // 解析元数据标签
        if let Some(meta_caps) = LRC_METADATA_TAG_REGEX.captures(line_str_trimmed) {
            let key = meta_caps
                .get(1)
                .map_or("", |m| m.as_str())
                .trim()
                .to_string();
            let value = meta_caps.get(2).map_or("", |m| m.as_str());
            let normalized_value = normalize_text_whitespace(value);

            if !key.is_empty() {
                raw_metadata.entry(key).or_default().push(normalized_value);
            }

            continue;
        }

        // 解析歌词行
        if let Some(line_caps) = LRC_LINE_REGEX.captures(line_str_trimmed) {
            let all_timestamps_str = line_caps.get(1).map_or("", |m| m.as_str());

            let raw_text_part = line_caps.get(2).map_or("", |m| m.as_str());
            let text_part = normalize_text_whitespace(raw_text_part);

            for ts_cap in LRC_TIMESTAMP_EXTRACT_REGEX.captures_iter(all_timestamps_str) {
                let minutes_str = ts_cap.get(1).map_or("0", |m| m.as_str());
                let seconds_str = ts_cap.get(2).map_or("0", |m| m.as_str());
                let fraction_str = ts_cap.get(3).map_or("0", |m| m.as_str());

                // 解析时间戳的各个部分
                let minutes = minutes_str.parse::<u64>();
                let seconds = seconds_str.parse::<u64>();
                let milliseconds = match fraction_str.len() {
                    2 => fraction_str.parse::<u64>().map(|f| f * 10),
                    3 => fraction_str.parse::<u64>(),
                    _ => {
                        warnings.push(format!(
                            "LRC解析警告 (行 {line_num_one_based}): 无效的毫秒部分长度 '{fraction_str}'."
                        ));
                        "invalid_length".parse::<u64>()
                    }
                };

                // 检查所有部分是否都解析成功
                if let (Ok(min), Ok(sec), Ok(ms)) = (minutes, seconds, milliseconds) {
                    if sec < 60 {
                        let total_ms = (min * 60 + sec) * 1000 + ms;
                        temp_entries.push(TempLrcEntry {
                            timestamp_ms: total_ms,
                            text: text_part.clone(),
                        });
                    } else {
                        warnings.push(format!(
                           "LRC解析警告 (行 {line_num_one_based}): 无效的时间戳秒数 '{seconds_str}'."
                        ));
                    }
                } else {
                    warnings.push(format!(
                        "LRC解析警告 (行 {}): 无法解析时间戳部分 '{}'。",
                        line_num_one_based,
                        ts_cap.get(0).map_or("", |m| m.as_str())
                    ));
                }
            }
        } else if !line_str_trimmed.is_empty() {
            warnings.push(format!(
                "LRC解析警告 (行 {line_num_one_based}): 无法识别的行格式 '{line_str_trimmed}'。"
            ));
        }
    }

    // 按时间戳对所有临时条目进行排序，以处理乱序的LRC文件
    temp_entries.sort_by_key(|e| e.timestamp_ms);

    // 构建最终的 LyricLine 列表，并处理双语LRC
    let mut final_lyric_lines: Vec<LyricLine> = Vec::new();
    let mut i = 0;
    while i < temp_entries.len() {
        let current_main_entry = &temp_entries[i];
        let mut current_translations = Vec::new();

        // 检查是否有紧随其后且时间戳相同的行，作为翻译
        let mut translation_lines_count = 0;
        while let Some(next_entry) = temp_entries.get(i + 1 + translation_lines_count) {
            if next_entry.timestamp_ms == current_main_entry.timestamp_ms {
                current_translations.push(TranslationEntry {
                    text: next_entry.text.clone(),
                    lang: None,
                });
                translation_lines_count += 1;
            } else {
                break;
            }
        }

        let start_ms = current_main_entry.timestamp_ms;
        final_lyric_lines.push(LyricLine {
            start_ms,
            end_ms: 0, // 结束时间将在下一步中计算
            line_text: Some(current_main_entry.text.clone()),
            main_syllables: vec![LyricSyllable {
                text: current_main_entry.text.clone(),
                start_ms,
                end_ms: 0, // 同样将在下一步计算
                duration_ms: None,
                ..Default::default()
            }],
            translations: current_translations,
            ..Default::default() // 其他字段使用默认值
        });

        i += 1 + translation_lines_count; // 跳过主歌词行和所有已处理的翻译行
    }

    // 再次遍历，以确定每行的结束时间
    if !final_lyric_lines.is_empty() {
        let num_lines = final_lyric_lines.len();
        for idx in 0..num_lines {
            let current_start_ms = final_lyric_lines[idx].start_ms;
            // 结束时间是下一行的开始时间，或者是最后一个的默认时长
            let end_ms = if idx + 1 < num_lines {
                final_lyric_lines[idx + 1]
                    .start_ms
                    .max(current_start_ms + 1)
            } else {
                current_start_ms + 10000
            };

            final_lyric_lines[idx].end_ms = end_ms;
            if let Some(syllable) = final_lyric_lines[idx].main_syllables.first_mut() {
                syllable.end_ms = end_ms;
                syllable.duration_ms = Some(end_ms.saturating_sub(current_start_ms));
            }
        }
    }

    Ok(ParsedSourceData {
        lines: final_lyric_lines,
        raw_metadata,
        source_format: LyricFormat::Lrc,
        is_line_timed_source: true,
        warnings,
        ..Default::default()
    })
}
