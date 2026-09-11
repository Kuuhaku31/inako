
// src/daemon/socket.rs
// socket.rs 负责 daemon 的 Unix socket 监听和客户端连接管理, 以及 IPC 消息的收发.

use crate::daemon::{ClientConnections, Daemon, ServerEvent, SocketGuard};
use crate::models::ipc::{ClientMessage, ServerMessage};
use crate::utils;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use crate::models::playlist_manager::{PlaylistChangeKind, PlaylistChangeMessage};


/// 检查 socket 路径并拒绝启动第二个活动 daemon.
///
/// 只有确认现有 socket 无法连接时才删除遗留文件. UnixListener::bind 仍是
/// 并发启动时的最终单实例仲裁.
pub(super) fn prepare_socket_path(socket: Option<String>) -> Result<SocketGuard, String> {

    let path = socket
    .map(PathBuf::from)
    .unwrap_or_else(utils::default_socket_path);

    // 如果 socket 路径不存在, 直接返回 Ok
    let Ok(metadata) = fs::symlink_metadata(&path) else { return Ok(SocketGuard{path}); };

    // 如果存在, 检查它是否是 socket 文件
    if !metadata.file_type().is_socket() {
        return Err(format!(
            "daemon socket 路径已存在且不是 socket: {}",
            path.display()
        ));
    }

    // 尝试连接现有 socket, 如果成功则拒绝启动第二个 daemon
    match UnixStream::connect(&path) {
        Ok(_) => Err(format!(
            "Inako 守护进程已在运行: {}",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            match fs::remove_file(&path) {
                Ok(()) => Ok(SocketGuard{path}),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SocketGuard{path}),
                Err(error) => Err(format!("无法清理失效 daemon socket: {error}")),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SocketGuard{path}),
        Err(error) => Err(format!(
            "无法检查现有 daemon socket {}: {error}",
            path.display()
        )),
    }
}

/// 创建 socket 监听器, 并在 daemon 生命周期内自动清理 socket 文件
///
/// 接受客户端并为每条连接创建独立读写线程.
pub(super) fn start_acceptor(socket_path: PathBuf)
    -> Result<(mpsc::Sender<ServerEvent>, mpsc::Receiver<ServerEvent>), String>
{
    if let Some(parent) = socket_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建 daemon socket 目录: {error}"))?;
    }
    let listener: UnixListener = UnixListener::bind(&socket_path).map_err(|error| format!("无法启动 Inako 守护进程: {error}"))?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).map_err(|error| format!("无法设置 daemon socket 权限: {error}"))?;

    let (s, r) = mpsc::channel();  // 创建事件通道并启动连接接收线程
    let event_sender = s.clone();  // mpsc::Sender 可以被克隆, 多个线程可以向同一个接收端发送事件

    // 启动后台线程, 监听 socket 并为每条连接创建独立读写线程
    thread::spawn(move || {
        let mut next_client_id = 1_u64;
        for stream in listener.incoming() {

            let Ok(stream  ) = stream             else { continue; };
            let Ok(writer  ) = stream.try_clone() else { continue; };
            let Ok(shutdown) = stream.try_clone() else { continue; };
            let _ = writer.set_write_timeout(Some(Duration::from_secs(2))); // 设置写超时, 避免慢客户端阻塞 daemon 主线程
            let (outbound, messages) = mpsc::sync_channel(32);

            // 通知 daemon 主线程有新客户端连接
            if event_sender
            .send(ServerEvent::ClientConnected { client_id: next_client_id, outbound, shutdown })
            .is_err() { return; }

            // 为每条连接创建独立读写线程, 读线程阻塞等待客户端请求, 写线程阻塞等待 daemon 消息.
            let w = event_sender.clone();
            let cid = next_client_id;
            let r = event_sender.clone();
            thread::spawn(move || write_client(cid, writer, messages, w));
            thread::spawn(move || read_client (cid, stream, r));

            // 准备下一个客户端 ID
            next_client_id = next_client_id.wrapping_add(1);
        }
    });

    Ok((s, r))
}


