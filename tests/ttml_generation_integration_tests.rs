use lyrics_helper_rs::converter::{
    generators::ttml_generator::generate_ttml,
    processors::metadata_processor::MetadataStore,
    types::{
        CanonicalMetadataKey, LyricLine, LyricSyllable, TtmlGenerationOptions, TtmlTimingMode,
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
