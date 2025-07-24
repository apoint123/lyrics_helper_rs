//! 定义了歌词转换中使用的核心数据类型。

use std::{collections::HashMap, fmt, io, path::PathBuf, str::FromStr};

use quick_xml::{
    Error as QuickXmlErrorMain, encoding::EncodingError,
    events::attributes::AttrError as QuickXmlAttrError,
};
use serde::{Deserialize, Serialize};
use strum_macros::EnumString;
use thiserror::Error;
use tracing::warn;

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
    Format(#[from] std::fmt::Error),
    /// 内部逻辑错误或未明确分类的错误。
    #[error("错误: {0}")]
    Internal(String),
    /// 文件读写等IO错误。
    #[error("IO 错误: {0}")]
    Io(#[from] io::Error),
    /// JSON 解析错误。
    #[error("解析 JSON 内容 {context} 失败: {source}")]
    JsonParse {
        /// 底层 serde_json 错误
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
}

impl ConvertError {
    /// 创建一个带有上下文的 JsonParse 错误。
    pub fn json_parse(source: serde_json::Error, context: String) -> Self {
        Self::JsonParse { source, context }
    }
}

/// 定义从字符串解析 `CanonicalMetadataKey` 时可能发生的错误。
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ParseCanonicalMetadataKeyError(String); // 存储无法解析的原始键字符串

impl std::fmt::Display for ParseCanonicalMetadataKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "未知或无效的元数据键: {}", self.0)
    }
}
impl std::error::Error for ParseCanonicalMetadataKeyError {}

//=============================================================================
// 2. 核心歌词格式枚举及相关
//=============================================================================

/// 枚举：表示支持的歌词格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Serialize, Deserialize)]
#[strum(ascii_case_insensitive)]
#[derive(Default)]
pub enum LyricFormat {
    /// Advanced SubStation Alpha 格式。
    Ass,
    /// Timed Text Markup Language 格式。
    #[default]
    Ttml,
    /// Apple Music JSON 格式 (内嵌TTML)。
    AppleMusicJson,
    /// Lyricify Syllable (*.lys)。
    Lys,
    /// 标准 LRC (LyRiCs) 格式。
    Lrc,
    /// 增强型 LRC (Enhanced LRC) 格式，支持逐字时间戳。
    EnhancedLrc,
    /// QQ Music QRC 格式。
    Qrc,
    /// NetEase YRC 格式。
    Yrc,
    /// Lyricify Lines (*.lyl)。
    Lyl,
    /// Salt Player Lyrics (*.spl)。
    Spl,
    /// Lyricify Quick Export (*.lqe)。
    Lqe,
    /// 酷狗 KRC 格式。
    Krc,
    /// Musixmatch JSON 格式。
    Musixmatch,
}

impl LyricFormat {
    /// 返回所有支持的歌词格式的列表。
    pub fn all() -> &'static [Self] {
        &[
            LyricFormat::Ass,
            LyricFormat::Ttml,
            LyricFormat::AppleMusicJson,
            LyricFormat::Lys,
            LyricFormat::Lrc,
            LyricFormat::EnhancedLrc,
            LyricFormat::Qrc,
            LyricFormat::Yrc,
            LyricFormat::Lyl,
            LyricFormat::Spl,
            LyricFormat::Lqe,
            LyricFormat::Krc,
            LyricFormat::Musixmatch,
        ]
    }

    /// 将歌词格式枚举转换为对应的文件扩展名字符串。
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
            LyricFormat::Musixmatch => "json",
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
            LyricFormat::Musixmatch => write!(f, "Musixmatch lyrics"),
        }
    }
}

//=============================================================================
// 3. 歌词内部表示结构
//=============================================================================

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

/// 表示单个翻译及其语言的结构体。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationEntry {
    /// 翻译的文本内容。
    pub text: String,
    /// 翻译的语言代码，可选。
    /// 建议遵循 BCP 47 标准 (例如 "en", "ja", "zh-Hans")。
    pub lang: Option<String>,
}

/// 表示单个罗马音及其语言/方案的结构体。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct RomanizationEntry {
    /// 罗马音的文本内容。
    pub text: String,
    /// 目标音译的语言和脚本代码，可选。
    /// 例如 "ja-Latn" (日语罗马字), "ko-Latn" (韩语罗马字)。
    pub lang: Option<String>,
    /// 可选的特定罗马音方案名称。
    /// 例如 "Hepburn" (平文式罗马字), "Nihon-shiki" (日本式罗马字), "RevisedRomanization" (韩语罗马字修正案)。
    pub scheme: Option<String>,
}

/// 表示歌词行中的背景歌词部分。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundSection {
    /// 背景歌词的开始时间（毫秒）。
    pub start_ms: u64,
    /// 背景歌词的结束时间（毫秒）。
    pub end_ms: u64,
    /// 背景歌词的音节列表。
    pub syllables: Vec<LyricSyllable>,
    /// 背景歌词的翻译。
    pub translations: Vec<TranslationEntry>,
    /// 背景歌词的罗马音。
    pub romanizations: Vec<RomanizationEntry>,
}

