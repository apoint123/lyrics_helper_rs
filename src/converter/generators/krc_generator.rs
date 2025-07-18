//! KRC 格式生成器

use std::fmt::Write;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{ConvertError, LyricLine},
};

/// KRC 生成的主入口函数。
pub fn generate_krc(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
) -> Result<String, ConvertError> {
    let mut krc_output = String::new();

    writeln!(krc_output, "{}", metadata_store.generate_lrc_header())?;

    for line in lines {
        // KRC 不支持背景人声
        if line.main_syllables.is_empty() {
            continue;
        }

        let line_duration = line.end_ms.saturating_sub(line.start_ms);
        write!(krc_output, "[{},{}]", line.start_ms, line_duration)?;

        for syl in &line.main_syllables {
            let offset_ms = syl.start_ms.saturating_sub(line.start_ms);
            let duration_ms = syl.end_ms.saturating_sub(syl.start_ms);

            write!(krc_output, "<{},{},0>{}", offset_ms, duration_ms, syl.text)?;
        }

        writeln!(krc_output)?;
    }

    Ok(krc_output)
}
