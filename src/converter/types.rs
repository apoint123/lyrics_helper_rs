//! 定义了歌词转换中使用的核心数据类型。

use std::{collections::HashMap, fmt, io, path::PathBuf, str::FromStr};

use bitflags::bitflags;
use quick_xml::{
    Error as QuickXmlErrorMain, encoding::EncodingError,
    events::attributes::AttrError as QuickXmlAttrError,
};
use serde::{Deserialize, Serialize};
use strum_macros::{EnumIter, EnumString};
use thiserror::Error;
use tracing::warn;

use crate::converter::processors::metadata_processor::MetadataStore;

//=============================================================================
// 1. 错误枚举
//=============================================================================

/// 定义歌词转换和处理过程中可能发生的各种错误。
#[derive(Error, Debug)]
pub enum ConvertError {
    /// XML 生成错误，通常来自 `quick-xml` 库。
    #[error("生成 XML 错误: {0}")]
    Xml(#[from] QuickXmlErrorMain),
    /// XML 属性解析错误，通常来自 `quick-xml` 库。
    #[error("XML 属性错误: {0}")]
    Attribute(#[from] QuickXmlAttrError),
    /// 整数解析错误。
    #[error("解析错误: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    /// 无效的时间格式字符串。
    #[error("无效的时间格式: {0}")]
    InvalidTime(String),
    /// 字符串格式化错误。
    #[error("格式错误: {0}")]
    Format(#[from] fmt::Error),
    /// 内部逻辑错误或未明确分类的错误。
    #[error("错误: {0}")]
    Internal(String),
    /// 文件读写等IO错误。
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),
    /// JSON 解析错误。
    #[error("解析 JSON 内容 {context} 失败: {source}")]
    JsonParse {
        /// 底层 `serde_json` 错误
        #[source]
        source: serde_json::Error,
        /// 有关错误发生位置的上下文信息。
        context: String,
    },
    /// JSON 结构不符合预期。
    #[error("JSON 结构无效: {0}")]
    InvalidJsonStructure(String),
    /// Base64 解码错误。
    #[error("Base64 解码错误: {0}")]
    Base64Decode(#[from] base64::DecodeError),
    /// 从字节序列转换为 UTF-8 字符串失败。
    #[error("UTF-8 转换错误: {0}")]
    FromUtf8(#[from] std::string::FromUtf8Error),
    /// 无效的歌词格式。
    #[error("无效的歌词格式: {0}")]
    InvalidLyricFormat(String),
    /// XML 文本编码或解码错误。
    #[error("文本编码或解码错误: {0}")]
    Encoding(#[from] EncodingError),
    /// 词组边界检测错误
    #[error("词组边界检测失败: {0}")]
    WordBoundaryDetection(String),
    /// 振假名解析错误
    #[error("振假名解析失败: {0}")]
    FuriganaParsingError(String),
    /// 轨道合并错误
    #[error("轨道合并失败: {0}")]
    TrackMergeError(String),
}

impl ConvertError {
    /// 创建一个带有上下文的 `JsonParse` 错误。
    #[must_use]
    pub fn json_parse(source: serde_json::Error, context: String) -> Self {
        Self::JsonParse { source, context }
    }
}

/// 定义从字符串解析 `CanonicalMetadataKey` 时可能发生的错误。
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParseCanonicalMetadataKeyError(String); // 存储无法解析的原始键字符串

impl fmt::Display for ParseCanonicalMetadataKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "未知或无效的元数据键: {}", self.0)
    }
}
impl std::error::Error for ParseCanonicalMetadataKeyError {}

//=============================================================================
// 2. 核心歌词格式枚举及相关
//=============================================================================

/// 枚举：表示支持的歌词格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Serialize, Deserialize, EnumIter)]
#[strum(ascii_case_insensitive)]
#[derive(Default)]
pub enum LyricFormat {
    /// `Advanced SubStation Alpha` 格式。
    Ass,
    /// `Timed Text Markup Language` 格式。
    #[default]
    Ttml,
    /// `Apple Music JSON` 格式 (内嵌TTML)。
    AppleMusicJson,
    /// `Lyricify Syllable` 格式。
    Lys,
    /// 标准 LRC (`LyRiCs`) 格式。
    Lrc,
    /// 增强型 LRC (Enhanced LRC) 格式，支持逐字时间戳。
    EnhancedLrc,
    /// QQ 音乐 QRC 格式。
    Qrc,
    /// 网易云音乐 YRC 格式。
    Yrc,
    /// `Lyricify Lines` 格式。
    Lyl,
    /// `Salt Player Lyrics` 格式。
    Spl,
    /// `Lyricify Quick Export` 格式。
    Lqe,
    /// 酷狗 KRC 格式。
    Krc,
}

impl LyricFormat {
    /// 将歌词格式枚举转换为对应的文件扩展名字符串。
    #[must_use]
    pub fn to_extension_str(self) -> &'static str {
        match self {
            LyricFormat::Ass => "ass",
            LyricFormat::Ttml => "ttml",
            LyricFormat::AppleMusicJson => "json",
            LyricFormat::Lys => "lys",
            LyricFormat::Lrc => "lrc",
            LyricFormat::EnhancedLrc => "elrc",
            LyricFormat::Qrc => "qrc",
            LyricFormat::Yrc => "yrc",
            LyricFormat::Lyl => "lyl",
            LyricFormat::Spl => "spl",
            LyricFormat::Lqe => "lqe",
            LyricFormat::Krc => "krc",
        }
    }

    /// 从字符串（通常是文件扩展名或用户输入）解析歌词格式枚举。
    /// 此方法不区分大小写，并会移除输入字符串中的空格和点。
    pub fn from_string(s: &str) -> Option<Self> {
        let normalized_s = s.to_uppercase().replace([' ', '.'], "");
        match normalized_s.as_str() {
            "ASS" | "SUBSTATIONALPHA" | "SSA" => Some(LyricFormat::Ass),
            "TTML" | "XML" => Some(LyricFormat::Ttml),
            "JSON" => Some(LyricFormat::AppleMusicJson),
            "LYS" | "LYRICIFYSYLLABLE" => Some(LyricFormat::Lys),
            "LRC" => Some(LyricFormat::Lrc),
            "ENHANCEDLRC" | "LRCX" | "ELRC" | "ALRC" => Some(LyricFormat::EnhancedLrc),
            "QRC" => Some(LyricFormat::Qrc),
            "YRC" => Some(LyricFormat::Yrc),
            "LYL" | "LYRICIFYLINES" => Some(LyricFormat::Lyl),
            "SPL" => Some(LyricFormat::Spl),
            "LQE" | "LYRICIFYQUICKEXPORT" => Some(LyricFormat::Lqe),
            "KRC" => Some(LyricFormat::Krc),
            _ => {
                warn!("[LyricFormat] 未知的格式字符串: {}", s);
                None
            }
        }
    }
}

impl fmt::Display for LyricFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LyricFormat::Ass => write!(f, "ASS"),
            LyricFormat::Ttml => write!(f, "TTML"),
            LyricFormat::AppleMusicJson => write!(f, "JSON (Apple Music)"),
            LyricFormat::Lys => write!(f, "Lyricify Syllable"),
            LyricFormat::Lrc => write!(f, "LRC"),
            LyricFormat::EnhancedLrc => write!(f, "Enhanced LRC"),
            LyricFormat::Qrc => write!(f, "QRC"),
            LyricFormat::Yrc => write!(f, "YRC"),
            LyricFormat::Lyl => write!(f, "Lyricify Lines"),
            LyricFormat::Spl => write!(f, "SPL"),
            LyricFormat::Lqe => write!(f, "Lyricify Quick Export"),
            LyricFormat::Krc => write!(f, "KRC"),
        }
    }
}

