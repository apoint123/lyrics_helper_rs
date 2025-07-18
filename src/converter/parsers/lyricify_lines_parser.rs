//! # Lyricify Lines 格式解析器

use std::collections::HashMap;

use regex::Regex;
use std::sync::LazyLock;

use crate::converter::types::{
    ConvertError, LyricFormat, LyricLine, LyricSyllable, ParsedSourceData,
};

/// 匹配 Lyricify Lines 的行格式 `[start,end]Text`
static LYL_LINE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d+),(\d+)\](.*)$").expect("编译 LYL_LINE_REGEX 失败"));

/// 解析 LYL 格式内容到 `ParsedSourceData` 结构。
pub fn parse_lyl(content: &str) -> Result<ParsedSourceData, ConvertError> {
    let mut lines: Vec<LyricLine> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 逐行遍历文件内容
    for (i, line_str) in content.lines().enumerate() {
        let line_num = i + 1;
        let trimmed_line = line_str.trim();

        // 跳过空行和格式声明头
        if trimmed_line.is_empty() || trimmed_line.eq_ignore_ascii_case("[type:LyricifyLines]") {
            continue;
        }

        // 使用正则表达式匹配歌词行
        if let Some(caps) = LYL_LINE_REGEX.captures(trimmed_line) {
            // 从捕获组中提取时间戳和文本
            let start_ms_str = &caps[1];
            let end_ms_str = &caps[2];
            let text = caps[3].trim().to_string();

            let start_ms: u64 = start_ms_str.parse().map_err(ConvertError::ParseInt)?;
            let end_ms: u64 = end_ms_str.parse().map_err(ConvertError::ParseInt)?;

            // 检查结束时间是否在开始时间之前
            if end_ms < start_ms {
                warnings.push(format!(
                    "第 {line_num} 行: 结束时间 {end_ms}ms 在开始时间 {start_ms}ms 之前。该行仍将被处理。"
                ));
            }

            // LYL 是纯逐行格式，所以直接填充 `line_text` 字段。
            lines.push(LyricLine {
                start_ms,
                end_ms,
                line_text: Some(text.clone()),
                main_syllables: vec![LyricSyllable {
                    text,
                    start_ms,
                    end_ms,
                    duration_ms: Some(end_ms.saturating_sub(start_ms)),
                    ..Default::default()
                }],
                ..Default::default()
            });
        } else {
            warnings.push(format!("第 {line_num} 行: 未能识别的行格式。"));
        }
    }

    Ok(ParsedSourceData {
        lines,
        raw_metadata: HashMap::new(),
        warnings,
        source_format: LyricFormat::Lyl,
        is_line_timed_source: true,
        ..Default::default()
    })
}