impl Daemon {

    /// 解析和执行一个客户端请求.
    ///
    /// GetPlayerState 和 GetPlaylist 只回复请求客户端, Shutdown 标记主循环退出.
    /// 播放器命令执行后统一比较状态并广播实际变化.
    pub(super) fn handle_request(&mut self, client_id: u64, line: &str) -> Result<(), String> {

        // 记录 daemon 收到的原始 IPC 行, 便于关联客户端请求与后续广播.
        eprintln!("[daemon ipc <- client {client_id}] {line}");
        let message = match serde_json::from_str::<ClientMessage>(line) {
            Ok(message) => message,
            Err(error) => { return Err(format!("无效的 daemon 请求: {error}")); }
        };

        // 请求类型只在此处匹配一次, 各分支负责自己的回复或状态同步.
        match message {
            ClientMessage::Shutdown          => {
                let this = &mut *self;
                println!("daemon 正在退出, 停止播放器并断开全部客户端连接...");
                this.running = false;
                Ok(())
            },

            ClientMessage::GetPlayerState    => {
                let this = &mut *self;
                let s = this.player.get_player_state();
                write_message(&mut this.connections, client_id, ServerMessage::UpdatePlayerState(s));
                Ok(())
            },
            ClientMessage::GetPlaylist       => {
                let this = &mut *self;
                let event = {
                    let this = &this;
                    let msg = this.playlist_manager.get_reload_message();
                    ServerMessage::PlaylistChange(msg)
                };
                write_message(&mut this.connections, client_id, event);
                Ok(())
            },
            ClientMessage::Toggle            => self.player.toggle(),
            ClientMessage::Seek(seconds)     => self.player.seek(seconds),
            ClientMessage::SetVolume(volume) => self.player.set_volume(volume),

            ClientMessage::PlaylistChange(change_message) => {

                // 新增媒体路径必须在 daemon 侧规范化, 保证所有客户端共享有效路径.
                let change_message = match change_message.change {
                    PlaylistChangeKind::Add(items) => {
                        let items = items.into_iter().map(|(index, path)| {
                            crate::utils::canonical_media_path(&path)
                                .map(|path| (index, path))
                        }).collect::<Result<Vec<_>, _>>()?;
                        PlaylistChangeMessage {
                            state_number: change_message.state_number,
                            change: PlaylistChangeKind::Add(items),
                        }
                    }
                    change => PlaylistChangeMessage {
                        state_number: change_message.state_number,
                        change,
                    },
                };
                // 版本冲突时返回完整快照, 让客户端可以恢复后重试.
                let msg = match self.playlist_manager.apply_playlist_change(change_message) {
                    Ok(message) => message,
                    Err(error) => {
                        let snapshot = self.playlist_manager.get_reload_message();
                        write_message(
                            &mut self.connections,
                            client_id,
                            ServerMessage::PlaylistChange(snapshot),
                        );
                        return Err(error);
                    }
                };
                // 广播播放列表变化 
                self.broadcast_message(ServerMessage::PlaylistChange(msg))
            },

            ClientMessage::Next => {
                let change = self.playlist_manager.select_next()?; // 尝试更新列表
                // 切换索引后立即加载对应媒体并广播新选择.
                self.play_and_broadcast_selection(change)
            }
            ClientMessage::Previous => {
                let change = self.playlist_manager.select_previous()?; // 尝试更新列表
                // 切换索引后立即加载对应媒体并广播新选择.
                self.play_and_broadcast_selection(change)
            }
            ClientMessage::Play(index, position, is_resume) => {

                // 尝试选择新的播放索引
                let msg = self.playlist_manager.select(index)?;
                // 广播播放列表变化
                self.broadcast_message(ServerMessage::PlaylistChange(msg))?;

                let path = self.playlist_manager.get_path_by_index(index)?;

                // 播放
                self.player.play(&path, Some(position), is_resume)
            }

            // 重新加载播放列表
            ClientMessage::LoadPlaylist(path) => {

                self.player.stop()?;
                let playlist = super::get_playlist_by_txt_file(&path)?;
                let msg = self.playlist_manager.reload(playlist, None)?;
                self.broadcast_message(ServerMessage::PlaylistChange(msg)) // 广播播放列表变化
            }

            ClientMessage::SwitchPlaylist(name) => {
                let path = super::playlist_file_path(&name)?;
                let playlist = super::get_playlist_by_json_file(&path)?;
                self.player.stop()?;
                let msg = self.playlist_manager.reload(playlist, None)?;
                self.playlist_name = Some(name);
                self.broadcast_message(ServerMessage::PlaylistChange(msg))
            }

            ClientMessage::SavePlaylist(name) => {
                let path = super::playlist_file_path(&name)?;
                super::write_playlist_to_json_file(&path, self.playlist_manager.get_playlist_ref())?;
                self.playlist_name = Some(name);
                Ok(())
            }
        }
    }

