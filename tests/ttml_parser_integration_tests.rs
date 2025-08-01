use lyrics_helper_rs::converter::{
    generators::ttml_generator::generate_ttml,
    parsers::ttml_parser::parse_ttml,
    processors::metadata_processor::MetadataStore,
    types::{TtmlGenerationOptions, TtmlParsingOptions, TtmlTimingMode},
};

use std::path::Path;

fn load_test_data(filename: &str) -> String {
    let path = Path::new("tests/test_data").join(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读取测试文件 '{:?}' 失败: {}", path, e))
}

#[test]
fn test_parse_line_timed_basic() {
    let content = load_test_data("line_timed_basic.ttml");
    let result = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    assert!(result.is_line_timed_source, "应该检测到逐行歌词");

    assert_eq!(result.lines.len(), 2, "应该解析两行歌词");

    let first_line = &result.lines[0];
    assert_eq!(first_line.start_ms, 10000);
    assert_eq!(first_line.end_ms, 15500);
    assert_eq!(first_line.line_text.as_deref(), Some("这是一行歌词."));
}

#[test]
fn test_parse_word_timed_basic() {
    let content = load_test_data("word_timed_basic.ttml");
    let result = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    assert!(!result.is_line_timed_source, "应该检测到逐字歌词");
    assert_eq!(result.lines.len(), 1);

    let line = &result.lines[0];
    assert_eq!(line.main_syllables.len(), 2, "应该有两个音节");

    let first_syl = &line.main_syllables[0];
    assert_eq!(first_syl.text, "Hello");
    assert_eq!(first_syl.start_ms, 5000);
    assert_eq!(first_syl.end_ms, 5500);
    assert!(first_syl.ends_with_space, "第一个音节后面应该有空格");

    let second_syl = &line.main_syllables[1];
    assert_eq!(second_syl.text, "world");
    assert_eq!(second_syl.start_ms, 5600);
    assert_eq!(second_syl.end_ms, 6200);
    assert!(!second_syl.ends_with_space);
}

#[test]
fn test_metadata_extraction() {
    let content = load_test_data("full_metadata.ttml");
    let result = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    let metadata = &result.raw_metadata;

    assert_eq!(metadata.get("title").unwrap()[0], "My Awesome Song");
    assert_eq!(metadata.get("artist").unwrap()[0], "The Rustaceans");

    assert_eq!(metadata.get("songwriters").unwrap().len(), 2);
    assert_eq!(metadata.get("songwriters").unwrap()[0], "作曲者1号");

    assert!(
        metadata
            .get("agent")
            .unwrap()
            .contains(&"v1=演唱者1号".to_string())
    );
    assert!(
        metadata
            .get("agent")
            .unwrap()
            .contains(&"v2=演唱者2号".to_string())
    );
}

#[test]
fn test_parse_word_timed_with_background() {
    let content = load_test_data("background_vocals.ttml");
    let result = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    assert!(!result.is_line_timed_source);

    let line_with_bg = result
        .lines
        .iter()
        .find(|l| l.background_section.is_some())
        .expect("应该找到一行有背景人声的歌词");

    let bg_section = line_with_bg.background_section.as_ref().unwrap();

    assert_eq!(bg_section.start_ms, 15000);
    assert_eq!(bg_section.end_ms, 17500);
    assert_eq!(bg_section.syllables.len(), 2);
    assert_eq!(bg_section.syllables[0].text, "ooh");
    assert_eq!(bg_section.syllables[1].text, "aah");
}

#[test]
fn test_warning_generation_for_recoverable_issues() {
    let content = load_test_data("malformed_but_recoverable.ttml");
    let result = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    assert!(!result.warnings.is_empty(), "应该产生警告");

    assert!(
        result.warnings.iter().any(|w| w.contains("<br/>")),
        "应该警告 br 标签"
    );

    assert!(
        result.warnings.iter().any(|w| w.contains("时间戳无效")),
        "应该警告时间戳无效"
    );
}

#[test]
fn test_round_trip() {
    let content = load_test_data("real_world.ttml");
    let parsed_data = parse_ttml(&content, &TtmlParsingOptions::default()).unwrap();

    let mut metadata_store = MetadataStore::new();
    for (raw_key, values) in &parsed_data.raw_metadata {
        for value in values {
            metadata_store.add(raw_key, value.clone()).unwrap();
        }
    }

    let options = TtmlGenerationOptions {
        timing_mode: if parsed_data.is_line_timed_source {
            TtmlTimingMode::Line
        } else {
            TtmlTimingMode::Word
        },
        format: true,
        ..Default::default()
    };

    let generated_ttml_output =
        generate_ttml(&parsed_data.lines, &metadata_store, &options).unwrap();

    insta::assert_snapshot!(generated_ttml_output);
}

#[test]
fn test_parse_formatted_ttml() {
    // 格式化TTML，<span>之间只有换行符。预期：无空格。
    let formatted_no_space_content = r#"
<tt xmlns="http://www.w3.org/ns/ttml" itunes:timing="word" xmlns:itunes="http://itunes.apple.com/lyric-ttml-extensions">
  <body>
    <p begin="0s" end="2s">
      <span begin="0s" end="1s">Hello</span>
      <span begin="1s" end="2s">World</span>
    </p>
  </body>
</tt>
"#;
    let result1 = parse_ttml(formatted_no_space_content, &TtmlParsingOptions::default()).unwrap();

    assert!(
        result1.detected_formatted_ttml_input.unwrap_or(false),
        "场景1: 应该检测到格式化输入"
    );
    let line1 = &result1.lines[0];
    assert_eq!(line1.main_syllables.len(), 2);
    assert!(
        !line1.main_syllables[0].ends_with_space,
        "场景1: 'Hello' 后面不应该有空格"
    );

    // 格式化TTML，<span>之间有一个明确的空格。预期：有空格。
    let formatted_with_space_content = r#"
<tt xmlns="http://www.w3.org/ns/ttml" itunes:timing="word" xmlns:itunes="http://itunes.apple.com/lyric-ttml-extensions">
  <body>
    <p begin="0s" end="2s">
      <span begin="0s" end="1s">Hello</span> <span begin="1s" end="2s">World</span>
    </p>
  </body>
</tt>
"#;
    let result2 = parse_ttml(formatted_with_space_content, &TtmlParsingOptions::default()).unwrap();

    assert!(
        result2.detected_formatted_ttml_input.unwrap_or(false),
        "场景2: 应该检测到格式化输入"
    );
    let line2 = &result2.lines[0];
    assert!(
        line2.main_syllables[0].ends_with_space,
        "场景2: 'Hello' 后面应该有空格"
    );

    // 未格式化的TTML，<span>之间有空格。预期：有空格。
    let unformatted_with_space_content = r#"<tt xmlns="http://www.w3.org/ns/ttml" itunes:timing="word" xmlns:itunes="http://itunes.apple.com/lyric-ttml-extensions"><body><p begin="0s" end="2s"><span begin="0s" end="1s">Hello</span> <span begin="1s" end="2s">World</span></p></body></tt>"#;
    let result3 = parse_ttml(
        unformatted_with_space_content,
        &TtmlParsingOptions::default(),
    )
    .unwrap();

    let line3 = &result3.lines[0];
    assert!(
        line3.main_syllables[0].ends_with_space,
        "场景3: 在未格式化输入中，'Hello' 后面应该有空格"
    );

    // 混合了紧邻和非紧邻<span>的格式化文件。预期：精确识别空格。
    let mixed_formatted_content = r#"
<tt xmlns="http://www.w3.org/ns/ttml" itunes:timing="word" xmlns:itunes="http://itunes.apple.com/lyric-ttml-extensions">
  <body>
    <p begin="31s" end="36s">
      <span begin="31s" end="32s">1</span
      ><span begin="32s" end="33s">2</span>
      <span begin="34s" end="35s">3</span>
    </p>
  </body>
</tt>
"#;
    let result4 = parse_ttml(mixed_formatted_content, &TtmlParsingOptions::default()).unwrap();

    assert!(
        result4.detected_formatted_ttml_input.unwrap_or(false),
        "场景4: 应该检测到格式化输入"
    );
    let line4 = &result4.lines[0];
    assert_eq!(line4.main_syllables.len(), 3);
    assert!(
        !line4.main_syllables[0].ends_with_space,
        "场景4: '1' 后面不应该有空格 (紧邻)"
    );
    assert!(
        !line4.main_syllables[1].ends_with_space,
        "场景4: '2' 后面不应该有空格 (换行分隔)"
    );
}

// 暂时不运行这个测试，因为格式错误也能继续解析
// #[test]
// fn test_fatal_error_on_invalid_xml() {
//     let content = load_test_data("invalid_xml.xml");
//     let result = parse_ttml(&content, &DefaultLanguageOptions::default());

//     assert!(result.is_err(), "解析XML应该报错");

//     assert!(
//         matches!(result.unwrap_err(), ConvertError::Xml(_)),
//         "错误类型应该为 ConvertError::Xml"
//     );
// }
