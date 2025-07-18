//! 负责处理应用的持久化配置。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::info;

use crate::providers::qq::device::Device;

/// 酷狗音乐的配置项。
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct KugouConfig {
    /// 缓存的酷狗音乐 DFID。
    pub dfid: String,
}

/// 获取应用配置目录下指定文件的完整路径。
///
/// # 参数
/// * `filename` - 目标配置文件的名称，例如 "kugou_config.json"。
pub(crate) fn get_config_file_path(filename: &str) -> Result<PathBuf, std::io::Error> {
    if let Some(mut config_dir) = dirs::config_dir() {
        config_dir.push("lyrics-helper");
        fs::create_dir_all(&config_dir)?;
        config_dir.push(filename);
        Ok(config_dir)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "无法找到用户配置目录",
        ))
    }
}

/// 从文件加载酷狗音乐的配置。
pub(crate) fn load_kugou_config() -> Result<KugouConfig, Box<dyn std::error::Error>> {
    let config_path = get_config_file_path("kugou_config.json")?;
    let content = fs::read_to_string(config_path)?;
    let config: KugouConfig = serde_json::from_str(&content)?;
    info!("已从缓存加载 DFID: {}", config.dfid);
    Ok(config)
}

/// 将酷狗音乐的配置实例序列化为 JSON 并保存到文件。
pub(crate) fn save_kugou_config(config: &KugouConfig) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = get_config_file_path("kugou_config.json")?;
    let content = serde_json::to_string_pretty(config)?;
    fs::write(config_path, content)?;
    info!("已将 DFID 保存到本地。");
    Ok(())
}

/// 从缓存加载或创建一个新的 QQ 音乐的 Device 实例。
pub(crate) fn load_qq_device() -> Result<Device, Box<dyn std::error::Error>> {
    let cache_path = get_config_file_path("qq_device.json")?;

    match fs::read_to_string(&cache_path) {
        Ok(content) => {
            let device: Device = serde_json::from_str(&content)?;
            info!("已从缓存加载 QQ Device。");
            Ok(device)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!("QQ Device 配置文件不存在，将创建并保存一个新设备。");
            let new_device = Device::new();
            save_qq_device(&new_device)?;
            Ok(new_device)
        }
        Err(e) => Err(e.into()),
    }
}

/// 将 QQ 音乐的 Device 实例保存到缓存文件。
pub(crate) fn save_qq_device(device: &Device) -> Result<(), Box<dyn std::error::Error>> {
    let cache_path = get_config_file_path("qq_device.json")?;
    let content = serde_json::to_string_pretty(device)?;
    fs::write(cache_path, content)?;
    info!("QQ Device 配置已保存。");
    Ok(())
}