    /// 向全部客户端提交消息, 队列满时关闭并移除慢客户端.
    pub(super) fn broadcast_message(&mut self, event: ServerMessage) -> Result<(), String> {
        self.connections.retain(|_, connection| {
            let keep = connection.outbound.try_send(event.clone()).is_ok();
            if !keep {
                let _ = connection.shutdown.shutdown(Shutdown::Both);
            }
            keep
        });
        Ok(())
    }
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

/// 从有界队列读取消息并串行写入一个客户端.
fn write_client(
    cid: u64,
mut stream: UnixStream,
    messages: mpsc::Receiver<ServerMessage>,
    event_sender: mpsc::Sender<ServerEvent>,
) {
    // 读取消息并写入客户端, 队列满时退出
    for message in messages {
        if write_to(cid, &mut stream, &message).is_err() { break; }
    }

    // 关闭客户端连接并通知 daemon 主线程
    let _ = stream.shutdown(Shutdown::Both); // 这行会触发 read_client 退出, 但 read_client 可能已经在等待下一条消息, 所以这里也要发送 ClientDisconnected
    let _ = event_sender.send(ServerEvent::ClientDisconnected { client_id: cid });
}

/// 阻塞读取一个客户端的请求并转发给 daemon 主线程.
fn read_client(
    cid: u64,
    stream: UnixStream,
    event_sender: mpsc::Sender<ServerEvent>)
{
    // 阻塞读取客户端请求, 直到连接关闭或发生错误
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break; };
        if event_sender.send(ServerEvent::ClientRequest { client_id: cid, line })
        .is_err() { return; }
    }

    // 关闭客户端连接并通知 daemon 主线程
    let _ = event_sender.send(ServerEvent::ClientDisconnected { client_id: cid });
}


// ========================================================================== //

/// 向指定连接提交普通消息.
fn write_message(
    connections: &mut ClientConnections,
    client_id: u64,
    message: ServerMessage,
) {
    let failed = connections
        .get_mut(&client_id)
        .is_some_and(|connection| {
            connection
                .outbound
                .try_send(message)
                .is_err()
        });
    if failed {
        disconnect(connections, client_id);
    }
}

/// 从连接表删除客户端并关闭其 socket.
fn disconnect(connections: &mut ClientConnections, client_id: u64) {
    if let Some(connection) = connections.remove(&client_id) {
        let _ = connection.shutdown.shutdown(Shutdown::Both);
    }
}

/// 将一条 ServerMessage 编码为单行 JSON 并记录实际出站内容.
fn write_to(
    client_id: u64,
    writer: &mut UnixStream,
    message: &ServerMessage,
) -> Result<(), String> {
    let message = serde_json::to_string(message)
        .map_err(|error| format!("无法编码 daemon 消息: {error}"))?;
    // 记录 daemon 写入此客户端的原始 IPC 行, 包括高频播放器状态广播.
    eprintln!("[daemon ipc -> client {client_id}] {message}");
    writeln!(writer, "{message}")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("写入 daemon 消息失败: {error}"))
}
