//! # KRC 格式解析器

use std::collections::HashMap;

use base64::{Engine, engine::general_purpose};
use regex::Regex;
use serde::Deserialize;
use std::sync::LazyLock;

use crate::converter::{
    types::{
        ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData, TranslationEntry,
    },
    utils::{parse_lrc_metadata_tag, process_syllable_text},
};

/// 匹配 KRC 行级时间戳 `[start,duration]`
static KRC_LINE_TIMESTAMP_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(?P<start>\d+),(?P<duration>\d+)\]")
        .expect("编译 KRC_LINE_TIMESTAMP_REGEX 失败")
});

/// 匹配 KRC 音节级时间戳和文本 `<offset,duration,pitch>text`
static KRC_SYLLABLE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(?P<offset>\d+),(?P<duration>\d+),(?P<pitch>0)>(?P<text>[^<]*)")
        .expect("编译 KRC_SYLLABLE_REGEX 失败")
});

// 匹配内嵌翻译的 language 标签
static KRC_TRANSLATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[language:(?P<base64>[A-Za-z0-9+/=]+)\]")
        .expect("编译 KRC_TRANSLATION_REGEX 失败")
});

/// 解析 KRC 格式内容到 `ParsedSourceData` 结构。
pub fn parse_krc(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut raw_metadata: HashMap<String, Vec<String>> = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();

    let translations = extract_translation_from_krc(content)?;

    for (i, line_str) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed_line = line_str.trim();

        if trimmed_line.is_empty() {
            continue;
        }

        if trimmed_line.starts_with("[language:") {
            continue;
        }

        if parse_lrc_metadata_tag(trimmed_line, &mut raw_metadata) {
            continue;
        }

        if let Some(line_caps) = KRC_LINE_TIMESTAMP_REGEX.captures(trimmed_line) {
            let line_start_ms: u64 = line_caps["start"].parse().map_err(ConvertError::ParseInt)?;
            let line_duration_ms: u64 = line_caps["duration"]
                .parse()
                .map_err(ConvertError::ParseInt)?;

            let content_after_line_ts = if let Some(m) = line_caps.get(0) {
                &trimmed_line[m.end()..]
            } else {
                warnings.push(format!("第 {line_num} 行: 正则表达式捕获意外失败。"));
                continue;
            };

            let mut syllables: Vec<LyricSyllable> = Vec::new();

            for syl_caps in KRC_SYLLABLE_REGEX.captures_iter(content_after_line_ts) {
                let raw_text = &syl_caps["text"];

                if let Some((clean_text, ends_with_space)) =
                    process_syllable_text(raw_text, &mut syllables)
                {
                    let offset_ms: u64 =
                        syl_caps["offset"].parse().map_err(ConvertError::ParseInt)?;
                    let duration_ms: u64 = syl_caps["duration"]
                        .parse()
                        .map_err(ConvertError::ParseInt)?;
                    let absolute_start_ms = line_start_ms + offset_ms;

                    syllables.push(LyricSyllable {
                        text: clean_text,
                        start_ms: absolute_start_ms,
                        end_ms: absolute_start_ms + duration_ms,
                        duration_ms: Some(duration_ms),
                        ends_with_space,
                    });
                }
            }

            if !syllables.is_empty() {
                let translation_text: Option<String> = translations
                    .as_ref()
                    .and_then(|t| t.get(lines.len()).cloned());

                let translation_entries: Vec<TranslationEntry> =
                    if let Some(text) = translation_text {
                        vec![TranslationEntry {
                            text,
                            lang: Some("zh-Hans".to_string()),
                        }]
                    } else {
                        Vec::new()
                    };

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
                    start_ms: line_start_ms,
                    end_ms: line_start_ms + line_duration_ms,
                    line_text: Some(full_line_text),
                    translations: translation_entries,
                    main_syllables: syllables,
                    agent: Some("v1".to_string()),
                    ..Default::default()
                });
            } else {
                warnings.push(format!("第 {line_num} 行: 未找到任何有效的音节。"));
            }
        } else {
            warnings.push(format!("第 {line_num} 行: 未能识别的行格式。"));
        }
    }

    Ok(ParsedSourceData {
        lines,
        raw_metadata,
        warnings,
        source_format: LyricFormat::Krc,
        is_line_timed_source: false,
        ..Default::default()
    })
}

