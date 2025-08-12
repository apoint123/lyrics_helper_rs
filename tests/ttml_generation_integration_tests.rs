use lyrics_helper_rs::converter::{
    generators::ttml_generator::generate_ttml,
    processors::metadata_processor::MetadataStore,
    types::{
        AgentStore, AnnotatedTrack, CanonicalMetadataKey, ContentType, LyricLine, LyricSyllable,
        LyricTrack, TrackMetadataKey, TtmlGenerationOptions, TtmlTimingMode, Word,
    },
};

use std::collections::HashMap;

#[test]
fn test_generate_line_timed_snapshot() {
    let lines = vec![
        LyricLine {
            start_ms: 1000,
            end_ms: 5000,
            tracks: vec![AnnotatedTrack {
                content_type: ContentType::Main,
                content: LyricTrack {
                    words: vec![Word {
                        syllables: vec![LyricSyllable {
                            text: "这是一行歌词".to_string(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        },
        LyricLine {
            start_ms: 6000,
            end_ms: 10000,
            tracks: vec![AnnotatedTrack {
                content_type: ContentType::Main,
                content: LyricTrack {
                    words: vec![Word {
                        syllables: vec![LyricSyllable {
                            text: "这是第二行歌词".to_string(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        },
    ];
    let mut metadata = MetadataStore::new();
    metadata.set_single(&CanonicalMetadataKey::Title.to_string(), "逐行歌曲");
    metadata.set_single(&CanonicalMetadataKey::Artist.to_string(), "测试艺术家");

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Line,
        format: true,
        ..Default::default()
    };

    let agent_store = AgentStore::from_metadata_store(&metadata);
    let ttml_output = generate_ttml(&lines, &metadata, &agent_store, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_generate_word_timed_with_agents_snapshot() {
    let lines = vec![
        LyricLine {
            agent: Some("v1".to_string()),
            start_ms: 5000,
            end_ms: 8200,
            tracks: vec![AnnotatedTrack {
                content_type: ContentType::Main,
                content: LyricTrack {
                    words: vec![Word {
                        syllables: vec![
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
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        },
        LyricLine {
            agent: Some("v1000".to_string()),
            start_ms: 9000,
            end_ms: 12000,
            tracks: vec![AnnotatedTrack {
                content_type: ContentType::Main,
                content: LyricTrack {
                    words: vec![Word {
                        syllables: vec![
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
                    }],
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        },
    ];
    let mut metadata = MetadataStore::new();
    metadata
        .add(&CanonicalMetadataKey::Songwriter.to_string(), "作曲家1号")
        .unwrap();

    metadata.add("agent", "v1=演唱者1号").unwrap();
    metadata.add("agent", "v1000=合唱").unwrap();

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Word,
        use_apple_format_rules: true,
        format: true,
        ..Default::default()
    };
    let agent_store = AgentStore::from_metadata_store(&metadata);
    let ttml_output = generate_ttml(&lines, &metadata, &agent_store, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_auto_word_splitting_snapshot() {
    let lines = vec![LyricLine {
        start_ms: 1000,
        end_ms: 5000,
        tracks: vec![AnnotatedTrack {
            content_type: ContentType::Main,
            content: LyricTrack {
                words: vec![Word {
                    syllables: vec![LyricSyllable {
                        text: "Split this,你好世界".to_string(),
                        start_ms: 1000,
                        end_ms: 5000,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            },
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
    let metadata = Default::default();
    let agent_store = AgentStore::from_metadata_store(&metadata);
    let ttml_output = generate_ttml(&lines, &metadata, &agent_store, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_generate_timed_romanization_snapshot() {
    let mut main_track_metadata = HashMap::new();
    main_track_metadata.insert(
        TrackMetadataKey::Custom("itunes_key".to_string()),
        "L1".to_string(),
    );
    let mut romanization_track_metadata = HashMap::new();
    romanization_track_metadata.insert(TrackMetadataKey::Language, "ja-Latn".to_string());

    let lines = vec![LyricLine {
        start_ms: 1000,
        end_ms: 2000,
        tracks: vec![AnnotatedTrack {
            content_type: ContentType::Main,
            content: LyricTrack {
                words: vec![Word {
                    syllables: vec![LyricSyllable {
                        text: "朝も".to_string(),
                        start_ms: 1000,
                        end_ms: 2000,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                metadata: main_track_metadata,
            },
            romanizations: vec![LyricTrack {
                words: vec![Word {
                    syllables: vec![
                        LyricSyllable {
                            text: "Asa".to_string(),
                            start_ms: 1000,
                            end_ms: 1500,
                            ..Default::default()
                        },
                        LyricSyllable {
                            text: "mo".to_string(),
                            start_ms: 1600,
                            end_ms: 2000,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                metadata: romanization_track_metadata,
            }],
            ..Default::default()
        }],
        ..Default::default()
    }];

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Word,
        use_apple_format_rules: true,
        format: true,
        ..Default::default()
    };
    let metadata = Default::default();
    let agent_store = AgentStore::from_metadata_store(&metadata);
    let ttml_output = generate_ttml(&lines, &metadata, &agent_store, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}

#[test]
fn test_generate_timed_translation_snapshot() {
    let mut main_track_metadata = HashMap::new();
    main_track_metadata.insert(
        TrackMetadataKey::Custom("itunes_key".to_string()),
        "L1".to_string(),
    );
    let mut translation_track_metadata = HashMap::new();
    translation_track_metadata.insert(TrackMetadataKey::Language, "zh-Hans".to_string());

    let lines = vec![LyricLine {
        start_ms: 1000,
        end_ms: 2000,
        tracks: vec![AnnotatedTrack {
            content_type: ContentType::Main,
            content: LyricTrack {
                words: vec![Word {
                    syllables: vec![LyricSyllable {
                        text: "鐘聲響起歸家".to_string(),
                        start_ms: 1000,
                        end_ms: 2000,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                metadata: main_track_metadata,
            },
            translations: vec![LyricTrack {
                words: vec![Word {
                    syllables: vec![
                        LyricSyllable {
                            text: "钟声响起".to_string(),
                            start_ms: 1000,
                            end_ms: 1500,
                            ..Default::default()
                        },
                        LyricSyllable {
                            text: "归家".to_string(),
                            start_ms: 1500,
                            end_ms: 2000,
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                metadata: translation_track_metadata,
            }],
            ..Default::default()
        }],
        ..Default::default()
    }];

    let options = TtmlGenerationOptions {
        timing_mode: TtmlTimingMode::Word,
        use_apple_format_rules: true,
        format: true,
        ..Default::default()
    };
    let metadata = Default::default();
    let agent_store = AgentStore::from_metadata_store(&metadata);
    let ttml_output = generate_ttml(&lines, &metadata, &agent_store, &options).unwrap();

    insta::assert_snapshot!(ttml_output);
}