/// 通用的歌词行结构，作为项目内部处理歌词数据的主要表示。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    /// 行的开始时间，相对于歌曲开始的绝对时间（毫秒）。
    pub start_ms: u64,
    /// 行的结束时间，相对于歌曲开始的绝对时间（毫秒）。
    pub end_ms: u64,
    /// 可选的整行文本内容。
    /// 主要用于纯逐行歌词格式（如标准LRC）。
    pub line_text: Option<String>,
    /// 主歌词的音节列表。
    pub main_syllables: Vec<LyricSyllable>,
    /// 该行的翻译列表。
    pub translations: Vec<TranslationEntry>,
    /// 该行的罗马音列表。
    pub romanizations: Vec<RomanizationEntry>,
    /// 可选的演唱者标识。
    /// 通常情况下应确保至少有一个 `v1`。
    pub agent: Option<String>,
    /// 可选的背景歌词部分。
    pub background_section: Option<BackgroundSection>,
    /// 可选的歌曲组成部分标记。
    /// 例如 "verse", "chorus", "bridge"。
    pub song_part: Option<String>,
    /// 可选的 iTunes Key (如 "L1", "L2")。
    pub itunes_key: Option<String>,
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
    /// TTML歌词贡献者的GitHubID。
    TtmlAuthorGithub,
    /// TTML歌词贡献者的GitHub登录名。
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
            "title" | "musicname" => Ok(Self::Title),
            "artist" | "artists" => Ok(Self::Artist),
            "album" => Ok(Self::Album),
            "language" | "lang" => Ok(Self::Language),
            "offset" => Ok(Self::Offset),
            "songwriter" | "songwriters" => Ok(Self::Songwriter),
            "ncmmusicid" => Ok(Self::NcmMusicId),
            "qqmusicid" => Ok(Self::QqMusicId),
            "spotifyid" => Ok(Self::SpotifyId),
            "applemusicid" => Ok(Self::AppleMusicId),
            "isrc" => Ok(Self::Isrc),
            "ttmlauthorgithub" => Ok(Self::TtmlAuthorGithub),
            "ttmlauthorgithublogin" => Ok(Self::TtmlAuthorGithubLogin),
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
}

//=============================================================================
// 6. 辅助类型与函数
//=============================================================================

/// 表示从ASS中提取的标记信息。
/// 元组的第一个元素是原始行号，第二个元素是标记文本。
pub type MarkerInfo = (usize, String);

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
    // /// 可选的用户指定的元数据覆盖（原始键值对）。
    // pub user_metadata_overrides: Option<HashMap<String, Vec<String>>>,
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

/// 统一管理所有格式的转换选项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConversionOptions {
    /// TTML 转换选项
    pub ttml: TtmlGenerationOptions,
    /// LQE 转换选项
    #[serde(default)]
    pub lqe: LqeGenerationOptions,
    /// ASS 转换选项
    pub ass: AssGenerationOptions,
    /// LRC 转换选项
    #[serde(default)]
    pub lrc: LrcGenerationOptions,
    /// 元数据移除选项
    pub metadata_stripper: MetadataStripperOptions,
    /// 简繁转换选项
    #[serde(default)]
    pub chinese_conversion: ChineseConversionOptions,
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

/// 配置元数据行清理器的选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataStripperOptions {
    /// 是否启用此清理功能。
    pub enabled: bool,
    /// 用于匹配头部/尾部块的关键词列表。
    /// 如果为 `None`，将使用一组内建的默认关键词。
    pub keywords: Option<Vec<String>>,
    /// 关键词匹配是否区分大小写。
    pub keyword_case_sensitive: bool,
    /// 是否启用基于正则表达式的行移除。
    pub enable_regex_stripping: bool,
    /// 用于匹配并移除任意行的正则表达式列表。
    /// 如果为 `None`，将使用一组内建的默认正则表达式。
    pub regex_patterns: Option<Vec<String>>,
    /// 正则表达式匹配是否区分大小写。
    pub regex_case_sensitive: bool,
}

impl Default for MetadataStripperOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            keywords: None,
            keyword_case_sensitive: false,
            enable_regex_stripping: true,
            regex_patterns: None,
            regex_case_sensitive: false,
        }
    }
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

/// 简繁转换的配置选项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ChineseConversionOptions {
    /// 指定要使用的 ferrous-opencc 配置文件名，例如 "s2t.json" 或 "s2hk.json"。
    /// 当值为 `Some(string)` 且不为空时，功能启用。
    pub config_name: Option<String>,

    /// 为翻译指定 BCP 47 语言标签，例如 "zh-Hant" 或 "zh-Hant-HK"。
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
