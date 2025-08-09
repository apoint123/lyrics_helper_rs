//! # LRC 格式解析器

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        AnnotatedTrack, ContentType, ConvertError, LyricFormat, LyricLine, LyricSyllable,
        LyricTrack, ParsedSourceData, Word,
    },
    utils::{normalize_text_whitespace, parse_and_store_metadata},
};

/// 用于匹配一个完整的 LRC 歌词行，捕获时间戳部分和文本部分
static LRC_LINE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:\[\d{2,}:\d{2}[.:]\d{2,3}])+)(.*)$").expect("未能编译 LRC_LINE_REGEX")
});

/// 用于从一个时间戳组中提取出单个时间戳
static LRC_TIMESTAMP_EXTRACT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d{2,}):(\d{2})[.:](\d{2,3})]").expect("未能编译 LRC_TIMESTAMP_EXTRACT_REGEX")
});

const DEFAULT_LAST_LINE_DURATION_MS: u64 = 10000;

/// 解析 LRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_lrc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    struct TempLrcEntry {
        timestamp_ms: u64,
        text: String,
    }

    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    let mut temp_entries: Vec<TempLrcEntry> = Vec::new();

    for (line_num, line_str) in content.lines().enumerate() {
        let line_str_trimmed = line_str.trim();
        if line_str_trimmed.is_empty()
            || parse_and_store_metadata(line_str_trimmed, &mut raw_metadata)
        {
            continue;
        }

        if let Some(line_caps) = LRC_LINE_REGEX.captures(line_str_trimmed) {
            let all_timestamps_str = line_caps.get(1).map_or("", |m| m.as_str());
            let raw_text_part = line_caps.get(2).map_or("", |m| m.as_str());
            let text_part = normalize_text_whitespace(raw_text_part);

            for ts_cap in LRC_TIMESTAMP_EXTRACT_REGEX.captures_iter(all_timestamps_str) {
                let minutes: u64 = ts_cap[1].parse()?;
                let seconds: u64 = ts_cap[2].parse()?;
                let fraction_str = &ts_cap[3];
                let milliseconds: Result<u64, ConvertError> = match fraction_str.len() {
                    2 => Ok(fraction_str.parse::<u64>().map(|f| f * 10)?),
                    3 => Ok(fraction_str.parse::<u64>()?),
                    _ => Err(ConvertError::InvalidTime(format!(
                        "无效的毫秒部分: {fraction_str}"
                    ))),
                };
                if let Ok(ms) = milliseconds {
                    if seconds < 60 {
                        temp_entries.push(TempLrcEntry {
                            timestamp_ms: (minutes * 60 + seconds) * 1000 + ms,
                            text: text_part.clone(),
                        });
                    } else {
                        warnings.push(format!("LRC秒数无效 (行 {}): '{}'", line_num + 1, seconds));
                    }
                }
            }
        }
    }

    temp_entries.sort_by_key(|e| e.timestamp_ms);

    let mut final_lyric_lines: Vec<LyricLine> = Vec::new();
    let mut i = 0;
    while i < temp_entries.len() {
        if temp_entries[i].text.is_empty() {
            i += 1;
            continue;
        }

        let current_main_entry = &temp_entries[i];
        let start_ms = current_main_entry.timestamp_ms;

        let main_track = LyricTrack {
            words: vec![Word {
                syllables: vec![LyricSyllable {
                    text: current_main_entry.text.clone(),
                    start_ms,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let mut translations = vec![];
        let mut next_event_index = i + 1;
        while let Some(next_entry) = temp_entries.get(next_event_index) {
            if next_entry.timestamp_ms == start_ms {
                if !next_entry.text.is_empty() {
                    translations.push(LyricTrack {
                        words: vec![Word {
                            syllables: vec![LyricSyllable {
                                text: next_entry.text.clone(),
                                start_ms,
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    });
                }
                next_event_index += 1;
            } else {
                break;
            }
        }

        let end_ms = temp_entries
            .get(next_event_index)
            .map_or(start_ms + DEFAULT_LAST_LINE_DURATION_MS, |next| {
                next.timestamp_ms.max(start_ms)
            });

        let annotated_track = AnnotatedTrack {
            content_type: ContentType::Main,
            content: main_track,
            translations,
            romanizations: vec![],
        };

        let mut final_line = LyricLine {
            start_ms,
            end_ms,
            tracks: vec![annotated_track],
            ..Default::default()
        };

        for track in &mut final_line.tracks {
            track.content.words[0].syllables[0].end_ms = end_ms;
            for trans_track in &mut track.translations {
                trans_track.words[0].syllables[0].end_ms = end_ms;
            }
        }

        final_lyric_lines.push(final_line);
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
    fn test_bilingual_lrc_parsing() {
        let content = "[00:20.00]Hello world\n[00:20.00]你好世界\n[00:22.00]Next line";
        let parsed_data = parse_lrc(content).unwrap();
        assert_eq!(parsed_data.lines.len(), 2, "相同时间戳的行应被合并");

        let line1 = &parsed_data.lines[0];
        assert_eq!(line1.start_ms, 20000);

        let annotated_track = &line1.tracks[0];
        assert_eq!(annotated_track.content_type, ContentType::Main);
        assert_eq!(
            annotated_track.content.words[0].syllables[0].text,
            "Hello world"
        );

        assert_eq!(annotated_track.translations.len(), 1, "应有1行翻译");
        assert_eq!(
            annotated_track.translations[0].words[0].syllables[0].text,
            "你好世界"
        );
    }
}
