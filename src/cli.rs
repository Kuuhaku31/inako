
// src/cli.rs
// CLI 命令执行逻辑模块

use crate::models::ipc::{ClientMessage};
use crate::models::playlist_manager::{PlaylistChangeKind, PlaylistChangeMessage};
use crate::models::{MediaContent, PlayerState};
use crate::utils::{format_time, get_lyric_current_line, read_media};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

mod request;
mod watch;

pub type CliArguments = (CliCommand, Option<PathBuf>);

/// 已解析的 CLI 命令. 参数验证和文本解析由二进制入口完成.
pub enum CliCommand {
    Status,
    Watch,
    Stop,
    Toggle,
    Seek(f64),
    Next,
    Previous,
    LoadPlaylist(PathBuf),
    SwitchPlaylist(String),
    SavePlaylist(String),
    Add(PathBuf),
    Clear,
    Remove(usize),
    Move { from: usize, to: usize },
    Shuffle,
    Play { index: usize, position: Duration },
}

/// 执行已解析的 CLI 命令.
// pub fn execute(command: Command, socket_path: Option<PathBuf>) -> Result<(), String> {
pub fn execute(arg: CliArguments) -> Result<(), String> {
    let (command, socket_path) = arg;
    match command {
        CliCommand::Status => print_status(false, socket_path),
        CliCommand::Watch => watch(socket_path),
        CliCommand::Stop => send(ClientMessage::Shutdown, socket_path),
        CliCommand::Toggle => send(ClientMessage::Toggle, socket_path),
        CliCommand::Seek(seconds) => send(ClientMessage::Seek(seconds), socket_path),
        CliCommand::Next => send(ClientMessage::Next, socket_path),
        CliCommand::Previous => send(ClientMessage::Previous, socket_path),
        CliCommand::LoadPlaylist(path) => send(ClientMessage::LoadPlaylist(path), socket_path),
        CliCommand::SwitchPlaylist(name) => send(ClientMessage::SwitchPlaylist(name), socket_path),
        CliCommand::SavePlaylist(name) => send(ClientMessage::SavePlaylist(name), socket_path),
        CliCommand::Add(path) => edit_playlist(socket_path, |len, _| {
            Ok(PlaylistChangeKind::Add(vec![(len, path)]))
        }),
        CliCommand::Clear => edit_playlist(socket_path, |_, playlist| {
            Ok(PlaylistChangeKind::Delete((0..playlist.len()).collect()))
        }),
        CliCommand::Remove(index) => edit_playlist(socket_path, |_, _| {
            Ok(PlaylistChangeKind::Delete(vec![index]))
        }),
        CliCommand::Move { from, to } => edit_playlist(socket_path, |_, _| {
            Ok(PlaylistChangeKind::Move(vec![(from, to)]))
        }),
        CliCommand::Shuffle => edit_playlist(socket_path, |_, playlist| {
            let mut order: Vec<usize> = (0..playlist.len()).collect();
            let seed = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("无法获取随机种子: {error}"))?
                .as_nanos() as usize;
            let mut state = seed;
            for index in (1..order.len()).rev() {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let target = state % (index + 1);
                order.swap(index, target);
            }
            Ok(PlaylistChangeKind::Move(
                order
                    .into_iter()
                    .enumerate()
                    .map(|(target, source)| (source, target))
                    .collect(),
            ))
        }),
        CliCommand::Play { index, position } => send(
            ClientMessage::Play(
                index,
                position,
                true,
            ),
            socket_path,
        ),
    }
}

// ========================================================================== //

/// 查询一次播放器状态并输出纯文本或 Waybar JSON.
fn print_status(plain: bool, socket_path: Option<PathBuf>) -> Result<(), String> {
    let state = request::player_state(socket_path)?;
    let media = state
        .path
        .as_deref()
        .and_then(|path| read_media(path).ok())
        .unwrap_or_default();
    if plain {
        println!("{}", status_text(&state, &media));
    } else {
        println!("{}", waybar_json(&state, &media));
    }
    Ok(())
}

/// 通过订阅连接持续输出 daemon 主动推送的 Waybar 状态.
fn watch(socket_path: Option<PathBuf>) -> Result<(), String> {
    let mut media_content_buffer = MediaContent::default();
    let mut media_path = None;

    for state in watch::states(socket_path)? {
        if state.path != media_path {
            media_content_buffer = state
                .path
                .as_deref()
                .and_then(|path| read_media(path).ok())
                .unwrap_or_default();
            media_path = state.path.clone();
        }

        // 使用缓存的内容, 输出 Waybar JSON
        println!("{}", waybar_json(&state, &media_content_buffer));
    }

    // 如果迭代器结束, 输出未连接状态
    println!(
        "{}",
        json!({"text": "󰝛  Music", "tooltip": "Inako 守护进程未连接", "class": "stopped"})
    );
    Err("daemon 状态连接已关闭".to_string())
}

fn send(command: ClientMessage, socket_path: Option<PathBuf>) -> Result<(), String> {
    request::send(command, socket_path)
}

/// 获取最新播放列表版本后发送一个原子编辑操作.
fn edit_playlist(
    socket_path: Option<PathBuf>,
    build: impl FnOnce(usize, &crate::models::playlist_manager::Playlist) -> Result<PlaylistChangeKind, String>,
) -> Result<(), String> {
    let (playlist, _, state_number) = request::playlist(socket_path.clone())?;
    let change = build(playlist.len(), &playlist)?;
    send(
        ClientMessage::PlaylistChange(PlaylistChangeMessage {
            state_number,
            change,
        }),
        socket_path,
    )
}

/// 将播放器状态转换为 Waybar custom module JSON.
fn waybar_json(state: &PlayerState, media: &MediaContent) -> Value {
    let text = status_text(state, media);
    let tooltip = format!(
        "{}\n{}\n{} / {}",
        media.artist,
        media.album,
        format_time(state.position),
        format_time(state.duration)
    );
    let percentage = if state.duration > 0.0 {
        (state.position / state.duration * 100.0).clamp(0.0, 100.0) as u64
    } else {
        0
    };
    json!({
        "text": text,
        "tooltip": tooltip,
        "class": if state.paused { "paused" } else { "playing" },
        "percentage": percentage,
    })
}

/// 生成包含控制图标, 元数据, 进度和当前歌词的状态文本.
fn status_text(state: &PlayerState, media: &MediaContent) -> String {
    let icon = if state.paused { "▶" } else { "⏸" };
    let artist = if media.artist.is_empty() {
        String::new()
    } else {
        format!("{} - ", media.artist)
    };

    let mut lyric = None;
    if let Some(lyrics) = &media.lyrics {
        if let Some(line) = get_lyric_current_line(lyrics, &state.position) {
            lyric = Some(format!("  ·  {line}"));
        }
    }

    let lyric = lyric.unwrap_or_default();
    format!(
        "⏮  {icon}  {artist}{}  {} / {}{lyric}  ⏭",
        media.title,
        format_time(state.position),
        format_time(state.duration)
    )
}