//=============================================================================
// 3. 歌词内部表示结构
//=============================================================================

/// 定义可以被注解的内容轨道类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ContentType {
    #[default]
    /// 主歌词
    Main,
    /// 背景人声
    Background,
}

/// 定义轨道元数据的规范化键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrackMetadataKey {
    /// BCP 47 语言代码
    Language,
    /// 罗马音方案名
    Scheme,
    /// 自定义元数据键
    Custom(String),
}

/// 表示振假名中的一个音节。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FuriganaSyllable {
    /// 振假名文本内容
    pub text: String,
    /// 可选的时间戳 (`start_ms`, `end_ms`)
    pub timing: Option<(u64, u64)>,
}

/// 表示一个语义上的"单词"或"词组"，主要为振假名服务。
///
/// 目前还没有歌词格式提供词组信息，应将整行直接作为一个词组。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    /// 组成该词的音节列表
    pub syllables: Vec<LyricSyllable>,
    /// 可选的振假名信息
    pub furigana: Option<Vec<FuriganaSyllable>>,
}

/// 一个通用的歌词轨道。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricTrack {
    /// 组成该轨道的音节列表。
    pub words: Vec<Word>,
    /// 轨道元数据。
    #[serde(default)]
    pub metadata: HashMap<TrackMetadataKey, String>,
}

/// 将一个内容轨道（如主歌词）及其所有注解轨道（如翻译、罗马音）绑定在一起的结构。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnnotatedTrack {
    /// 该内容轨道的类型。
    pub content_type: ContentType,

    /// 内容轨道本身。
    pub content: LyricTrack,

    /// 依附于该内容轨道的翻译轨道列表。
    #[serde(default)]
    pub translations: Vec<LyricTrack>,

    /// 依附于该内容轨道的罗马音轨道列表。
    #[serde(default)]
    pub romanizations: Vec<LyricTrack>,
}

/// 表示一位演唱者的类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AgentType {
    #[default]
    /// 单人演唱。
    Person,
    /// 合唱。
    Group,
    /// 未指定或其它类型。
    Other,
}

/// 表示歌词中的演唱者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// 内部ID, 例如 "v1"
    pub id: String,
    /// 可选的完整名称，例如 "演唱者1号"
    pub name: Option<String>,
    /// Agent 的类型
    pub agent_type: AgentType,
}

/// 用于存储歌词轨道中识别到的所有演唱者。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStore {
    /// 从 ID 到演唱者结构体的映射。
    pub agents_by_id: HashMap<String, Agent>,
}

impl AgentStore {
    /// 从歌词行列表中构建 `AgentStore`
    #[must_use]
    pub fn from_metadata_store(metadata_store: &MetadataStore) -> Self {
        let mut store = AgentStore::default();

        if let Some(agent_definitions) = metadata_store.get_multiple_values_by_key("agent") {
            for def_string in agent_definitions {
                let (id, parsed_name) = match def_string.split_once('=') {
                    Some((id, name)) => (id.to_string(), Some(name.to_string())),
                    None => (def_string.clone(), None),
                };

                let is_chorus = id == "v1000"
                    || parsed_name.as_deref() == Some("合")
                    || parsed_name.as_deref() == Some("合唱");

                let final_name = if is_chorus { None } else { parsed_name };
                let agent_type = if is_chorus {
                    AgentType::Group
                } else {
                    AgentType::Person
                };

                let agent = Agent {
                    id: id.clone(),
                    name: final_name,
                    agent_type,
                };
                store.agents_by_id.insert(id, agent);
            }
        }
        store
    }