#[derive(Deserialize)]
struct TranslationJson {
    content: Vec<ContentEntry>,
}

#[derive(Deserialize)]
struct ContentEntry {
    #[serde(rename = "lyricContent")]
    lyric_content: Vec<Vec<String>>,
}

/// 从 KRC 内容中提取并解析内嵌的翻译。
pub fn extract_translation_from_krc(content: &str) -> Result<Option<Vec<String>>, ConvertError> {
    if let Some(caps) = KRC_TRANSLATION_REGEX.captures(content) {
        let base64_str = &caps["base64"];
        let decoded_bytes = general_purpose::STANDARD
            .decode(base64_str)
            .map_err(ConvertError::Base64Decode)?;
        let decoded_text = String::from_utf8(decoded_bytes).map_err(ConvertError::FromUtf8)?;

        // 将解码后的文本作为 JSON 解析
        let parsed_json: TranslationJson = serde_json::from_str(&decoded_text)
            .map_err(|e| ConvertError::json_parse(e, "KRC 内嵌翻译 JSON".to_string()))?;

        // 从 JSON 结构中提取翻译行
        if let Some(first_content) = parsed_json.content.first() {
            let translation_lines: Vec<String> = first_content
                .lyric_content
                .iter()
                // 每行翻译本身也是一个数组，将数组内的字符串连接起来
                .map(|line_parts| line_parts.join(""))
                .collect();

            return Ok(Some(translation_lines));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KRC_CONTENT: &str = r#"
[language:eyJjb250ZW50IjpbeyJsYW5ndWFnZSI6MCwibHlyaWNDb250ZW50IjpbWyIgIl0sWyIgIl0sWyIgIl0sWyIgIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTY2MkZcdTU1NEEgXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTc1MUZcdTZEM0JcdTVDMzFcdTUwQ0ZcdTRFMDBcdTY4MkFcdThEQzNcdTUyQThcdTc2ODRcdTg1RTRcdTg1MTMiXSxbIlx1OTU3Rlx1OUE3MVx1NzZGNFx1NTE2NSBcdTZGQzBcdTZEM0JcdTYyMTFcdTc2ODRcdTUxODVcdTVGQzMiXSxbIlx1NjIxMVx1ODBGRFx1NjExRlx1NTNEN1x1NTIzMFx1NTkyQVx1OTYzM1x1NzY4NFx1ODAwMFx1NzczQyJdLFsiXHU5MDREXHU1QkZCXHU0RTRCXHU1NDBFXHU0RjYwXHU1QzMxXHU0RjFBXHU1M0QxXHU3M0IwIl0sWyJcdTYyMTFcdTg2N0RcdTRFMEFcdTRFODZcdTVFNzRcdTdFQUEgXHU0RjQ2XHU0RTVGXHU0RTBEXHU2NjJGXHU4MDAxXHU2MDAxXHU5Rjk5XHU5NDlGIl0sWyJcdTg2N0RcdThGRDhcdTVFNzRcdThGN0IgXHU1Mzc0XHU2NzJBXHU1RkM1XHU5QzgxXHU4M0JEXHU1OTMxXHU3OTNDIl0sWyJcdTU3NUFcdTRGRTFcdThGRDlcdTRFMkFcdTRFMTZcdTc1NENcdTdGOEVcdTU5N0RcdTU5ODJcdTUyMUQiXSxbIlx1NjIxMVx1NTNFQVx1NjYyRlx1NUZBQVx1ODlDNFx1OEU0OFx1NzdFOVx1NTczMFx1NzUxRlx1NkQzQlx1Nzc0MCJdLFsiXHU2MEVGXHU0RThFXHU3OUJCXHU3RUNGXHU1M0RCXHU5MDUzXHU0RTJEIl0sWyJcdTRGNTNcdTU0NzNcdTVGQzNcdTVCODlcdTc0MDZcdTVGOTciXSxbIlx1NEVBNlx1NEU4RVx1NjMwOVx1OTBFOFx1NUMzMVx1NzNFRFx1NEUyRCJdLFsiXHU3NURCXHU2MTFGXHU0RTRGXHU1NTg0XHU1M0VGXHU5NjQ4Il0sWyJcdTYyMTFcdTRFMERcdTUzRUZcdTRFRTVcdTgxRUFcdTZCM0FcdTZCM0FcdTRFQkEgXHU0RTBEXHU1M0VGXHU0RUU1XHU4MUVBXHU2QjNBXHU2QjNBXHU0RUJBIl0sWyJcdTRGNDZcdTdGNkVcdTYyMTFcdTRFOEVcdTZCN0JcdTU3MzBcdTgwMDUgXHU1RkM1XHU1QzA2XHU4RDUwXHU2MjExXHU0RUU1XHU1NDBFXHU3NTFGIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTYyMTFcdTYxMUZcdTg5QzlcdTUyMzBcdTcyMzFcdTRFODYgXHU2MjExXHU2MTFGXHU4OUM5XHU1MjMwXHU1QjgzXHU2QjYzXHU1NzI4XHU3MUMzXHU3MEU3Il0sWyJcdTRFOEVcdTZDQjNcdTZENDFcdTc2ODRcdTZCQ0ZcdTRFMkFcdThGQzJcdTU2REVcdTU5MDRcdTdGRkJcdTgxN0UiXSxbIlx1NUUwQ1x1NjcxQlx1NTNFQVx1NjYyRlx1NEUyQVx1NTZEQlx1NUI1N1x1NTM1NVx1OEJDRCJdLFsiXHU4RUFCXHU1OTE2XHU0RTRCXHU3MjY5IFx1NzY4Nlx1NTNFRlx1NjI5Qlx1NTM3NCJdLFsiXHU2MjExXHU4NjdEXHU0RTBBXHU0RTg2XHU1RTc0XHU3RUFBIFx1NEY0Nlx1NEU1Rlx1NEUwRFx1NjYyRlx1ODAwMVx1NjAwMVx1OUY5OVx1OTQ5RiJdLFsiXHU4NjdEXHU4RkQ4XHU1RTc0XHU4RjdCIFx1NTM3NFx1NjcyQVx1NUZDNVx1OUM4MVx1ODNCRFx1NTkzMVx1NzkzQyJdLFsiXHU1NzVBXHU0RkUxXHU4RkQ5XHU0RTJBXHU0RTE2XHU3NTRDXHU3RjhFXHU1OTdEXHU1OTgyXHU1MjFEIl0sWyJcdTYyMTFcdTUzRUFcdTY2MkZcdTVGQUFcdTg5QzRcdThFNDhcdTc3RTlcdTU3MzBcdTc1MUZcdTZEM0JcdTc3NDAiXSxbIlx1NEVBNlx1NEU4RVx1NjMwOVx1OTBFOFx1NUMzMVx1NzNFRFx1NEUyRCJdLFsiXHU3NURCXHU2MTFGXHU0RTRGXHU1NTg0XHU1M0VGXHU5NjQ4Il0sWyJcdTYyMTFcdTRFMERcdTUzRUZcdTRFRTVcdTgxRUFcdTZCM0FcdTZCM0FcdTRFQkEgXHU0RTBEXHU1M0VGXHU0RUU1XHU4MUVBXHU2QjNBXHU2QjNBXHU0RUJBIl0sWyJcdTRFMDBcdTUyMDdcdTZERjlcdTZDQTFcdTYyMTFcdTc2ODRcdTRFMUNcdTg5N0YgXHU2MDNCXHU2NjJGXHU2MEYzXHU4QkE5XHU2MjExXHU5OERFXHU3RkQ0Il0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdThFQUJcdTU5MTZcdTRFNEJcdTcyNjkiXSxbIlx1NzY4Nlx1NTNFRlx1NjI5Qlx1NTM3NCJdLFsiXHU2Q0RCXHU4MjFGXHU1RjUzXHU2QjRDIl0sWyJcdTRFQkFcdTc1MUZcdTUxRTBcdTRGNTUiXSxbIlx1OEVBQlx1NTkxNlx1NEU0Qlx1NzI2OSJdLFsiXHU3Njg2XHU1M0VGXHU2MjlCXHU1Mzc0Il0sWyJcdTZDREJcdTgyMUZcdTVGNTNcdTZCNEMiXSxbIlx1NEVCQVx1NzUxRlx1NTFFMFx1NEY1NSJdLFsiXHU4RUFCXHU1OTE2XHU0RTRCXHU3MjY5Il0sWyJcdTc2ODZcdTUzRUZcdTYyOUJcdTUzNzQiXSxbIlx1NkNEQlx1ODIxRlx1NUY1M1x1NkI0QyJdLFsiXHU0RUJBXHU3NTFGXHU1MUUwXHU0RjU1Il0sWyJcdThFQUJcdTU5MTZcdTRFNEJcdTcyNjkiXSxbIlx1NzY4Nlx1NTNFRlx1NjI5Qlx1NTM3NCJdLFsiXHU2Q0RCXHU4MjFGXHU1RjUzXHU2QjRDIl0sWyJcdTRFQkFcdTc1MUZcdTUxRTBcdTRGNTUiXSxbIlx1NEY0Nlx1N0Y2RVx1NjIxMVx1NEU4RVx1NkI3Qlx1NTczMFx1ODAwNSJdLFsiXHU1RkM1XHU1QzA2XHU4RDUwXHU2MjExXHU0RUU1XHU1NDBFXHU3NTFGIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdTY3MDBcdThGRDFcdTYyMTFcdTYwM0JcdTY2MkZcdThGOTdcdThGNkNcdTUzQ0RcdTRGQTcgXHU5NkJFXHU0RUU1XHU1MTY1XHU3NzIwIl0sWyJcdTVCRjlcdTYyMTFcdTRFRUNcdTY2RkVcdTY3MDlcdThGQzdcdTc2ODRcdTYxM0ZcdTY2NkZcdTZENkVcdTYwRjNcdTgwNTRcdTdGRTkiXSxbIlx1NEY0Nlx1NEVCMlx1NzIzMVx1NzY4NCBcdTYyMTFcdTY1RTlcdTVERjJcdTU3MjhcdTUxODVcdTVGQzNcdTZERjFcdTU5MDRcdTc5NDhcdTc5NzdcdTc3NDAiXSxbIlx1Nzk0OFx1Nzk3N1x1ODFFQVx1NURGMVx1NEUwRFx1NTE4RFx1OEZGN1x1NTkzMVx1NEU4RVx1OTFEMVx1OTRCMVx1NzY4NFx1OEZGRFx1OTAxMFx1NEUyRCJdLFsiXHU2MjExXHU0RUVDXHU1M0VGXHU0RUU1XHU3RUM2XHU2NTcwXHU2RUUxXHU1OTI5XHU3RTQxXHU2NjFGIl0sWyJcdThFQUJcdTU5MTZcdTRFNEJcdTcyNjkiXSxbIlx1NzY4Nlx1NTNFRlx1NjI5Qlx1NTM3NCJdLFsiXHU2Q0RCXHU4MjFGXHU1RjUzXHU2QjRDIl0sWyJcdTRFQkFcdTc1MUZcdTUxRTBcdTRGNTUiXSxbIlx1OEVBQlx1NTkxNlx1NEU0Qlx1NzI2OSJdLFsiXHU3Njg2XHU1M0VGXHU2MjlCXHU1Mzc0Il0sWyJcdTZDREJcdTgyMUZcdTVGNTNcdTZCNEMiXSxbIlx1NEVCQVx1NzUxRlx1NTFFMFx1NEY1NSJdLFsiXHU4RUFCXHU1OTE2XHU0RTRCXHU3MjY5Il0sWyJcdTc2ODZcdTUzRUZcdTYyOUJcdTUzNzQiXSxbIlx1NkNEQlx1ODIxRlx1NUY1M1x1NkI0QyJdLFsiXHU0RUJBXHU3NTFGXHU1MUUwXHU0RjU1Il0sWyJcdThFQUJcdTU5MTZcdTRFNEJcdTcyNjkiXSxbIlx1NzY4Nlx1NTNFRlx1NjI5Qlx1NTM3NCJdLFsiXHU2Q0RCXHU4MjFGXHU1RjUzXHU2QjRDIl0sWyJcdTRFQkFcdTc1MUZcdTUxRTBcdTRGNTUiXV0sInR5cGUiOjF9XSwidmVyc2lvbiI6MX0=]
[47,71]<0,0,0>Counting <0,37,0>Stars <37,0,0>- <37,34,0>OneRepublic
[190,200]<0,28,0>Lyrics<28,28,0> <56,28,0>by<84,28,0>：<112,28,0>Ryan<140,28,0> <168,28,0>Tedder
[390,200]<0,28,0>Composed<28,28,0> <56,28,0>by<84,28,0>：<112,28,0>Ryan<140,28,0> <168,28,0>Tedder
[590,190]<0,17,0>Produced<17,17,0> <34,17,0>by<51,17,0>：<68,17,0>Ryan<85,17,0> <102,17,0>Tedder<119,17,0>/<136,17,0>Noel<153,17,0> <170,17,0>Zancanella
[790,3661]<0,1072,0>Lately <1072,533,0>I've <1605,471,0>been <2076,343,0>I've <2419,327,0>been <2746,348,0>losing <3094,567,0>sleep
[5499,3492]<0,642,0>Dreaming <642,463,0>about <1105,279,0>the <1384,383,0>things <1767,599,0>that we <2366,633,0>could <2999,493,0>be
[9390,4266]<0,200,0>But <200,1079,0>baby <1279,417,0>I've <1696,687,0>been <2383,215,0>I've <2598,326,0>been <2924,344,0>praying <3268,998,0>hard
[14361,1996]<0,190,0>Said <190,362,0>no <552,263,0>more <815,446,0>counting <1261,735,0>dollars
[16362,2920]<0,144,0>We'll <144,285,0>be <429,590,0>counting <1019,1901,0>stars
[19286,3606]<0,530,0>Yeah <530,191,0>we'll <721,182,0>be <903,1330,0>counting <2233,1373,0>stars
"#;

    #[test]
    fn debug_krc_parsing_flow() {
        let parsed_data = parse_krc(KRC_CONTENT).unwrap();

        for (i, line) in parsed_data.lines.iter().take(10).enumerate() {
            let line_preview = line
                .line_text
                .as_deref()
                .unwrap_or("")
                .chars()
                .collect::<String>();
            if !line.translations.is_empty() {
                println!(
                    "第 {} 行 ('{}'): 翻译 -> '{}'",
                    i + 1,
                    line_preview,
                    line.translations[0].text
                );
            } else {
                println!("第 {} 行 ('{}'): 无翻译", i + 1, line_preview);
            }
        }

        assert!(!parsed_data.lines.is_empty(), "至少应该解析出一行歌词");
    }
}
