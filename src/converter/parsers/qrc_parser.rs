//! # QRC (Lyricify 标准) 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        BackgroundSection, ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
    },
    utils::{parse_and_store_metadata, process_syllable_text},
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

    // unwrap 是安全的，因为 captures() 已确认匹配成功
    let content_after_line_ts = &line_str[line_ts_cap.get(0).unwrap().end()..];

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_match_end = 0;

    // 遍历所有音节时间戳，提取其前面的文本作为音节
    for captures in WORD_TIMESTAMP_REGEX.captures_iter(content_after_line_ts) {
        let full_tag_match = captures.get(0).unwrap();
        let raw_text_slice = &content_after_line_ts[last_match_end..full_tag_match.start()];

        if let Some((clean_text, ends_with_space)) =
            process_syllable_text(raw_text_slice, &mut syllables)
        {
            let syl_start_ms: u64 = captures["start"].parse().map_err(ConvertError::ParseInt)?;
            let syl_duration_ms: u64 = captures["duration"]
                .parse()
                .map_err(ConvertError::ParseInt)?;

            syllables.push(LyricSyllable {
                text: clean_text,
                start_ms: syl_start_ms,
                end_ms: syl_start_ms + syl_duration_ms,
                duration_ms: Some(syl_duration_ms),
                ends_with_space,
            });
        }
        last_match_end = full_tag_match.end();
    }

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

        match parse_qrc_line_raw(trimmed_line, line_num) {
            Ok((is_background, mut parsed_line)) => {
                if is_background {
                    if let Some(main_line) = lines.last_mut() {
                        if main_line.background_section.is_none() {
                            // 第一个背景行，正常关联，并移除括号
                            for syl in &mut parsed_line.main_syllables {
                                syl.text =
                                    syl.text.trim_matches(['(', '（', ')', '）']).to_string();
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
                        }
                    } else {
                        // 孤立的背景行，也提升为主歌词行，保留括号
                        warnings.push(format!(
                            "第 {line_num} 行: 在任何主歌词行之前发现背景行，将提升为主歌词行。"
                        ));
                        lines.push(parsed_line);
                    }
                } else {
                    // 主歌词行
                    lines.push(parsed_line);
                }
            }
            Err(e) => {
                warnings.push(format!("第 {line_num} 行: 解析失败，已跳过。错误: {e}"));
            }
        }
    }

    lines.sort_by_key(|l| l.start_ms);

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: LyricFormat::Qrc,
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
    fn test_parse_simple_qrc_lines() {
        let content = r#"
        [ti:QRC Test]
        [ar:Tester]
        [100,500]Hello(100,200) world(300,300)
        "#;
        let result = parse_qrc(content).unwrap();

        assert_eq!(
            result.raw_metadata.get("ti"),
            Some(&vec!["QRC Test".to_string()])
        );
        assert_eq!(
            result.raw_metadata.get("ar"),
            Some(&vec!["Tester".to_string()])
        );

        assert_eq!(result.lines.len(), 1);
        let line1 = &result.lines[0];
        assert_eq!(line1.start_ms, 100);
        assert_eq!(line1.end_ms, 600);
        assert_eq!(line1.main_syllables.len(), 2);
        assert_eq!(
            line1.main_syllables[0],
            new_syllable("Hello", 100, 300, true)
        );
        assert_eq!(
            line1.main_syllables[1],
            new_syllable("world", 300, 600, false)
        );
    }

    #[test]
    fn test_associate_background_line() {
        let content = "[100,500]Main(100,200) line(300,300)\n[600,200](Background)(600,200)";
        let result = parse_qrc(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        let line = &result.lines[0];

        assert_eq!(line.main_syllables.len(), 2);

        assert!(line.background_section.is_some());
        let bg_section = line.background_section.as_ref().unwrap();
        assert_eq!(bg_section.syllables.len(), 1);
        assert_eq!(bg_section.syllables[0].text, "Background");
    }

    #[test]
    fn test_promote_consecutive_background_lines() {
        let content = "[100,200]Main(100,200)\n[300,200](BG 1)(300,200)\n[500,200](BG 2)(500,200)";
        let result = parse_qrc(content).unwrap();

        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("连续的背景行"));

        let line1 = &result.lines[0];
        assert_eq!(line1.main_syllables[0].text, "Main");
        let bg1 = line1.background_section.as_ref().unwrap();
        assert_eq!(bg1.syllables[0].text, "BG 1");

        let line2 = &result.lines[1];
        assert_eq!(line2.main_syllables[0].text, "(BG 2)");
        assert!(line2.background_section.is_none());
    }

    #[test]
    fn test_promote_orphan_background_line() {
        let content = "[100,200](Orphan BG)(100,200)";
        let result = parse_qrc(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("在任何主歌词行之前发现背景行"));

        let line = &result.lines[0];
        assert_eq!(line.main_syllables[0].text, "(Orphan BG)");
    }

    #[test]
    fn test_sorting_of_out_of_order_lines() {
        let content = "[1000,200]Second(1000,200)\n[100,200]First(100,200)";
        let result = parse_qrc(content).unwrap();

        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.lines[0].start_ms, 100);
        assert_eq!(result.lines[1].start_ms, 1000);
    }

    #[test]
    fn test_invalid_line_is_skipped_with_warning() {
        let content = "This is not a valid line.\n[100,200]Valid(100,200)";
        let result = parse_qrc(content).unwrap();

        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].main_syllables[0].text, "Valid");
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("解析失败"));
    }

    #[test]
    fn test_use_of_process_syllable_text_logic() {
        let content = "[100,500]Word1 (100,100) Word2(300,200)";
        let result = parse_qrc(content).unwrap();

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