    /// 获取所有 Agent 的迭代器
    pub fn all_agents(&self) -> impl Iterator<Item = &Agent> {
        self.agents_by_id.values()
    }
}

/// 歌词行结构，作为多个并行带注解轨道的容器。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 该行包含的所有带注解的轨道。
    pub tracks: Vec<AnnotatedTrack>,
    /// 行的开始时间，相对于歌曲开始的绝对时间（毫秒）。
    pub start_ms: u64,
    /// 行的结束时间，相对于歌曲开始的绝对时间（毫秒）。
    pub end_ms: u64,
    /// 可选的演唱者标识。
    ///
    /// 应该为数字 ID，例如 "v1"，"v1000"。
    pub agent: Option<String>,
    /// 可选的歌曲组成部分标记。
    pub song_part: Option<String>,
    /// 可选的 iTunes Key (如 "L1", "L2")。
    pub itunes_key: Option<String>,
}

impl LyricTrack {
    /// 将轨道内所有音节的文本拼接成一个完整的字符串。
    #[must_use]
    pub fn text(&self) -> String {
        self.words
            .iter()
            .flat_map(|word| &word.syllables)
            .map(|syl| {
                if syl.ends_with_space {
                    format!("{} ", syl.text)
                } else {
                    syl.text.clone()
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string()
    }
}

impl LyricLine {
    /// 创建一个带有指定时间戳的空 `LyricLine`。
    #[must_use]
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self {
            start_ms,
            end_ms,
            ..Default::default()
        }
    }

    /// 返回一个迭代器，用于遍历所有指定内容类型的带注解轨道。
    pub fn tracks_by_type(
        &self,
        content_type: ContentType,
    ) -> impl Iterator<Item = &AnnotatedTrack> {
        self.tracks
            .iter()
            .filter(move |t| t.content_type == content_type)
    }

    /// 返回一个迭代器，用于遍历所有主歌词轨道 (`ContentType::Main`)。
    pub fn main_tracks(&self) -> impl Iterator<Item = &AnnotatedTrack> {
        self.tracks_by_type(ContentType::Main)
    }

    /// 返回一个迭代器，用于遍历所有背景人声音轨 (`ContentType::Background`)。
    pub fn background_tracks(&self) -> impl Iterator<Item = &AnnotatedTrack> {
        self.tracks_by_type(ContentType::Background)
    }

    /// 获取第一个主歌词轨道（如果存在）。
    #[must_use]
    pub fn main_track(&self) -> Option<&AnnotatedTrack> {
        self.main_tracks().next()
    }

    /// 获取第一个背景人声音轨（如果存在）。
    #[must_use]
    pub fn background_track(&self) -> Option<&AnnotatedTrack> {
        self.background_tracks().next()
    }

    /// 获取第一个主歌词轨道的完整文本（如果存在）。
    #[must_use]
    pub fn main_text(&self) -> Option<String> {
        self.main_track().map(|t| t.content.text())
    }

    /// 获取第一个背景人声轨道的完整文本（如果存在）。
    #[must_use]
    pub fn background_text(&self) -> Option<String> {
        self.background_track().map(|t| t.content.text())
    }

    /// 向该行添加一个预先构建好的带注解轨道。
    pub fn add_track(&mut self, track: AnnotatedTrack) {
        self.tracks.push(track);
    }

    /// 向该行添加一个新的、简单的内容轨道（主歌词或背景）。
    ///
    /// # 参数
    /// * `content_type` - 轨道的类型 (`Main` 或 `Background`)。
    /// * `text` - 该轨道的完整文本。
    pub fn add_content_track(&mut self, content_type: ContentType, text: impl Into<String>) {
        let syllable = LyricSyllable {
            text: text.into(),
            start_ms: self.start_ms,
            end_ms: self.end_ms,
            ..Default::default()
        };
        let track = AnnotatedTrack {
            content_type,
            content: LyricTrack {
                words: vec![Word {
                    syllables: vec![syllable],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        self.add_track(track);
    }

    /// 为该行中所有指定类型的内容轨道添加一个翻译。
    /// 例如，可用于为所有主歌词轨道添加一个统一的翻译。
    pub fn add_translation(
        &mut self,
        content_type: ContentType,
        text: impl Into<String>,
        language: Option<&str>,
    ) {
        let text = text.into();
        for track in self
            .tracks
            .iter_mut()
            .filter(|t| t.content_type == content_type)
        {
            let mut metadata = HashMap::new();
            if let Some(lang) = language {
                metadata.insert(TrackMetadataKey::Language, lang.to_string());
            }
            let translation_track = LyricTrack {
                words: vec![Word {
                    syllables: vec![LyricSyllable {
                        text: text.clone(),
                        start_ms: self.start_ms,
                        end_ms: self.end_ms,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                metadata,
            };
            track.translations.push(translation_track);
        }
    }

    /// 为该行中所有指定类型的内容轨道添加一个罗马音。
    pub fn add_romanization(
        &mut self,
        content_type: ContentType,
        text: impl Into<String>,
        scheme: Option<&str>,
    ) {
        let text = text.into();
        for track in self
            .tracks
            .iter_mut()
            .filter(|t| t.content_type == content_type)
        {
            let mut metadata = HashMap::new();
            if let Some(s) = scheme {
                metadata.insert(TrackMetadataKey::Scheme, s.to_string());
            }
            let romanization_track = LyricTrack {
                words: vec![Word {
                    syllables: vec![LyricSyllable {
                        text: text.clone(),
                        start_ms: self.start_ms,
                        end_ms: self.end_ms,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                metadata,
            };
            track.romanizations.push(romanization_track);
        }
    }

    /// 移除所有指定类型的内容轨道及其所有注解。
    pub fn clear_tracks(&mut self, content_type: ContentType) {
        self.tracks.retain(|t| t.content_type != content_type);
    }
}

/// 通用的歌词音节结构，用于表示逐字歌词中的一个音节。
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricSyllable {
    /// 音节的文本内容。
    pub text: String,
    /// 音节开始时间，相对于歌曲开始的绝对时间（毫秒）。
    pub start_ms: u64,
    /// 音节结束时间，相对于歌曲开始的绝对时间（毫秒）。
    pub end_ms: u64,
    /// 可选的音节持续时间（毫秒）。
    /// 如果提供，`end_ms` 可以由 `start_ms + duration_ms` 计算得出，反之亦然。
    /// 解析器应确保 `start_ms` 和 `end_ms` 最终被填充。
    pub duration_ms: Option<u64>,
    /// 指示该音节后是否应有空格。
    pub ends_with_space: bool,
}

//=============================================================================
// 4. 元数据结构体
//=============================================================================

/// 定义元数据的规范化键。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanonicalMetadataKey {
    /// 歌曲标题。
    Title,
    /// 艺术家。
    Artist,
    /// 专辑名。
    Album,
    /// 主歌词的语言代码 (BCP 47)。
    Language,
    /// 全局时间偏移量（毫秒）。
    Offset,
    /// 词曲作者。
    Songwriter,
    /// 网易云音乐 ID。
    NcmMusicId,
    /// QQ音乐 ID。
    QqMusicId,
    /// Spotify ID。
    SpotifyId,
    /// Apple Music ID。
    AppleMusicId,
    /// 国际标准音像制品编码 (International Standard Recording Code)。
    Isrc,
    /// 逐词歌词作者 Github ID。
    TtmlAuthorGithub,
    /// 逐词歌词作者 GitHub 用户名。
    TtmlAuthorGithubLogin,

    /// 用于所有其他未明确定义的标准或非标准元数据键。
    Custom(String),
}

impl fmt::Display for CanonicalMetadataKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key_name = match self {
            CanonicalMetadataKey::Title => "Title",
            CanonicalMetadataKey::Artist => "Artist",
            CanonicalMetadataKey::Album => "Album",
            CanonicalMetadataKey::Language => "Language",
            CanonicalMetadataKey::Offset => "Offset",
            CanonicalMetadataKey::Songwriter => "Songwriter",
            CanonicalMetadataKey::NcmMusicId => "NcmMusicId",
            CanonicalMetadataKey::QqMusicId => "QqMusicId",
            CanonicalMetadataKey::SpotifyId => "SpotifyId",
            CanonicalMetadataKey::AppleMusicId => "AppleMusicId",
            CanonicalMetadataKey::Isrc => "Isrc",
            CanonicalMetadataKey::TtmlAuthorGithub => "TtmlAuthorGithub",
            CanonicalMetadataKey::TtmlAuthorGithubLogin => "TtmlAuthorGithubLogin",
            CanonicalMetadataKey::Custom(s) => s.as_str(),
        };
        write!(f, "{key_name}")
    }
}

impl CanonicalMetadataKey {
    /// 定义哪些键应该被显示出来
    #[must_use]
    pub fn is_public(&self) -> bool {
        matches!(
            self,
            Self::Title
                | Self::Artist
                | Self::Album
                | Self::NcmMusicId
                | Self::QqMusicId
                | Self::SpotifyId
                | Self::AppleMusicId
                | Self::Isrc
                | Self::TtmlAuthorGithub
                | Self::TtmlAuthorGithubLogin
        )
    }
}

impl FromStr for CanonicalMetadataKey {
    type Err = ParseCanonicalMetadataKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ti" | "title" | "musicname" => Ok(Self::Title),
            "ar" | "artist" | "artists" => Ok(Self::Artist),
            "al" | "album" => Ok(Self::Album),
            "by" | "ttmlauthorgithublogin" => Ok(Self::TtmlAuthorGithubLogin),
            "language" | "lang" => Ok(Self::Language),
            "offset" => Ok(Self::Offset),
            "songwriter" | "songwriters" => Ok(Self::Songwriter),
            "ncmmusicid" => Ok(Self::NcmMusicId),
            "qqmusicid" => Ok(Self::QqMusicId),
            "spotifyid" => Ok(Self::SpotifyId),
            "applemusicid" => Ok(Self::AppleMusicId),
            "isrc" => Ok(Self::Isrc),
            "ttmlauthorgithub" => Ok(Self::TtmlAuthorGithub),
            custom_key if !custom_key.is_empty() => Ok(Self::Custom(custom_key.to_string())),
            _ => Err(ParseCanonicalMetadataKeyError(s.to_string())),
        }
    }
}

//=============================================================================
// 5. 处理与数据结构体
//=============================================================================

/// 存储从源文件解析出的、准备进行进一步处理或转换的歌词数据。
/// 这是解析阶段的主要输出，也是后续处理和生成阶段的主要输入。
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedSourceData {
    /// 解析后的歌词行列表。
    pub lines: Vec<LyricLine>,
    /// 从文件头或特定元数据标签中解析出的原始（未规范化）元数据。
    /// 键是原始元数据标签名，值是该标签对应的所有值（因为某些标签可能出现多次）。
    pub raw_metadata: HashMap<String, Vec<String>>,
    /// 解析的源文件格式。
    pub source_format: LyricFormat,
    /// 从文件中解析出的所有演唱者信息。
    #[serde(default)]
    pub agents: AgentStore,
    /// 可选的原始文件名，可用于日志记录或某些特定转换逻辑。
    pub source_filename: Option<String>,
    /// 指示源文件是否是逐行歌词（例如LRC）。
    pub is_line_timed_source: bool,
    /// 解析过程中产生的警告信息列表。
    pub warnings: Vec<String>,
    /// 如果源文件是内嵌TTML的JSON，此字段存储原始的TTML字符串内容。
    pub raw_ttml_from_input: Option<String>,
    /// 指示输入的TTML（来自`raw_ttml_from_input`）是否被格式化。
    /// 这影响空格和换行的处理。
    pub detected_formatted_ttml_input: Option<bool>,
    /// 提供商名称
    pub source_name: String,
}

//=============================================================================
// 6. 辅助类型与函数
//=============================================================================

/// 表示从ASS中提取的标记信息。
/// 元组的第一个元素是原始行号，第二个元素是标记文本。
pub type MarkerInfo = (usize, String);

/// 定义 LYS 格式使用的歌词行属性。
pub mod lys_properties {
    /// 视图：未设置，人声：未设置
    pub const UNSET_UNSET: u8 = 0;
    /// 视图：左，人声：未设置
    pub const UNSET_LEFT: u8 = 1;
    /// 视图：右，人声：未设置
    pub const UNSET_RIGHT: u8 = 2;
    /// 视图：未设置，人声：主歌词
    pub const MAIN_UNSET: u8 = 3;
    /// 视图：左，人声：主歌词
    pub const MAIN_LEFT: u8 = 4;
    /// 视图：右，人声：主歌词
    pub const MAIN_RIGHT: u8 = 5;
    /// 视图：未设置，人声：背景
    pub const BG_UNSET: u8 = 6;
    /// 视图：左，人声：背景
    pub const BG_LEFT: u8 = 7;
    /// 视图：右，人声：背景
    pub const BG_RIGHT: u8 = 8;
}

//=============================================================================
// 7. 批量转换相关结构体
//=============================================================================

/// 批量加载文件的唯一标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BatchFileId(pub u64);
impl Default for BatchFileId {
    fn default() -> Self {
        Self::new()
    }
}

impl BatchFileId {
    /// 生成一个新的唯一 `BatchFileId`。
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        // 使用静态原子计数器确保ID的唯一性。
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        BatchFileId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// 表示在批量转换模式下加载的单个文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchLoadedFile {
    /// 文件的唯一ID。
    pub id: BatchFileId,
    /// 文件的完整路径。
    pub path: PathBuf,
    /// 从路径中提取的文件名。
    pub filename: String,
}
impl BatchLoadedFile {
    /// 根据文件路径创建一个新的 `BatchLoadedFile` 实例。
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        Self {
            id: BatchFileId::new(),
            path,
            filename,
        }
    }
}

/// 表示批量转换中单个条目的状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BatchEntryStatus {
    /// 等待转换。
    Pending,
    /// 准备好进行转换。
    ReadyToConvert,
    /// 正在转换中。
    Converting,
    /// 转换完成。
    Completed {
        /// 输出文件的路径。
        output_path: PathBuf,
        /// 转换过程中产生的警告信息。
        warnings: Vec<String>,
    },
    /// 转换失败。
    Failed(String),
    /// 跳过转换，通常因为在配对逻辑中未能找到匹配的主歌词文件（针对辅助歌词文件）。
    SkippedNoMatch,
}

/// 批量转换配置的唯一标识符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BatchConfigId(pub u64);
impl Default for BatchConfigId {
    fn default() -> Self {
        Self::new()
    }
}

impl BatchConfigId {
    /// 生成一个新的唯一 `BatchConfigId`。
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        BatchConfigId(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// 表示单个批量转换任务的配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConversionConfig {
    /// 配置的唯一ID。
    pub id: BatchConfigId,
    /// 主歌词文件的ID。
    pub main_lyric_id: BatchFileId,
    /// 关联的翻译歌词文件的ID列表。
    pub translation_lyric_ids: Vec<BatchFileId>,
    /// 关联的罗马音文件的ID列表。
    pub romanization_lyric_ids: Vec<BatchFileId>,
    /// 目标输出格式。
    pub target_format: LyricFormat,
    /// 用于UI预览的输出文件名（实际输出路径在任务执行时结合输出目录确定）。
    pub output_filename_preview: String,
    /// 当前转换任务的状态。
    pub status: BatchEntryStatus,
    /// 如果任务失败，存储相关的错误信息。
    pub last_error: Option<String>,
}

impl BatchConversionConfig {
    /// 创建一个新的 `BatchConversionConfig` 实例。
    #[must_use]
    pub fn new(
        main_lyric_id: BatchFileId,
        target_format: LyricFormat,
        output_filename: String,
    ) -> Self {
        Self {
            id: BatchConfigId::new(),
            main_lyric_id,
            translation_lyric_ids: Vec::new(),
            romanization_lyric_ids: Vec::new(),
            target_format,
            output_filename_preview: output_filename,
            status: BatchEntryStatus::Pending,
            last_error: None,
        }
    }
}

/// 用于在Rust后端内部传递批量转换任务状态更新的消息。
#[derive(Debug, Clone)]
pub struct BatchTaskUpdate {
    /// 关联的批量转换配置ID。
    pub entry_config_id: BatchConfigId,
    /// 更新后的任务状态。
    pub new_status: BatchEntryStatus,
}

/// 用于表示传递给核心转换函数的单个输入文件的信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputFile {
    /// 文件内容字符串。
    pub content: String,
    /// 该文件内容的歌词格式。
    pub format: LyricFormat,
    /// 可选的语言代码。
    /// 对于翻译或罗马音文件，指示其语言或具体方案。
    pub language: Option<String>,
    /// 可选的原始文件名。
    /// 可用于日志记录、元数据提取或某些特定转换逻辑。
    pub filename: Option<String>,
}

impl InputFile {
    /// 创建一个新的 `InputFile` 实例。
    ///
    /// 这是一个便利的构造函数，用于简化 `InputFile` 对象的创建，
    /// 使其在库的顶层 API (如 `lib.rs` 的示例) 中更易于使用。
    ///
    /// # 参数
    /// * `content` - 歌词文件的原始文本内容。
    /// * `format` - 歌词的格式 (`LyricFormat` 枚举)。
    /// * `language` - 可选的语言代码 (BCP-47 格式，例如 "zh-Hans")。
    /// * `filename` - 可选的原始文件名，用于提供上下文。
    #[must_use]
    pub fn new(
        content: String,
        format: LyricFormat,
        language: Option<String>,
        filename: Option<String>,
    ) -> Self {
        Self {
            content,
            format,
            language,
            filename,
        }
    }
}

impl Default for InputFile {
    ///
    /// 创建一个默认的 `InputFile` 实例。
    ///
    /// 这对于某些场景下需要一个“占位符”或空的 `InputFile` 实例非常有用。
    /// 默认值包括：
    /// - `content`: 空字符串
    /// - `format`: `LyricFormat` 的默认值，即 TTML
    /// - `language`: None
    /// - `filename`: None
    ///
    fn default() -> Self {
        Self {
            content: String::new(),
            format: LyricFormat::default(),
            language: None,
            filename: None,
        }
    }
}

/// 封装了调用核心歌词转换函数所需的所有输入参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionInput {
    /// 主歌词文件信息。
    pub main_lyric: InputFile,
    /// 翻译文件信息列表。每个 `InputFile` 包含内容、格式（通常是LRC）和语言。
    pub translations: Vec<InputFile>,
    /// 罗马音/音译文件信息列表。每个 `InputFile` 包含内容、格式（通常是LRC）和语言/方案。
    pub romanizations: Vec<InputFile>,
    /// 目标歌词格式。
    pub target_format: LyricFormat,
    /// 可选的用户指定的元数据覆盖（原始键值对）。
    pub user_metadata_overrides: Option<HashMap<String, Vec<String>>>,
    // /// 可选的应用级别的固定元数据规则（原始键值对）。
    // pub fixed_metadata_rules: Option<HashMap<String, Vec<String>>>,
}

/// TTML 生成时的计时模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TtmlTimingMode {
    #[default]
    /// 逐字计时
    Word,
    /// 逐行计时
    Line,
}

/// TTML 解析选项
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TtmlParsingOptions {
    /// 当TTML本身未指定语言时，解析器可以使用的默认语言。
    #[serde(default)]
    pub default_languages: DefaultLanguageOptions,

    /// 强制指定计时模式，忽略文件内的 `itunes:timing` 属性和自动检测逻辑。
    #[serde(default)]
    pub force_timing_mode: Option<TtmlTimingMode>,
}

/// TTML 生成选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtmlGenerationOptions {
    /// 生成的计时模式（逐字或逐行）。
    pub timing_mode: TtmlTimingMode,
    /// 指定输出 TTML 的主语言 (xml:lang)。如果为 None，则尝试从元数据推断。
    pub main_language: Option<String>,
    /// 为内联的翻译 `<span>` 指定默认语言代码。
    pub translation_language: Option<String>,
    /// 为内联的罗马音 `<span>` 指定默认语言代码。
    pub romanization_language: Option<String>,
    /// 是否遵循 Apple Music 的特定格式规则（例如，将翻译写入`<head>`而不是内联）。
    pub use_apple_format_rules: bool,
    /// 是否输出格式化的 TTML 文件。
    pub format: bool,
    /// 是否启用自动分词功能。
    pub auto_word_splitting: bool,
    /// 自动分词时，一个标点符号所占的权重（一个字符的权重为1.0）。
    pub punctuation_weight: f64,
}

impl Default for TtmlGenerationOptions {
    fn default() -> Self {
        Self {
            timing_mode: TtmlTimingMode::Word,
            main_language: None,
            translation_language: None,
            romanization_language: None,
            use_apple_format_rules: false,
            format: false,
            auto_word_splitting: false,
            punctuation_weight: 0.3,
        }
    }
}

/// TTML 解析时使用的默认语言选项
/// 当TTML本身未指定语言时，解析器可以使用这些值。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DefaultLanguageOptions {
    /// 默认主语言代码
    pub main: Option<String>,
    /// 默认翻译语言代码
    pub translation: Option<String>,
    /// 默认罗马音语言代码
    pub romanization: Option<String>,
}

/// LQE 生成选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LqeGenerationOptions {
    /// 用于 [lyrics] 区块的格式
    pub main_lyric_format: LyricFormat,
    /// 用于 [translation] 和 [pronunciation] 区块的格式
    pub auxiliary_format: LyricFormat,
}

impl Default for LqeGenerationOptions {
    fn default() -> Self {
        Self {
            main_lyric_format: LyricFormat::Lys,
            auxiliary_format: LyricFormat::Lrc,
        }
    }
}

/// 定义辅助歌词（翻译、音译）与主歌词的匹配策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuxiliaryLineMatchingStrategy {
    /// 精确匹配：要求时间戳完全相同。对时间差异敏感。
    Exact,
    /// 容差匹配：在预设的时间窗口内寻找匹配。
    Tolerance {
        /// 匹配时允许的最大时间差（毫秒）。
        tolerance_ms: u64,
    },
    /// 同步匹配：假定主歌词和辅助歌词都按时间排序，使用双指针算法在时间窗口内匹配。
    SortedSync {
        /// 匹配时允许的最大时间差（毫秒）。
        tolerance_ms: u64,
    },
}

impl Default for AuxiliaryLineMatchingStrategy {
    fn default() -> Self {
        Self::SortedSync { tolerance_ms: 20 }
    }
}

/// 指定LRC中具有相同时间戳的行的角色
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LrcLineRole {
    /// 主歌词
    Main,
    /// 翻译
    Translation,
    /// 罗马音
    Romanization,
}

/// 定义如何处理LRC中具有相同时间戳的多行歌词的策略
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LrcSameTimestampStrategy {
    /// [默认] 将文件顺序中的第一行视为主歌词，其余的都视为翻译。
    #[default]
    FirstIsMain,
    /// 使用启发式算法自动判断主歌词、翻译和罗马音。
    Heuristic,
    /// 将每一行都视为一个独立的、并列的主歌词轨道。
    AllAreMain,
    /// 根据用户提供的角色列表，按顺序为每一行分配角色。
    /// 列表的长度应与具有相同时间戳的行数相匹配。
    UseRoleOrder(Vec<LrcLineRole>),
}

/// LRC 解析选项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LrcParsingOptions {
    /// 定义如何处理具有相同时间戳的多行歌词的策略。
    #[serde(default)]
    pub same_timestamp_strategy: LrcSameTimestampStrategy,
}

