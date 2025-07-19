//! # LRC 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData, TranslationEntry,
    },
    utils::{normalize_text_whitespace, parse_and_store_metadata},
};

/// 用于匹配一个完整的 LRC 歌词行，捕获时间戳部分和文本部分
static LRC_LINE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:\[\d{2,}:\d{2}[.:]\d{2,3}\])+)(.*)$").expect("未能编译 LRC_LINE_REGEX")
});

/// 用于从一个时间戳组中提取出单个时间戳
static LRC_TIMESTAMP_EXTRACT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d{2,}):(\d{2})[.:](\d{2,3})\]").expect("未能编译 LRC_TIMESTAMP_EXTRACT_REGEX")
});

const DEFAULT_LAST_LINE_DURATION_MS: u64 = 10000;

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
        if parse_and_store_metadata(line_str_trimmed, &mut raw_metadata) {
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
        if temp_entries[i].text.is_empty() {
            i += 1;
            continue;
        }

        let current_main_entry = &temp_entries[i];
        let mut current_translations = Vec::new();

        // 检查是否有紧随其后且时间戳相同的行，作为翻译
        let mut next_event_index = i + 1;
        while let Some(next_entry) = temp_entries.get(next_event_index) {
            if next_entry.timestamp_ms == current_main_entry.timestamp_ms {
                if !next_entry.text.is_empty() {
                    current_translations.push(TranslationEntry {
                        text: next_entry.text.clone(),
                        lang: None,
                    });
                }
                next_event_index += 1;
            } else {
                break;
            }
        }

        let start_ms = current_main_entry.timestamp_ms;

        let end_ms = if let Some(next_event) = temp_entries.get(next_event_index) {
            next_event.timestamp_ms.max(start_ms + 1)
        } else {
            start_ms + DEFAULT_LAST_LINE_DURATION_MS
        };

        let duration = end_ms.saturating_sub(start_ms);

        final_lyric_lines.push(LyricLine {
            start_ms,
            end_ms,
            line_text: Some(current_main_entry.text.clone()),
            main_syllables: vec![LyricSyllable {
                text: current_main_entry.text.clone(),
                start_ms,
                end_ms,
                duration_ms: Some(duration),
                ..Default::default()
            }],
            translations: current_translations,
            ..Default::default()
        });

        i = next_event_index;
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_simple_lrc() {
        let content = r#"
        [00:10.00]Line 1
        [00:12.50]Line 2
        "#;
        let parsed_data = parse_lrc(content).unwrap();
        assert_eq!(parsed_data.lines.len(), 2);

        let line1 = &parsed_data.lines[0];
        assert_eq!(line1.start_ms, 10000);
        assert_eq!(line1.end_ms, 12500);
        assert_eq!(line1.line_text.as_deref(), Some("Line 1"));

        let line2 = &parsed_data.lines[1];
        assert_eq!(line2.start_ms, 12500);
        assert_eq!(line2.end_ms, 12500 + DEFAULT_LAST_LINE_DURATION_MS);
        assert_eq!(line2.line_text.as_deref(), Some("Line 2"));
    }

    #[test]
    fn test_lrc_handles_pause_line() {
        let content = r#"
        [01:31.460]第一行
        [01:35.840]
        [01:54.660]第二行
        "#;
        let parsed_data = parse_lrc(content).unwrap();

        assert_eq!(parsed_data.lines.len(), 2, "应只生成2行有效歌词");

        let line1 = &parsed_data.lines[0];
        assert_eq!(line1.start_ms, 91460);
        assert_eq!(line1.end_ms, 95840, "结束时间应是空行的时间戳");
        assert_eq!(line1.line_text.as_deref(), Some("第一行"));

        let line2 = &parsed_data.lines[1];
        assert_eq!(line2.start_ms, 114660);
    }

    #[test]
    fn test_bilingual_lrc_parsing() {
        let content = r#"
        [00:20.00]Hello world
        [00:20.00]你好世界
        [00:22.00]Next line
        "#;
        let parsed_data = parse_lrc(content).unwrap();

        assert_eq!(parsed_data.lines.len(), 2, "相同时间戳的行应被合并");

        let line1 = &parsed_data.lines[0];
        assert_eq!(line1.start_ms, 20000);
        assert_eq!(line1.line_text.as_deref(), Some("Hello world"));
        assert_eq!(line1.translations.len(), 1, "应有1行翻译");
        assert_eq!(line1.translations[0].text, "你好世界");

        let line2 = &parsed_data.lines[1];
        assert_eq!(line2.start_ms, 22000);
    }

    #[test]
    fn test_out_of_order_and_multi_timestamp_lrc() {
        let content = r#"
        [00:30.00]Chorus line
        [00:10.00][00:50.00]Verse line
        [00:20.00]Another line
        "#;
        let parsed_data = parse_lrc(content).unwrap();

        assert_eq!(parsed_data.lines.len(), 4);

        assert_eq!(parsed_data.lines[0].start_ms, 10000);
        assert_eq!(
            parsed_data.lines[0].line_text.as_deref(),
            Some("Verse line")
        );

        assert_eq!(parsed_data.lines[1].start_ms, 20000);
        assert_eq!(
            parsed_data.lines[1].line_text.as_deref(),
            Some("Another line")
        );

        assert_eq!(parsed_data.lines[2].start_ms, 30000);
        assert_eq!(
            parsed_data.lines[2].line_text.as_deref(),
            Some("Chorus line")
        );

        assert_eq!(parsed_data.lines[3].start_ms, 50000);
        assert_eq!(
            parsed_data.lines[3].line_text.as_deref(),
            Some("Verse line")
        );
    }

    #[test]
    fn test_whitespace_normalization_and_metadata() {
        let content = r#"
        [ti:  My Song Title  ]
        [ar:The Artist   ]
        [00:05.123]   leading and trailing spaces
        [00:08.45]multiple   internal    spaces
        "#;
        let parsed_data = parse_lrc(content).unwrap();

        assert_eq!(
            parsed_data.raw_metadata.get("ti").unwrap()[0],
            "My Song Title"
        );
        assert_eq!(parsed_data.raw_metadata.get("ar").unwrap()[0], "The Artist");

        assert_eq!(
            parsed_data.lines[0].line_text.as_deref(),
            Some("leading and trailing spaces")
        );
        assert_eq!(
            parsed_data.lines[1].line_text.as_deref(),
            Some("multiple internal spaces")
        );
    }
}
