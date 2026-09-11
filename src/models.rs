
// src/models.rs
// 数据模型定义.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
#[cfg(feature = "gui")]
use serde_json::Value;

pub(crate) mod ipc;
#[cfg(any(
    feature = "daemon",
    feature = "media",
    feature = "gui",
    test
))]
pub(crate) mod playlist_manager;

#[cfg(feature = "media")]
pub type Lyrics = Vec<LyricLine>;
#[cfg(feature = "media")]
pub type CoverData = Vec<u8>;
#[cfg(feature = "media")]
pub type Timestamp = f64;


#[cfg(feature = "media")]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct LyricLine {

    pub at: Timestamp, // 歌词时间戳, 单位秒
    pub text: String,
}

/// 媒体文件内容
#[cfg(feature = "media")]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct MediaContent {

    pub title: String, // 标题
    pub artist: String, // 艺术家
    pub album: String, // 专辑

    pub lyrics: Option<Lyrics>,
    pub cover: Option<CoverData>,

}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct PlayerState {

    pub path: Option<PathBuf>, // 当前播放文件路径, None 表示空闲

    pub paused: bool,  // 是否暂停
    pub position: f64, // 当前播放位置, 单位秒
    pub duration: f64, // 当前播放文件总时长, 单位秒
    pub volume: f64,   // 当前播放器音量, 范围 0 到 100.

}

/// 保存的窗口几何信息: 左上角坐标和宽高, 单位像素.
#[cfg(feature = "gui")]
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
pub(crate) struct WindowGeometry {
    pub(crate) x: i16,
    pub(crate) y: i16,
    pub(crate) width: i16,
    pub(crate) height: i16,
}

#[cfg(feature = "gui")]
impl WindowGeometry {

    pub(crate) fn from_vector(vec_at: &Vec<Value>, vec_size: &Vec<Value>) -> Option<Self> {
        Some(Self {
            x: vec_at[0].as_i64()? as i16,
            y: vec_at[1].as_i64()? as i16,
            width: vec_size[0].as_i64()? as i16,
            height: vec_size[1].as_i64()? as i16,
        })
    }

    pub(crate) fn width_i32(&self) -> i32 { self.width as i32 }
    pub(crate) fn height_i32(&self) -> i32 { self.height as i32 }
}
