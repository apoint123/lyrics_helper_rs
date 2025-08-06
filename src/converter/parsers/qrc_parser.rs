//! # QRC (Lyricify 标准) 格式解析器

use std::collections::HashMap;

use crate::converter::{
    types::{
        AnnotatedTrack, ContentType, ConvertError, LyricFormat, LyricLine, LyricSyllable,
        LyricTrack, ParsedSourceData, Word,
    },
    utils::{parse_and_store_metadata, process_syllable_text},
};
use regex::Regex;
use std::sync::LazyLock;

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

    let original_line_start_ms: u64 = line_ts_cap["start"].parse()?;
    let original_line_duration_ms: u64 = line_ts_cap["duration"].parse()?;
    let content_after_line_ts = &line_str[line_ts_cap.get(0).unwrap().end()..];

    let mut syllables: Vec<LyricSyllable> = Vec::new();
    let mut last_match_end = 0;

    for captures in WORD_TIMESTAMP_REGEX.captures_iter(content_after_line_ts) {
        let full_tag_match = captures.get(0).unwrap();
        let raw_text_slice = &content_after_line_ts[last_match_end..full_tag_match.start()];

        if let Some((clean_text, ends_with_space)) =
            process_syllable_text(raw_text_slice, &mut syllables)
        {
            let syl_start_ms: u64 = captures["start"].parse()?;
            let syl_duration_ms: u64 = captures["duration"].parse()?;

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

    let (final_start_ms, final_end_ms) = if !syllables.is_empty() {
        (
            syllables.first().unwrap().start_ms,
            syllables.last().unwrap().end_ms,
        )
    } else {
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

    let is_background_candidate = (full_line_text.starts_with('(')
        || full_line_text.starts_with('（'))
        && (full_line_text.ends_with(')') || full_line_text.ends_with('）'));

    let words = vec![Word {
        syllables,
        ..Default::default()
    }];

    let content_track = LyricTrack {
        words,
        ..Default::default()
    };
    let annotated_track = AnnotatedTrack {
        content_type: ContentType::Main,
        content: content_track,
        ..Default::default()
    };
    let lyric_line = LyricLine {
        start_ms: final_start_ms,
        end_ms: final_end_ms,
        tracks: vec![annotated_track],
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
        if parse_and_store_metadata(trimmed_line, &mut raw_metadata) {
            continue;
        }

        match parse_qrc_line_raw(trimmed_line, line_num) {
            Ok((is_background, mut parsed_line)) => {
                if is_background {
                    if let Some(main_line) = lines.last_mut() {
                        let main_line_has_bg = main_line
                            .tracks
                            .iter()
                            .any(|at| at.content_type == ContentType::Background);

                        if !main_line_has_bg {
                            // 第一个背景行，关联到主行
                            if let Some(mut bg_annotated_track) = parsed_line.tracks.pop() {
                                bg_annotated_track.content_type = ContentType::Background;
                                // 移除括号
                                for word in &mut bg_annotated_track.content.words {
                                    for syl in &mut word.syllables {
                                        syl.text = syl
                                            .text
                                            .trim_matches(['(', '（', ')', '）'])
                                            .to_string();
                                    }
                                    word.syllables.retain(|s| !s.text.is_empty());
                                }
                                bg_annotated_track
                                    .content
                                    .words
                                    .retain(|w| !w.syllables.is_empty());

                                if !bg_annotated_track.content.words.is_empty() {
                                    main_line.tracks.push(bg_annotated_track);
                                }
                            }
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
        let line1 = &result.lines[0];
        assert_eq!(line1.start_ms, 100);
        assert_eq!(line1.end_ms, 600);
        let main_track = &line1.tracks[0].content;
        assert_eq!(main_track.words.len(), 1, "所有音节应在一个Word中");
        assert_eq!(main_track.words[0].syllables.len(), 2);
        assert_eq!(
            main_track.words[0].syllables[0],
            new_syllable("Hello", 100, 300, true)
        );
        assert_eq!(
            main_track.words[0].syllables[1],
            new_syllable("world", 300, 600, false)
        );
    }

    #[test]
    fn test_associate_background_line() {
        let content = "[100,500]Main(100,200) line(300,300)\n[600,200](Background)(600,200)";
        let result = parse_qrc(content).unwrap();
        assert_eq!(result.lines.len(), 1);
        let line = &result.lines[0];
        assert_eq!(line.tracks.len(), 2); // Main and Background
        let bg_track = line
            .tracks
            .iter()
            .find(|at| at.content_type == ContentType::Background)
            .unwrap();
        assert_eq!(bg_track.content.words[0].syllables[0].text, "Background");
    }

    #[test]
    fn test_promote_consecutive_background_lines() {
        let content = "[100,200]Main(100,200)\n[300,200](BG 1)(300,200)\n[500,200](BG 2)(500,200)";
        let result = parse_qrc(content).unwrap();
        assert_eq!(result.lines.len(), 2);
        assert_eq!(result.warnings.len(), 1);

        let line1 = &result.lines[0];
        let bg1 = line1
            .tracks
            .iter()
            .find(|at| at.content_type == ContentType::Background)
            .unwrap();
        assert_eq!(bg1.content.words[0].syllables[0].text, "BG 1");

        let line2 = &result.lines[1];
        assert_eq!(line2.tracks.len(), 1);
        assert_eq!(line2.tracks[0].content_type, ContentType::Main);
        assert_eq!(line2.tracks[0].content.words[0].syllables[0].text, "(BG 2)");
    }
}
