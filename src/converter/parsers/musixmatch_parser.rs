//! # Musixmatch RichSync JSON 格式解析器
//!
//! 此模块负责解析 Musixmatch API 返回的 RichSync 逐字歌词格式。

use crate::converter::types::{
    ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
};

// 为了让解析器能够理解 RichSync 的 JSON 结构，
// 需要在这里引用 Musixmatch API 的模型定义。
use crate::providers::musixmatch::models as musixmatch_models;

/// 解析 Musixmatch RichSync JSON 格式内容到 `ParsedSourceData` 结构。
pub fn parse(json_content: &str) -> Result<ParsedSourceData, ConvertError> {
    // 将 JSON 字符串反序列化为 Musixmatch 的模型结构
    let richsync_lines: Vec<musixmatch_models::RichSyncLine> = serde_json::from_str(json_content)
        .map_err(|e| {
        tracing::error!("反序列化 Musixmatch RichSync JSON 失败: {}", e);
        ConvertError::json_parse(e, "Musixmatch RichSync".to_string())
    })?;

    // 将 Musixmatch 模型转换为通用的 `LyricLine` 结构
    let lines = richsync_lines
        .into_iter()
        .map(|rich_line| {
            let line_start_ms = (rich_line.line_start_ms * 1000.0) as u64;

            let mut syllables_accumulator: Vec<LyricSyllable> = Vec::new();

            for rich_syllable in rich_line.syllables {
                let original_text = &rich_syllable.text;
                let trimmed_text = original_text.trim();

                if trimmed_text.is_empty() {
                    if let Some(last_syllable) = syllables_accumulator.last_mut() {
                        last_syllable.ends_with_space = true;
                    }
                    continue;
                }

                if original_text.starts_with(char::is_whitespace) {
                    if let Some(last_syllable) = syllables_accumulator.last_mut() {
                        last_syllable.ends_with_space = true;
                    }
                }

                let new_syllable = LyricSyllable {
                    text: trimmed_text.to_string(),
                    start_ms: line_start_ms + (rich_syllable.offset * 1000.0) as u64,
                    end_ms: 0,
                    duration_ms: None,
                    ends_with_space: original_text.ends_with(char::is_whitespace),
                };

                syllables_accumulator.push(new_syllable);
            }

            LyricLine {
                start_ms: line_start_ms,
                end_ms: (rich_line.line_end_ms * 1000.0) as u64,
                line_text: Some(rich_line.line_text),
                main_syllables: syllables_accumulator,
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();

    // 后处理：计算每个音节的精确结束时间和持续时间
    let processed_lines = post_process_syllable_timing(lines);

    Ok(ParsedSourceData {
        lines: processed_lines,
        source_format: LyricFormat::Musixmatch,
        is_line_timed_source: false,
        ..Default::default()
    })
}

/// 后处理歌词行，计算每个音节的 `end_ms` 和 `duration_ms`。
///
/// Musixmatch 只提供了每个音节的开始时间，需要通过下一个音节的
/// 开始时间来推断当前音节的结束时间。
fn post_process_syllable_timing(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let mut processed_lines = Vec::with_capacity(lines.len());

    for mut line in lines {
        if line.main_syllables.is_empty() {
            processed_lines.push(line);
            continue;
        }

        let syllables_count = line.main_syllables.len();
        for i in 0..syllables_count {
            // 下一个音节的开始时间，就是当前音节的结束时间
            let next_syllable_start = if i + 1 < syllables_count {
                line.main_syllables[i + 1].start_ms
            } else {
                // 如果是最后一个音节，它的结束时间就是整行的结束时间
                line.end_ms
            };

            let current_syllable = &mut line.main_syllables[i];
            current_syllable.end_ms = next_syllable_start;
            current_syllable.duration_ms = Some(
                current_syllable
                    .end_ms
                    .saturating_sub(current_syllable.start_ms),
            );
        }

        // 修正最后一个音节后的空格标记
        if let Some(last_syllable) = line.main_syllables.last_mut() {
            last_syllable.ends_with_space = false;
        }

        processed_lines.push(line);
    }

    processed_lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::converter::types::ConvertError;

    #[test]
    fn test_basic_parsing_and_timing() {
        let json_content = r#"[
            {
                "ts": 10.5,
                "te": 12.8,
                "l": [
                    {"c": "Hello", "o": 0.1},
                    {"c": " ", "o": 0.6},
                    {"c": "world", "o": 0.7}
                ],
                "x": "Hello world"
            }
        ]"#;

        let result = parse(json_content);
        assert!(result.is_ok());
        let parsed_data = result.unwrap();

        assert_eq!(parsed_data.lines.len(), 1, "应该只解析出一行歌词");
        let line = &parsed_data.lines[0];
        assert_eq!(line.start_ms, 10500);
        assert_eq!(line.end_ms, 12800);
        assert_eq!(line.line_text.as_deref(), Some("Hello world"));

        assert_eq!(
            line.main_syllables.len(),
            2,
            "纯空格音节不应被创建，应只剩下2个音节"
        );

        let syllable1 = &line.main_syllables[0];
        assert_eq!(syllable1.text, "Hello");
        assert_eq!(syllable1.start_ms, 10600); // 10500 + 100
        assert_eq!(
            syllable1.ends_with_space, true,
            "后面跟了一个空格音节，应为true"
        );

        let syllable2 = &line.main_syllables[1];
        assert_eq!(syllable2.text, "world");
        assert_eq!(syllable2.start_ms, 11200); // 10500 + 700
        assert_eq!(
            syllable2.ends_with_space, false,
            "这是最后一个音节，应为false"
        );

        let processed_lines = post_process_syllable_timing(parsed_data.lines);
        let processed_line = &processed_lines[0];
        let processed_syl1 = &processed_line.main_syllables[0];
        let processed_syl2 = &processed_line.main_syllables[1];

        assert_eq!(
            processed_syl1.end_ms, processed_syl2.start_ms,
            "第一个音节的结束应是第二个的开始"
        );
        assert_eq!(
            processed_syl2.end_ms, processed_line.end_ms,
            "最后一个音节的结束应是整行的结束"
        );
        assert_eq!(processed_syl1.duration_ms, Some(600)); // 11200 - 10600
        assert_eq!(processed_syl2.duration_ms, Some(1600)); // 12800 - 11200
    }

    #[test]
    fn test_space_handling() {
        let json_content = r#"[
            {
                "ts": 0.0, "te": 10.0,
                "l": [
                    {"c": "word1 ", "o": 1.0},
                    {"c": "word2", "o": 2.0},
                    {"c": " ", "o": 3.0},
                    {"c": "word3", "o": 4.0},
                    {"c": " word4", "o": 5.0},
                    {"c": " word5 ", "o": 6.0},
                    {"c": " ", "o": 7.0},
                    {"c": "word6", "o": 8.0}
                ],
                "x": "..."
            }
        ]"#;

        let result = parse(json_content).unwrap();
        let line = &result.lines[0];

        assert_eq!(line.main_syllables.len(), 6, "应该生成6个有效音节");

        let texts: Vec<&str> = line
            .main_syllables
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["word1", "word2", "word3", "word4", "word5", "word6"]
        );

        let spaces: Vec<bool> = line
            .main_syllables
            .iter()
            .map(|s| s.ends_with_space)
            .collect();

        assert_eq!(spaces, vec![true, true, true, true, true, false]);
    }

    #[test]
    fn test_leading_space_at_start_of_line() {
        let json_content = r#"[
            {
                "ts": 0.0, "te": 2.0,
                "l": [
                    {"c": " ", "o": 0.1},
                    {"c": "word", "o": 0.5}
                ],
                "x": " word"
            }
        ]"#;
        let result = parse(json_content).unwrap();
        let line = &result.lines[0];

        assert_eq!(line.main_syllables.len(), 1);
        assert_eq!(line.main_syllables[0].text, "word");
        assert_eq!(line.main_syllables[0].ends_with_space, false);
    }

    #[test]
    fn test_empty_and_invalid_input() {
        let empty_json = "[]";
        let result = parse(empty_json).unwrap();
        assert!(result.lines.is_empty(), "解析空数组应返回空行列表");

        let invalid_json = r#"[{"ts":10.0, "l":}]"#;
        let result = parse(invalid_json);
        assert!(result.is_err());

        assert!(matches!(
            result.err().unwrap(),
            ConvertError::JsonParse { .. }
        ));
    }
}
