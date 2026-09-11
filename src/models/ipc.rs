
// src/models/ipc.rs
// 进程间通信协议定义, 包括客户端命令和 daemon 主动发送的状态消息.

use std::{path::PathBuf, time::Duration};

use super::{PlayerState, playlist_manager::PlaylistChangeMessage};
use serde::{Deserialize, Serialize};


/// GUI, CLI 和 daemon 共享的客户端消息.
///
/// serde 使用 name 标识命令, 使用 arguments 保存命令参数. 所有连接都使用
/// 相同结构, 避免客户端和服务端分别维护字符串参数解析规则.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "name", content = "arguments", rename_all = "kebab-case")]
pub(crate) enum ClientMessage {

    GetPlayerState,   // 请求播放器状态快照
    GetPlaylist,      // 请求完整播放列表和当前播放索引快照

    Toggle,           // 切换播放和暂停.
    Seek(f64),        // 跳转到绝对播放位置, 单位为秒.
    SetVolume(f64),   // 设置播放器音量, 范围 0 到 100.
    Next,             // 播放下一首.
    Previous,         // 播放上一首.
    Play(usize, Duration, bool), // 从指定秒数播放 0-based 播放列表索引.

    // 播放列表变更
    PlaylistChange(PlaylistChangeMessage),

    /// 重新加载指定播放列表
    /// 传递要加载的播放列表文件路径,
    /// 可为 txt 文件, 每行一个媒体文件路径.
    /// 更新 Playlist, 并广播 UpdatePlaylist 给所有客户端.
    LoadPlaylist(PathBuf),

    /// 加载指定名称的 JSON 播放列表.
    SwitchPlaylist(String),

    /// 将当前播放列表保存为指定名称.
    SavePlaylist(String),

    Shutdown, // 请求 daemon 正常退出.
}

/// daemon 写回客户端的状态同步消息.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ServerMessage {

    // 播放器状态. 客户端根据 path 在本地读取媒体内容.
    UpdatePlayerState(PlayerState),

    // 增量播放列表变更广播.
    // state_number 表示 change 所基于的版本. 客户端仅在本地版本与其相等时
    // 应用 change, 成功后将本地版本递增; 不相等时请求完整播放列表快照.
    PlaylistChange(PlaylistChangeMessage),
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证重新加载播放列表命令保留文件路径.
    #[test]
    fn load_playlist_round_trip() {
        // 路径通过 serde 的 arguments 字段传输.
        let message = ClientMessage::LoadPlaylist(PathBuf::from("/tmp/list.txt"));
        let encoded = serde_json::to_string(&message).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&encoded).unwrap();

        assert_eq!(
            encoded,
            r#"{"name":"load-playlist","arguments":"/tmp/list.txt"}"#
        );
        assert!(matches!(
            decoded,
            ClientMessage::LoadPlaylist(path) if path == PathBuf::from("/tmp/list.txt")
        ));
    }
}
