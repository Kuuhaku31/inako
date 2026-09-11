
// src/gui/daemon_connection.rs
// GUI 进程与 daemon 之间的长连接管理: 建立 socket, 收发线程和状态订阅.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::models::PlayerState;
use crate::models::ipc::{ClientMessage, ServerMessage};
use crate::models::playlist_manager::{PlaylistChangeKind, PlaylistChangeMessage};

/// 分别合并播放器状态和播放列表事件的共享槽位.
///
/// 播放器状态每类事件最多保留最新一条, 防止 GTK 主线程短暂繁忙时积压
/// 无意义的中间状态. 播放列表增量修改必须保序传递, 用队列保存, 不能
/// 像播放器状态那样合并为最新一条.
struct StateSlot {
    value: Mutex<StateSlotValue>,
    changed: Condvar,
}

/// 状态槽位中受 Mutex 保护的数据.
struct StateSlotValue {

    // 尚未被 GUI 消费的最新播放器状态
    latest_player: Option<PlayerState>,

    // 尚未被 GUI 消费的播放列表消息. 快照和增量必须共用一个有序队列,
    // 保留 daemon 写入 socket 时的顺序.
    playlist_updates: VecDeque<PlaylistChangeMessage>,

    // socket 读取结束后置为 true.
    closed: bool,
}

/// GUI 后台状态线程使用的阻塞迭代器.
///
/// 只在 window_state 中被消费, 但因为是 DaemonConnection::states 的返回
/// 类型, 至少需要和该方法一样的可见性 (pub(super)).
pub(super) struct StateSubscription {
    slot: Arc<StateSlot>,
}

impl Iterator for StateSubscription {

    type Item = (Option<PlayerState>, Option<PlaylistChangeMessage>);

    /// 按 socket 到达顺序取出播放列表消息, 再取出播放器事件.
    ///
    /// 没有待处理事件时阻塞等待. 连接关闭且槽位清空后返回 None.
    fn next(&mut self) -> Option<Self::Item> {
        let mut value = self.slot.value.lock().ok()?;
        loop {
            if let Some(update) = value.playlist_updates.pop_front() {
                return Some((None, Some(update)));
            }
            if let Some(state) = value.latest_player.take() {
                return Some((Some(state), None));
            }
            if value.closed { return None; }
            value = self.slot.changed.wait(value).ok()?;
        }
    }
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

/// GUI 进程唯一的 daemon 长连接句柄.
///
/// clone 只复制有界命令通道和状态槽位. socket 始终由一个写线程和一个读
/// 线程管理, GTK 回调不会等待 daemon 响应.
#[derive(Clone)]
pub(super) struct DaemonConnection {
    /// GTK 回调向 IPC 写线程提交命令的有界通道.
    commands: mpsc::SyncSender<ClientMessage>,

    /// 读取线程与 GUI 状态线程共享的最新状态槽位.
    state: Arc<StateSlot>,

}

impl DaemonConnection {

    /// 建立连接, 启动独立读写线程并注册状态订阅.
    pub(super) fn connect(socket_path: &Path) -> Result<Self, String> {

        // 连接到 daemon 并创建读写线程和状态槽位
        let stream = UnixStream::connect(socket_path)
            .map_err(|error| format!("无法连接 Inako 守护进程: {error}"))?;
        // 输出连接建立事件, 与后续收发日志共同标识窗口 IPC 生命周期.
        eprintln!("[window ipc] connected to daemon");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| format!("无法设置 daemon IPC 写入超时: {error}"))?;
        let reader = stream
            .try_clone()
            .map(BufReader::new)
            .map_err(|error| format!("无法创建 daemon IPC 读取流: {error}"))?;
        let state = Arc::new(StateSlot {
            value: Mutex::new(StateSlotValue {
                latest_player: None,
                playlist_updates: VecDeque::new(),
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let (commands, command_receiver) = mpsc::sync_channel(32);
        let reader_state = state.clone();
        thread::spawn(move || Self::read_messages(reader, reader_state));
        thread::spawn(move || Self::write_commands(stream, command_receiver));

        let connection = Self {
            commands,
            state,
        };
        connection.send(ClientMessage::GetPlayerState);
        connection.send(ClientMessage::GetPlaylist);
        Ok(connection)
    }

    /// 非阻塞地将命令放入 GUI IPC 队列.
    ///
    /// 队列已满或写线程退出时立即将错误输出到标准错误, 不阻塞 GTK 主线程.
    pub(super) fn send(&self, command: ClientMessage) {
        if let Err(error) = self.commands.try_send(command) {
            let message = match error {
                mpsc::TrySendError::Full(_) => "GUI daemon 命令队列已满".to_string(),
                mpsc::TrySendError::Disconnected(_) => "GUI daemon 连接已关闭".to_string(),
            };
            eprintln!("{message}");
        }
    }

    /// 返回共享最新状态槽位的阻塞迭代器.
    pub(super) fn states(&self) -> StateSubscription {
        StateSubscription {
            slot: self.state.clone(),
        }
    }

    /// 持续读取 daemon 响应和状态事件.
    ///
    /// 播放器状态和完整播放列表快照分别写入对应的合并字段, 增量播放列表
    /// 修改追加到有序队列. daemon 拒绝 PlaylistChange 时会发送完整播放
    /// 列表快照. EOF 会关闭状态迭代器并唤醒等待线程.
    fn read_messages(mut reader: BufReader<UnixStream>, slot: Arc<StateSlot>) {
        let mut line = String::new();
        loop {
            line.clear(); // 读取一行 JSON 消息
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    // 记录窗口收到的原始 IPC 行, 便于检查 daemon 广播顺序.
                    eprintln!("[window ipc <- daemon] {}", line.trim());
                    match serde_json::from_str::<ServerMessage>(line.trim()) {
                    Ok(ServerMessage::UpdatePlayerState(s)) => {
                        if let Ok(mut slot_value) = slot.value.lock() {
                            slot_value.latest_player = Some(s);
                            slot.changed.notify_one();
                        }
                    }
                    Ok(ServerMessage::PlaylistChange(msg)) => {
                        if let Ok(mut slot_value) = slot.value.lock() {
                            // 完整快照覆盖此前尚未交给 GUI 的过期增量消息.
                            if matches!(&msg.change, PlaylistChangeKind::Rebuild(_, _)) {
                                slot_value.playlist_updates.clear();
                            }
                            slot_value.playlist_updates.push_back(msg);
                            slot.changed.notify_one();
                        }
                    }
                    Err(error) => eprintln!("无效的 daemon 消息: {error}"),
                    }
                }
            }
        }

        if let Ok(mut value) = slot.value.lock() {
            value.closed = true;
            slot.changed.notify_all();
        }
    }

    /// 串行编码并写入 GUI 请求.
    ///
    /// GUI 不同步等待命令结果，界面仅应用 daemon 的状态消息.
    fn write_commands(mut stream: UnixStream, commands: mpsc::Receiver<ClientMessage>) {
        for message in commands {
            let result = serde_json::to_string(&message)
                .map_err(std::io::Error::other)
                .and_then(|request| {
                    // 记录窗口写入 daemon 的原始 IPC 行, 与接收日志配对.
                    eprintln!("[window ipc -> daemon] {request}");
                    writeln!(stream, "{request}")
                })
                .and_then(|_| stream.flush());
            if let Err(error) = result {
                eprintln!("发送 GUI daemon 命令失败: {error}");
                break;
            }
        }
        let _ = stream.shutdown(Shutdown::Both);
    }
}