/// 统一管理所有格式的转换选项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversionOptions {
    /// TTML 生成选项
    pub ttml: TtmlGenerationOptions,
    /// TTML 解析选项
    #[serde(default)]
    pub ttml_parsing: TtmlParsingOptions,
    /// LQE 转换选项
    #[serde(default)]
    pub lqe: LqeGenerationOptions,
    /// ASS 转换选项
    pub ass: AssGenerationOptions,
    /// LRC 转换选项
    #[serde(default)]
    pub lrc: LrcGenerationOptions,
    /// LRC 解析选项
    #[serde(default)]
    pub lrc_parsing: LrcParsingOptions,
    /// 元数据移除选项
    pub metadata_stripper: MetadataStripperOptions,
    /// 简繁转换选项
    #[serde(default)]
    pub chinese_conversion: ChineseConversionOptions,
    /// 辅助歌词（如翻译）的匹配策略
    #[serde(default)]
    pub matching_strategy: AuxiliaryLineMatchingStrategy,
}

/// ASS 生成转换选项
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AssGenerationOptions {
    /// 自定义的 [Script Info] 部分内容。如果为 `None`，则使用默认值。
    /// 用户提供的内容应包含 `[Script Info]` 头部。
    pub script_info: Option<String>,
    /// 自定义的 [V4+ Styles] 部分内容。如果为 `None`，则使用默认值。
    /// 用户提供的内容应包含 `[V4+ Styles]` 头部和 `Format:` 行。
    pub styles: Option<String>,
}

