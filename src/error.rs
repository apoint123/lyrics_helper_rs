//! 定义了整个 `lyrics-helper` 库的错误类型 `LyricsHelperError`。

use std::{io, string::FromUtf8Error};
use thiserror::Error;

use crate::converter::types::ConvertError;

/// `lyrics-helper` 库的通用错误枚举。
#[derive(Error, Debug)]
pub enum LyricsHelperError {
    /// 通用的 anyhow 错误
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),

    /// 网络请求失败 (源自 `reqwest::Error`)
    #[error("网络请求失败: {0}")]
    Reqwest(#[from] reqwest::Error),

    /// 网络请求失败 (源自 `wreq::Error`)
    #[error("网络请求失败: {0}")]
    Wreq(#[from] wreq::Error),

    /// JSON 解析失败 (源自 `serde_json::Error`)
    #[error("JSON 解析失败: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// XML 解析失败 (源自 `quick_xml::Error`)
    #[error("XML 解析失败: {0}")]
    XmlParse(#[from] quick_xml::Error),

    /// Base64 解码失败 (源自 `base64::DecodeError`)
    #[error("Base64 解码失败: {0}")]
    Base64Decode(#[from] base64::DecodeError),

    /// UTF-8 转换失败 (源自 `string::FromUtf8Error`)
    #[error("UTF-8 转换失败: {0}")]
    FromUtf8(#[from] FromUtf8Error),

    /// 整数解析失败 (源自 `std::num::ParseIntError`)
    #[error("整数解析失败: {0}")]
    ParseInt(#[from] std::num::ParseIntError),

    /// I/O 错误 (源自 `io::Error`)
    #[error("I/O 错误: {0}")]
    Io(#[from] io::Error),

    /// 通用的歌词解析错误
    #[error("歌词解析失败: {0}")]
    Parser(String),

    /// 在数据源中找不到歌词内容
    #[error("在源中未找到歌词内容")]
    LyricNotFound,

    /// 不支持的歌词源提供商
    #[error("不支持的提供商: '{0}'")]
    ProviderNotSupported(String),

    /// API 返回错误或空数据
    #[error("API 为 `{0}` 返回了错误或空数据")]
    ApiError(String),

    /// 解密失败
    #[error("解密失败: {0}")]
    Decryption(String),

    /// 加密失败
    #[error("加密失败: {0}")]
    Encryption(String),

    /// 内部错误
    #[error("内部错误: {0}")]
    Internal(String),

    /// 更通用的网络层错误
    #[error("网络错误: {0}")]
    Network(String),

    /// API 请求被限流
    #[error("API 请求被限流: {0}")]
    RateLimited(String),
}

/// `LyricsHelperError` 的 `Result` 类型别名，方便在函数签名中使用。
pub type Result<T> = std::result::Result<T, LyricsHelperError>;

impl From<ConvertError> for LyricsHelperError {
    fn from(err: ConvertError) -> Self {
        match err {
            ConvertError::Xml(e) => Self::XmlParse(e),
            ConvertError::Attribute(e) => Self::XmlParse(e.into()),
            ConvertError::ParseInt(e) => Self::ParseInt(e),
            ConvertError::Base64Decode(e) => Self::Base64Decode(e),
            ConvertError::FromUtf8(e) => Self::FromUtf8(e),
            ConvertError::Io(e) => Self::Io(e),
            ConvertError::Encoding(e) => Self::Internal(format!("编码错误: {e}")),

            ConvertError::JsonParse { source, context } => {
                let error_message = format!("解析 JSON 内容 {context} 失败: {source}");
                Self::Parser(error_message)
            }

            ConvertError::InvalidTime(s)
            | ConvertError::InvalidJsonStructure(s)
            | ConvertError::InvalidLyricFormat(s) => Self::Parser(s),

            ConvertError::Format(e) => Self::Internal(e.to_string()),
            ConvertError::Internal(s) => Self::Internal(s),
        }
    }
}
