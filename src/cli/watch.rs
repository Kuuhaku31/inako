
// src/cli/watch.rs
// Waybar 专用的持久状态连接模块

use crate::models::ipc::{ClientMessage, ServerMessage};
use crate::models::PlayerState;
use crate::utils;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;


/// 建立 Waybar 专用的持久状态连接.
///
/// 返回的迭代器直接阻塞读取 daemon 推送, 不执行定时状态查询.
/// 播放列表消息会被消费但不输出, 只有播放器状态事件会转换为 PlayerState.
pub(super) fn states(socket_path: Option<PathBuf>)
    -> Result<impl Iterator<Item = PlayerState>, String>
{

    // 连接到 daemon 并发送首次状态快照请求.
    let path = utils::get_socket_path(socket_path);
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("无法连接 Inako 守护进程: {error}"))?;
    let message = ClientMessage::GetPlayerState;
    writeln!(stream, "{}",
        serde_json::to_string(&message).map_err(|error| format!("无法编码 daemon 状态请求: {error}"))?
    )
    .and_then(|_| stream.flush())
    .map_err(|error| format!("发送 daemon 状态请求失败: {error}"))?;

    // 读取 daemon 推送的状态事件, 仅输出播放器状态.
    Ok(BufReader::new(stream).lines().filter_map(|line| {

        let message = serde_json::from_str::<ServerMessage>(&line.ok()?).ok()?;
        match message {
            ServerMessage::UpdatePlayerState(state) => Some(state),
            ServerMessage::PlaylistChange { .. } => None, // 无视
        }
    }))
}
