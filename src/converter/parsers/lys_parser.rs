//! # LYS 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        BackgroundSection, ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
        lys_properties,
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

    // unwrap 是安全的，因为 property_cap 在前面已确认存在
    let content_after_property = &line_str[property_cap.get(0).unwrap().end()..];

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_match_end = 0;
    let mut min_start_ms = u64::MAX;
    let mut max_end_ms = 0;

    for ts_cap in LYS_TIMESTAMP_REGEX.captures_iter(content_after_property) {
        // unwrap 是安全的，因为 captures_iter 只会返回成功的匹配。
        let full_match = ts_cap.get(0).unwrap();

        let raw_text_slice = &content_after_property[last_match_end..full_match.start()];

        if let Some((clean_text, ends_with_space)) =
            process_syllable_text(raw_text_slice, &mut syllables)
        {
            let start_ms: u64 = ts_cap["start"].parse().map_err(ConvertError::ParseInt)?;
            let duration_ms: u64 = ts_cap["duration"].parse().map_err(ConvertError::ParseInt)?;

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
        last_match_end = full_match.end();
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

        match parse_lys_line(trimmed_line, line_num) {
            Ok((property, mut parsed_line)) => {
                let is_background = matches!(
                    property,
                    lys_properties::BG_UNSET..=lys_properties::BG_RIGHT
                );

                if is_background {
                    if let Some(main_line) = lines.last_mut() {
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
                            parsed_line.agent = main_line.agent.clone();
                            lines.push(parsed_line);
                        }
                    } else {
                        warnings.push(format!(
                            "第 {line_num} 行: 背景行出现在任何主歌词行之前，将提升为主歌词行。"
                        ));
                        parsed_line.agent = Some("v1".to_string());
                        lines.push(parsed_line);
                    }
                } else {
                    // 这是主歌词行
                    let agent = match property {
                        lys_properties::UNSET_RIGHT | lys_properties::MAIN_RIGHT => {
                            Some("v2".to_string())
                        }
                        lys_properties::UNSET_UNSET
                        | lys_properties::UNSET_LEFT
                        | lys_properties::MAIN_UNSET
                        | lys_properties::MAIN_LEFT => Some("v1".to_string()),
                        _ => {
                            warnings.push(format!(
                                "第 {line_num} 行: 未定义的 LYS 属性值 `{property}`。"
                            ));
                            Some("v1".to_string())
                        }
                    };
                    parsed_line.agent = agent;
                    lines.push(parsed_line);
                }
            }
            Err(e) => {
                warnings.push(format!("第 {line_num} 行: 解析失败，已跳过。错误: {e}"));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::types::LyricSyllable;

    fn new_syllable(text: &str, start: u64, end: u64, space: bool) -> LyricSyllable {
        LyricSyllable {
            text: text.to_string(),
            start_ms: start,
            end_ms: end,
            duration_ms: Some(end - start),
            ends_with_space: space,
        }
    }

    #[test]
    fn test_parse_simple_main_lines() {
        let content = r#"
        [ti:Test Title]
        [ar:Test Artist]
        [4]Hello(100,200) world(300,300)
        [5]Another(1000,200) line(1200,300)
        "#;
        let result = parse_lys(content).unwrap();

        assert_eq!(
            result.raw_metadata.get("ti"),
            Some(&vec!["Test Title".to_string()])
        );
        assert_eq!(
            result.raw_metadata.get("ar"),
            Some(&vec!["Test Artist".to_string()])
        );

        assert_eq!(result.lines.len(), 2);

        let line1 = &result.lines[0];
        assert_eq!(line1.start_ms, 100);
        assert_eq!(line1.end_ms, 600);
        assert_eq!(line1.agent, Some("v1".to_string()));
        assert_eq!(line1.main_syllables.len(), 2);

        assert_eq!(
            line1.main_syllables[0],
            new_syllable("Hello", 100, 300, true)
        );
        assert_eq!(
            line1.main_syllables[1],
            new_syllable("world", 300, 600, false)
        );

        let line2 = &result.lines[1];
        assert_eq!(line2.start_ms, 1000);
        assert_eq!(line2.agent, Some("v2".to_string()));
        assert_eq!(line2.main_syllables.len(), 2);
    }

    #[test]
    fn test_parse_with_background_lines() {
        let content = "[4]Main(100,200) vocal(300,300)\n[7](Background)(500,400)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        let line = &result.lines[0];

        assert!(!line.main_syllables.is_empty());
        assert_eq!(line.main_syllables[0].text, "Main");

        assert!(line.background_section.is_some());
        let bg_section = line.background_section.as_ref().unwrap();
        assert_eq!(bg_section.start_ms, 500);
        assert_eq!(bg_section.syllables.len(), 1);
        assert_eq!(bg_section.syllables[0].text, "(Background)");
    }

    #[test]
    fn test_promote_consecutive_background_lines() {
        let content = "[4]Main(100,200)\n[7](BG 1)(300,200)\n[7](BG 2)(500,200)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("连续的背景行"));

        let line1 = &result.lines[0];
        assert_eq!(line1.agent, Some("v1".to_string()));
        assert_eq!(line1.main_syllables[0].text, "Main");
        assert!(line1.background_section.is_some());
        assert_eq!(
            line1.background_section.as_ref().unwrap().syllables[0].text,
            "(BG 1)"
        );

        let line2 = &result.lines[1];
        assert_eq!(line2.agent, Some("v1".to_string()));
        assert_eq!(line2.main_syllables[0].text, "(BG 2)");
        assert!(line2.background_section.is_none());
    }

    #[test]
    fn test_promote_background_line_at_start() {
        let content = "[6](Orphan BG)(100,200)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("背景行出现在任何主歌词行之前"));

        let line = &result.lines[0];
        assert_eq!(line.agent, Some("v1".to_string()));
        assert_eq!(line.main_syllables[0].text, "(Orphan BG)");
    }

    #[test]
    fn test_sorting_of_out_of_order_lines() {
        let content = "[4]Second line(1000,200)\n[4]First line(100,200)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.lines[0].start_ms, 100);
        assert_eq!(result.lines[0].main_syllables[0].text, "First line");
        assert_eq!(result.lines[1].start_ms, 1000);
        assert_eq!(result.lines[1].main_syllables[0].text, "Second line");
    }

    #[test]
    fn test_invalid_line_is_skipped_with_warning() {
        let content = "This is not a valid line.\n[4]This is a valid line(100,200)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        assert_eq!(
            result.lines[0].main_syllables[0].text,
            "This is a valid line"
        );

        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("解析失败"));
    }

    #[test]
    fn test_empty_and_metadata_only_input() {
        let result_empty = parse_lys("").unwrap();
        assert!(result_empty.lines.is_empty());
        assert!(result_empty.raw_metadata.is_empty());

        let content_meta = "[ti:Title]\n[offset:0]";
        let result_meta = parse_lys(content_meta).unwrap();
        assert!(result_meta.lines.is_empty());
        assert_eq!(result_meta.raw_metadata.len(), 2);
    }

    #[test]
    fn test_space_syllable_parsing() {
        let content = "[4]Word1(100,100) (0,0)Word2(200,100)";
        let result = parse_lys(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        let line = &result.lines[0];
        assert_eq!(line.main_syllables.len(), 2);

        let syl1 = &line.main_syllables[0];
        assert_eq!(syl1.text, "Word1");
        assert!(syl1.ends_with_space);

        let syl2 = &line.main_syllables[1];
        assert_eq!(syl2.text, "Word2");
        assert!(!syl2.ends_with_space);
    }
}