bitflags! {
    /// 元数据清理器的配置标志
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MetadataStripperFlags: u8 {
        /// 启用元数据清理功能
        const ENABLED                 = 1 << 0;
        /// 关键词匹配区分大小写
        const KEYWORD_CASE_SENSITIVE  = 1 << 1;
        /// 启用基于正则表达式的行移除
        const ENABLE_REGEX_STRIPPING  = 1 << 2;
        /// 正则表达式匹配区分大小写
        const REGEX_CASE_SENSITIVE    = 1 << 3;
    }
}

impl Default for MetadataStripperFlags {
    fn default() -> Self {
        Self::ENABLED | Self::ENABLE_REGEX_STRIPPING
    }
}

/// 配置元数据行清理器的选项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetadataStripperOptions {
    /// 用于控制清理器行为的位标志。
    #[serde(default)]
    pub flags: MetadataStripperFlags,

    /// 用于匹配头部/尾部块的关键词列表。
    /// 如果为 `None`，将使用一组内建的默认关键词。
    pub keywords: Option<Vec<String>>,

    /// 用于匹配并移除任意行的正则表达式列表。
    /// 如果为 `None`，将使用一组内建的默认正则表达式。
    pub regex_patterns: Option<Vec<String>>,
}

/// 简繁转换的模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChineseConversionMode {
    /// 直接替换原文
    #[default]
    Replace,
    /// 作为翻译条目添加
    AddAsTranslation,
}

