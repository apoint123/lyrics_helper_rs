use lyrics_helper_rs::converter::{
    generators::ttml_generator::generate_ttml,
    parsers::ttml_parser::parse_ttml,
    processors::metadata_processor::MetadataStore,
    types::{
        CanonicalMetadataKey, DefaultLanguageOptions, LyricLine, LyricSyllable,
        TtmlGenerationOptions, TtmlTimingMode,
    },
};

#[test]
fn test_generate_line_timed_snapshot() {
    let lines = vec![
        LyricLine {
            start_ms: 1000,
            end_ms: 5000,
            line_text: Some("这是一行歌词".to_string()),
            ..Default::default()
        },
        LyricLine {
            start_ms: 6000,
            end_ms: 10000,
            line_text: Some("这是第二行歌词".to_string()),
            ..Default::default()
        },
    ];
    let mut metadata = MetadataStore::new();
    metadata.set_single(
        &CanonicalMetadataKey::Title.to_string(),
        "逐行歌曲".to_string(),
    );
    metadata.set_single(
        &CanonicalMetadataKey::Artist.to_string(),
        "测试艺术家".to_string(),
    );

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Line,
        format: true,
        ..Default::default()
    };

    let ttml_output = generate_ttml(&lines, &metadata, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_generate_word_timed_with_agents_snapshot() {
    let lines = vec![
        LyricLine {
            agent: Some("演唱者1号".to_string()),
            start_ms: 5000,
            end_ms: 8200,
            main_syllables: vec![
                LyricSyllable {
                    text: "I".to_string(),
                    start_ms: 5000,
                    end_ms: 5500,
                    ends_with_space: true,
                    ..Default::default()
                },
                LyricSyllable {
                    text: "sing".to_string(),
                    start_ms: 5600,
                    end_ms: 6200,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        LyricLine {
            agent: Some("合唱".to_string()),
            start_ms: 9000,
            end_ms: 12000,
            main_syllables: vec![
                LyricSyllable {
                    text: "We".to_string(),
                    start_ms: 9000,
                    end_ms: 9500,
                    ends_with_space: true,
                    ..Default::default()
                },
                LyricSyllable {
                    text: "sing".to_string(),
                    start_ms: 9600,
                    end_ms: 10200,
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
    ];

    let mut metadata = MetadataStore::new();
    metadata
        .add(
            &CanonicalMetadataKey::Songwriter.to_string(),
            "作曲家1号".to_string(),
        )
        .unwrap();

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Word,
        use_apple_format_rules: true,
        format: true,
        ..Default::default()
    };

    let ttml_output = generate_ttml(&lines, &metadata, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_auto_word_splitting_snapshot() {
    let lines = vec![LyricLine {
        start_ms: 1000,
        end_ms: 5000,
        main_syllables: vec![LyricSyllable {
            text: "Split this,你好世界".to_string(),
            start_ms: 1000,
            end_ms: 5000,
            ..Default::default()
        }],
        ..Default::default()
    }];
    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Word,
        auto_word_splitting: true,
        format: true,
        punctuation_weight: 0.1,
        ..Default::default()
    };

    let ttml_output = generate_ttml(&lines, &Default::default(), &options).unwrap();

    insta::assert_snapshot!(ttml_output);
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
    let result1 = parse_ttml(
        formatted_no_space_content,
        &DefaultLanguageOptions::default(),
    )
    .unwrap();

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
    let result2 = parse_ttml(
        formatted_with_space_content,
        &DefaultLanguageOptions::default(),
    )
    .unwrap();

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
        &DefaultLanguageOptions::default(),
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
    let result4 = parse_ttml(mixed_formatted_content, &DefaultLanguageOptions::default()).unwrap();

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
