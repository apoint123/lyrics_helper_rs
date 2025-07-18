//! ASS 格式生成器

use std::fmt::Write;

use crate::converter::{
    processors::metadata_processor::MetadataStore,
    types::{AssGenerationOptions, ConvertError, LyricLine, RomanizationEntry, TranslationEntry},
};

/// 将毫秒时间格式化为 ASS 时间字符串 `H:MM:SS.CS` (小时:分钟:秒.厘秒)。
fn format_ass_time(ms: u64) -> String {
    let total_cs = (ms + 5) / 10; // 四舍五入到厘秒
    let cs = total_cs % 100;
    let total_seconds = total_cs / 100;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours}:{minutes:02}:{seconds:02}.{cs:02}")
}

/// 将毫秒时长四舍五入到厘秒 (cs)，用于 ASS 的 `\k` 标签。
fn round_duration_to_cs(duration_ms: u64) -> u32 {
    ((duration_ms + 5) / 10) as u32
}

/// ASS 生成的主入口函数。
pub fn generate_ass(
    lines: &[LyricLine],
    metadata_store: &MetadataStore,
    is_line_timed: bool,
    options: &AssGenerationOptions,
) -> Result<String, ConvertError> {
    let mut ass_content = String::with_capacity(lines.len() * 200 + 1024);

    // --- [Script Info] 部分 ---
    if let Some(custom_script_info) = &options.script_info {
        writeln!(ass_content, "{}", custom_script_info.trim())?;
    } else {
        writeln!(ass_content, "[Script Info]")?;
        writeln!(ass_content, "ScriptType: v4.00+")?;
        writeln!(ass_content, "PlayResX: 1920")?;
        writeln!(ass_content, "PlayResY: 1080")?;
    }
    writeln!(ass_content)?;

    // --- [V4+ Styles] 部分 ---
    if let Some(custom_styles) = &options.styles {
        writeln!(ass_content, "{}", custom_styles.trim())?;
    } else {
        writeln!(ass_content, "[V4+ Styles]")?;
        writeln!(
            ass_content,
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding"
        )?;
        writeln!(
            ass_content,
            "Style: Default,Arial,90,&H00FFFFFF,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,3.5,1,2,10,10,10,1"
        )?; // 主歌词
        writeln!(
            ass_content,
            "Style: ts,Arial,55,&H00D3D3D3,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,50,1"
        )?; // 翻译
        writeln!(
            ass_content,
            "Style: roma,Arial,55,&H00D3D3D3,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,2,1,2,10,10,50,1"
        )?; // 罗马音
        writeln!(
            ass_content,
            "Style: bg-main,Arial,75,&H00E5E5E5,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,3,1,8,10,10,10,1"
        )?; // 背景主歌词
        writeln!(
            ass_content,
            "Style: bg-ts,Arial,45,&H00A0A0A0,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,1.5,1,8,10,10,55,1"
        )?; // 背景翻译
        writeln!(
            ass_content,
            "Style: bg-roma,Arial,45,&H00A0A0A0,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,1.5,1,8,10,10,55,1"
        )?; // 背景罗马音
        writeln!(
            ass_content,
            "Style: meta,Arial,40,&H00C0C0C0,&H000000FF,&H00000000,&H99000000,0,0,0,0,100,100,0,0,1,1,0,5,10,10,10,1"
        )?; // 元数据
    }
    writeln!(ass_content)?;

    // --- [Events] 部分 ---
    writeln!(ass_content, "[Events]")?;
    writeln!(
        ass_content,
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
    )?;

    // 写入元数据注释行
    for (key, values) in metadata_store.get_all_data() {
        for value in values {
            writeln!(
                ass_content,
                "Comment: 0,0:00:00.00,0:00:00.00,meta,,0,0,0,,{key}: {value}"
            )?;
        }
    }

    // 遍历所有 LyricLine 生成 Dialogue 行
    for line in lines {
        // 主歌词行
        if !line.main_syllables.is_empty() || line.line_text.is_some() {
            let mut actor_field = line.agent.clone().unwrap_or_default();
            if let Some(part) = &line.song_part {
                write!(actor_field, r#" itunes:song-part="{part}""#)?;
            }

            let text_field = if is_line_timed {
                line.line_text.clone().unwrap_or_default()
            } else {
                build_karaoke_text(line, false)?
            };

            if !text_field.trim().is_empty() {
                writeln!(
                    ass_content,
                    "Dialogue: 0,{},{},Default,{},0,0,0,,{}",
                    format_ass_time(line.start_ms),
                    format_ass_time(line.end_ms),
                    actor_field.trim(),
                    text_field
                )?;
            }
        }

        // 翻译和罗马音行
        write_auxiliary_lines(
            &mut ass_content,
            line.start_ms,
            line.end_ms,
            &line.translations,
            &line.romanizations,
            false,
        )?;

        // 背景人声行
        if let Some(bg_section) = &line.background_section {
            if !bg_section.syllables.is_empty() {
                let bg_text_field = build_karaoke_text(line, true)?;
                if !bg_text_field.trim().is_empty() {
                    writeln!(
                        ass_content,
                        "Dialogue: 0,{},{},bg-main,x-bg,0,0,0,,{}",
                        format_ass_time(bg_section.start_ms),
                        format_ass_time(bg_section.end_ms),
                        bg_text_field
                    )?;
                }
            }
            write_auxiliary_lines(
                &mut ass_content,
                bg_section.start_ms,
                bg_section.end_ms,
                &bg_section.translations,
                &bg_section.romanizations,
                true,
            )?;
        }
    }

    Ok(ass_content)
}

/// 辅助函数，构建带 `\k` 标签的文本
fn build_karaoke_text(line: &LyricLine, is_background: bool) -> Result<String, ConvertError> {
    let syllables = if is_background {
        if let Some(bg_section) = &line.background_section {
            &bg_section.syllables
        } else {
            return Ok(String::new());
        }
    } else {
        &line.main_syllables
    };

    if syllables.is_empty() {
        return Ok(String::new());
    }

    let mut text_builder = String::new();
    let mut previous_syllable_end_ms = if is_background {
        line.background_section
            .as_ref()
            .map_or(line.start_ms, |bg| bg.start_ms)
    } else {
        line.start_ms
    };

    for syl in syllables {
        // 计算音节间的间隙
        if syl.start_ms > previous_syllable_end_ms {
            let gap_cs = round_duration_to_cs(syl.start_ms - previous_syllable_end_ms);
            if gap_cs > 0 {
                write!(text_builder, "{{\\k{gap_cs}}}")?;
            }
        }

        // 计算音节本身的时长
        let syl_duration_ms = syl.end_ms.saturating_sub(syl.start_ms);
        let mut syl_duration_cs = round_duration_to_cs(syl_duration_ms);
        // 对于非常短的音节，确保其至少有1cs
        if syl_duration_cs == 0 && syl_duration_ms > 0 {
            syl_duration_cs = 1;
        }

        if syl_duration_cs > 0 {
            write!(text_builder, "{{\\k{syl_duration_cs}}}")?;
        }

        text_builder.push_str(&syl.text);

        if syl.ends_with_space {
            text_builder.push(' ');
        }
        previous_syllable_end_ms = syl.end_ms;
    }

    Ok(text_builder.trim_end().to_string())
}

/// 辅助函数，写入翻译和罗马音的 Dialogue 行。
fn write_auxiliary_lines(
    ass_content: &mut String,
    start_ms: u64,
    end_ms: u64,
    translations: &[TranslationEntry],
    romanizations: &[RomanizationEntry],
    is_background: bool,
) -> Result<(), ConvertError> {
    let trans_style = if is_background { "bg-ts" } else { "ts" };
    let roma_style = if is_background { "bg-roma" } else { "roma" };

    for entry in translations {
        if !entry.text.trim().is_empty() {
            let actor = entry
                .lang
                .as_ref()
                .map_or(String::new(), |l| format!("x-lang:{l}"));
            writeln!(
                ass_content,
                "Dialogue: 0,{},{},{},{},0,0,0,,{}",
                format_ass_time(start_ms),
                format_ass_time(end_ms),
                trans_style,
                actor,
                entry.text
            )?;
        }
    }

    for entry in romanizations {
        if !entry.text.trim().is_empty() {
            let actor = entry
                .lang
                .as_ref()
                .map_or(String::new(), |l| format!("x-lang:{l}"));
            writeln!(
                ass_content,
                "Dialogue: 0,{},{},{},{},0,0,0,,{}",
                format_ass_time(start_ms),
                format_ass_time(end_ms),
                roma_style,
                actor,
                entry.text
            )?;
        }
    }

    Ok(())
}