/// 为 `ferrous_opencc::config::BuiltinConfig` 提供扩展方法
pub trait BuiltinConfigExt {
    /// 推断配置对应的目标语言标签
    fn deduce_lang_tag(self) -> Option<&'static str>;
}

impl BuiltinConfigExt for ferrous_opencc::config::BuiltinConfig {
    fn deduce_lang_tag(self) -> Option<&'static str> {
        use ferrous_opencc::config::BuiltinConfig;
        match self {
            BuiltinConfig::S2t
            | BuiltinConfig::Jp2t
            | BuiltinConfig::Hk2t
            | BuiltinConfig::Tw2t => Some("zh-Hant"),
            BuiltinConfig::S2tw | BuiltinConfig::S2twp | BuiltinConfig::T2tw => Some("zh-Hant-TW"),
            BuiltinConfig::S2hk | BuiltinConfig::T2hk => Some("zh-Hant-HK"),
            BuiltinConfig::T2s
            | BuiltinConfig::Tw2s
            | BuiltinConfig::Tw2sp
            | BuiltinConfig::Hk2s => Some("zh-Hans"),
            BuiltinConfig::T2jp => Some("ja"),
        }
    }
}

/// 简繁转换的配置选项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChineseConversionOptions {
    /// 指定要使用的 `OpenCC` 配置。
    /// 当值为 `Some(config)` 时，功能启用。
    pub config: Option<ferrous_opencc::config::BuiltinConfig>,

    /// 为翻译指定 BCP 47 语言标签，例如 "zh-Hant" 或 "zh-Hant-HK"。
    /// 如果未指定，将根据配置自动推断。
    pub target_lang_tag: Option<String>,

    /// 指定转换模式，默认为直接替换
    #[serde(default)]
    pub mode: ChineseConversionMode,
}

