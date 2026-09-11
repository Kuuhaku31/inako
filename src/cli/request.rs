
// src/cli/request.rs
// 发送短连接请求的模块

use crate::models::ipc::{ClientMessage, ServerMessage};
use crate::models::{PlayerState, playlist_manager::Playlist};
use crate::utils;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;


/// 使用短连接向 daemon 发送一条控制命令.
///
/// 客户端不等待执行结果; 写入 socket 成功后关闭连接. 后续状态由持久客户端
/// 通过 daemon 广播同步.
pub(super) fn send(command: ClientMessage, socket_path: Option<PathBuf>) -> Result<(), String> {

    // 连接到 daemon 并发送请求
    let path = utils::get_socket_path(socket_path);
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("无法连接 Inako 守护进程: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|error| format!("无法设置 daemon IPC 超时: {error}"))?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&command)
            .map_err(|error| format!("无法编码 daemon 请求: {error}"))?
    )
    .and_then(|_| stream.flush())
    .map_err(|error| format!("发送 daemon 请求失败: {error}"))
}

/// 通过 GetPlayerState 请求等待 daemon 返回播放器状态消息.
pub(super) fn player_state(socket_path: Option<PathBuf>) -> Result<PlayerState, String> {
    let path = utils::get_socket_path(socket_path);
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("无法连接 Inako 守护进程: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|error| format!("无法设置 daemon IPC 超时: {error}"))?;
    let message = ClientMessage::GetPlayerState;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&message)
            .map_err(|error| format!("无法编码 daemon 状态请求: {error}"))?
    )
    .and_then(|_| stream.flush())
    .map_err(|error| format!("发送 daemon 状态请求失败: {error}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .map_err(|error| format!("读取 daemon 状态失败: {error}"))?
            == 0
        {
            return Err("Inako 守护进程已关闭连接".to_string());
        }

        let message = serde_json::from_str::<ServerMessage>(line.trim())
            .map_err(|error| format!("无效的 daemon 状态消息: {error}"))?;
        if let ServerMessage::UpdatePlayerState(state) = message {
            return Ok(state);
        }

    }
}

/// 通过 GetPlaylist 请求播放列表快照, 用于构造带版本号的编辑命令.
pub(super) fn playlist(socket_path: Option<PathBuf>)
    -> Result<(Playlist, Option<usize>, usize), String>
{
    let path = utils::get_socket_path(socket_path);
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("无法连接 Inako 守护进程: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .and_then(|_| stream.set_write_timeout(Some(Duration::from_secs(2))))
        .map_err(|error| format!("无法设置 daemon IPC 超时: {error}"))?;
    let message = ClientMessage::GetPlaylist;
    writeln!(stream, "{}", serde_json::to_string(&message)
        .map_err(|error| format!("无法编码 daemon 播放列表请求: {error}"))?)
        .and_then(|_| stream.flush())
        .map_err(|error| format!("发送 daemon 播放列表请求失败: {error}"))?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)
            .map_err(|error| format!("读取 daemon 播放列表失败: {error}"))? == 0 {
            return Err("Inako 守护进程已关闭连接".to_string());
        }
        let ServerMessage::PlaylistChange(message) = serde_json::from_str(line.trim())
            .map_err(|error| format!("无效的 daemon 播放列表消息: {error}"))?
        else {
            continue;
        };
        if let crate::models::playlist_manager::PlaylistChangeKind::Rebuild(entries, current) = message.change {
            return Ok((entries, current, message.state_number));
        }
    }
}