/// LRC 生成时，背景人声的输出方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LrcSubLinesOutputMode {
    /// 默认忽略所有背景人声，只输出主歌词
    #[default]
    Ignore,
    /// 将子行用括号合并到主行中，如 "主歌词 (背景人声)"
    MergeWithParentheses,
    /// 将背景人声行作为独立的、带时间戳的歌词行输出
    SeparateLines,
}

/// LRC 生成时，行结束时间标记 `[mm:ss.xx]` 的输出方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LrcEndTimeOutputMode {
    /// [默认] 不输出任何结束时间标记
    #[default]
    Never,
    /// 为每一行歌词都输出一个结束时间标记
    Always,
    /// 仅在当前行与下一行的时间间隔超过阈值时，才输出结束标记
    OnLongPause {
        /// 触发输出的最小暂停时长（毫秒）
        threshold_ms: u64,
    },
}

/// LRC 生成选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LrcGenerationOptions {
    /// 控制背景人声的输出方式
    pub sub_lines_output_mode: LrcSubLinesOutputMode,
    /// 控制行结束时间标记的输出方式
    pub end_time_output_mode: LrcEndTimeOutputMode,
}

impl Default for LrcGenerationOptions {
    fn default() -> Self {
        Self {
            sub_lines_output_mode: LrcSubLinesOutputMode::Ignore,
            end_time_output_mode: LrcEndTimeOutputMode::Never,
        }
    }
}

// =============================================================================
// 8. 转换任务入口结构体
// =============================================================================

/// 用于批量转换的输入参数。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchInput {
    /// 包含源歌词文件的输入目录。
    pub input_dir: PathBuf,
    /// 用于保存转换后文件的输出目录。
    pub output_dir: PathBuf,
    /// 所有任务的目标输出格式。
    pub target_format: LyricFormat,
}

/// 表示一个转换任务，可以是单个文件或批量处理。
#[derive(Debug, Clone)]
pub enum ConversionTask {
    /// 单个转换任务，输入为内存中的内容。
    Single(ConversionInput),
    /// 批量转换任务，输入为文件目录。
    Batch(BatchInput),
}

/// 表示转换操作的输出结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConversionResult {
    /// 单个文件转换的结果，为一个字符串。
    Single(String),
    /// 批量转换的结果，为所有任务的最终状态列表。
    Batch(Vec<BatchConversionConfig>),
}

/// 包含完整转换结果的结构体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullConversionResult {
    /// 最终生成的歌词字符串。
    pub output_lyrics: String,
    /// 在转换开始时从输入解析出的源数据。
    pub source_data: ParsedSourceData,
}

// =============================================================================
// 9. 平滑优化选项
// =============================================================================

/// 控制平滑优化的选项。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SyllableSmoothingOptions {
    /// 用于平滑的因子 (0.0 ~ 0.5)。
    pub factor: f64,
    /// 用于分组的时长差异阈值（毫秒）。
    pub duration_threshold_ms: u64,
    /// 用于分组的间隔阈值（毫秒）。
    pub gap_threshold_ms: u64,
    /// 组内平滑的次数。
    pub smoothing_iterations: u32,
}

impl Default for SyllableSmoothingOptions {
    fn default() -> Self {
        Self {
            factor: 0.15,
            duration_threshold_ms: 50,
            gap_threshold_ms: 100,
            smoothing_iterations: 5,
        }
    }
}
