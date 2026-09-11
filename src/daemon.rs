
// src/daemon.rs
// 通过 Unix socket 接收客户端请求并控制内嵌 libmpv 播放器.

mod mpv_player;
mod socket;

use serde::{Deserialize, Serialize};

use crate::daemon::mpv_player::EmbeddedPlayer;
use crate::models::playlist_manager::{PlaylistManager, Playlist, PlaylistChangeMessage};
use crate::models::ipc::ServerMessage;
use crate::utils;
use std::collections::HashMap;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;


/// 启动事件驱动的 daemon socket 服务.
///
/// daemon 状态仅在此线程访问. socket 和 libmpv 线程只提交具体事件,
/// 每个事件都会立即唤醒主循环.
pub fn run_socket(socket: Option<String>) -> Result<(), String> {

    // 实例化 daemon, 读取播放列表状态文件, 并创建唯一的内嵌 libmpv 播放器.
    let mut daemon = Daemon::load(socket)?;
    println!("Inako 守护进程已启动, socket: {}", daemon.get_socket_path().display());

    // 从事件通道取得下一条 socket 或播放器事件.
    fn get(daemon: &mut Daemon) -> Result<ServerEvent, String> {
        daemon.event_receiver.recv()
        .map_err(|error| format!("daemon 事件通道已关闭: {error}"))
    }

    // 事件循环, 处理 socket 和 libmpv 播放器事件.
    while daemon.running { match get(&mut daemon)?
    {
        ServerEvent::ClientConnected       { client_id, outbound, shutdown }
            => daemon.handle_connected     ( client_id, outbound, shutdown ),
        ServerEvent::ClientDisconnected    { client_id }
            => daemon.handle_disconnected  ( client_id ),
        ServerEvent::ClientRequest         { client_id, line }
            => daemon.handle_request_event ( client_id, line),
        ServerEvent::PropertyChanged
            => daemon.handle_property_changed(),
        ServerEvent::PlaybackEnded
            => daemon.handle_playback_ended(),
    }}

    // 退出前读取 libmpv 实际播放位置并写回播放列表状态.
    // daemon.persist_playback_position().unwrap_or_else(|error| eprintln!("保存播放进度失败: {error}"));

    let config_file_path = utils::state_dir().join("daemon_snapshot.json");

    // 获取要保存的信息
    let playback_position = daemon.player.playback_position();
    let volume = Some(daemon.player.volume());
    let playlist_name = daemon.playlist_name;
    let current = daemon.playlist_manager.get_current();
    let snapshot = DaemonSnapshot {
        playback_position,
        volume,
        playlist_name: playlist_name.clone(),
        current,
    };
    write_daemon_snapshot_to_json_file(&config_file_path, &snapshot)
    .unwrap_or_else(|error| eprintln!("保存播放器管理器快照失败: {error}"));

    // 如果存在播放列表名称, 则保存播放列表到对应的 JSON 文件.
    if let Some(name) = playlist_name {
        let playlist_file_path = playlist_file_path(&name)?;
        let playlist = daemon.playlist_manager.get_playlist_ref();
        write_playlist_to_json_file(&playlist_file_path, &playlist)
        .unwrap_or_else(|error| eprintln!("保存播放列表失败: {error}"));
    }


    // 退出前直接读取 libmpv 实际音量, 不在 daemon 运行状态中保存音量字段.
    // DaemonConfig::save_volume(daemon.player.volume()).unwrap_or_else(|error| eprintln!("保存播放器音量失败: {error}"));

    println!("Inako 守护进程已退出.");
    Ok(())
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

#[derive(Default, Deserialize, Serialize)]
struct DaemonSnapshot {

    playback_position: Option<Duration>, // 当前播放项播放位置, 单位为秒
    volume: Option<f64>, // 当前播放器音量
    playlist_name: Option<String>, // 播放列表名称
    current: Option<usize>, // 当前播放项索引
}

/// Daemon 是播放列表及内嵌 libmpv 播放器状态的唯一持有者.
pub(crate)  struct Daemon {

    running: bool, // 用于退出 daemon.

    playlist_name: Option<String>, // 播放列表名称
    playlist_manager: PlaylistManager, // 播放列表


    player: EmbeddedPlayer,        // 唯一的内嵌 libmpv 播放器.

    connections: ClientConnections, // 已连接客户端及其订阅状态.
    event_receiver: mpsc::Receiver<ServerEvent>, // socket 和播放器事件入口.

    socket_guard: SocketGuard, // daemon 退出时删除 socket 文件.
}

impl Daemon {

    /// 加载播放列表状态并创建唯一的内嵌 libmpv 核心.
    fn load(socket: Option<String>) -> Result<Self, String> {

        println!("加载 daemon 配置...");

        let config_dir = utils::state_dir();
        let snapshot = get_player_manager_snapshot_by_json_file(&config_dir.join("daemon_snapshot.json")).unwrap_or_default();
        let playlist_name = snapshot.playlist_name;
        let snapshot_current = snapshot.current;

        // 初始化 PlaylistManager
        let mut playlist_manager = PlaylistManager::new();

        // 加载播放列表
        if let Some(name) = &playlist_name {

            let path = config_dir.join("playlists").join(format!("{}.json", name));
            println!("尝试加载播放列表文件: {}", path.display());

            // 尝试加载指定名称的播放列表文件
            match get_playlist_by_json_file(&path) {
                Ok(playlist) => {
                    // 如果返回错误, 打印输出
                    if let Err(e) = playlist_manager.reload(playlist, snapshot_current) {
                        eprintln!("加载播放列表失败: {e}");
                    } else {
                        println!("成功加载播放列表: {}", name);
                    }
                }
                Err(e) => {
                    println!("播放列表文件不存在: {}, 错误: {}", path.display(), e);
                }
            }
        } else {
            println!("未指定播放列表名称, 将使用空播放列表.");
        }

        // 准备 socket 并启动接收器
        let socket = socket::prepare_socket_path(socket)?;
        let (s, r) = socket::start_acceptor(socket.path.clone())?;

        // 初始化嵌入式播放器
        let player = EmbeddedPlayer::new(s);


        // 恢复音量
        if let Some(volume) = snapshot.volume {
            // 如果返回错误, 打印输出
            if let Err(e) = player.set_volume(volume) {
                eprintln!("设置音量失败: {e}");
            }
        }

        let num = playlist_manager.get_state_number();

        // 构建 Daemon 实例
        let daemon = Self {
            running        : true,
            playlist_manager,
            playlist_name,
            player,
            connections    : ClientConnections::new(),
            socket_guard   : socket,
            event_receiver : r,
        };

        // 自动加载上次选中的歌曲
        if let Some(path) = &daemon.playlist_manager.get_current_path()
        && let position = snapshot.playback_position {
            if let Err(error) = daemon.player.play(path, position, false)
            { eprintln!("恢复上次播放失败: {error}"); }
        }

        // 打印 playlist 状态号
        println!("当前播放列表状态号: {}", num);

        Ok(daemon)
    }

    fn get_socket_path(&self) -> &PathBuf { &self.socket_guard.path }

    /// 将 daemon 当前选中的播放列表项交给内嵌 libmpv 播放.
    fn play_current(&self) -> Result<(), String> {
        if let Some(path) = &self.playlist_manager.get_current_path() {
            self.player.play(path, None, true)
        }
        else {
            Err("当前播放列表没有选中项".into())
        }
    }

    /// 播放已经选中的项目, 并向所有客户端广播对应的索引变更.
    fn play_and_broadcast_selection(
        &mut self,
        change: PlaylistChangeMessage,
    ) -> Result<(), String> {
        // 即使媒体加载失败也同步 daemon 已提交的选择状态.
        let play_result = self.play_current();
        self.broadcast_message(ServerMessage::PlaylistChange(change))?;
        play_result
    }


    // ====================================================================== //

    /// 记录新客户端的写队列和 socket 清理句柄.
    fn handle_connected(&mut self,
        client_id: u64,
        outbound: mpsc::SyncSender<ServerMessage>,
        shutdown: UnixStream,
    ) {
        self.connections.insert(
            client_id,
            ClientConnection {
                outbound,
                shutdown,
            },
        );
    }

    /// 从连接表移除已断开的客户端.
    fn handle_disconnected(&mut self, client_id: u64) {
        self.connections.remove(&client_id);
    }

    /// libmpv 属性发生变化时广播最新播放器状态.
    fn handle_property_changed(&mut self) {
        let state = self.player.get_player_state();

        // 如果返回错误, 打印输出
        if let Err(e) = self.broadcast_message(ServerMessage::UpdatePlayerState(state)) {
            eprintln!("广播播放器状态失败: {e}");
        }
    }

    /// 自然播放结束时推进到下一首, 并广播状态和当前索引增量变更.
    fn handle_playback_ended(&mut self) {

        // 尝试更新列表
        match self.playlist_manager.select_next() {
            Ok(change) => if let Err(error) = self.play_and_broadcast_selection(change) {
                eprintln!("daemon 自动播放下一首失败: {error}");
            },
            Err(e) => { eprintln!("无法选择下一首: {e}"); }
        }
    }

    /// 分发客户端请求, 并在收到 Shutdown 后结束 daemon 主循环.
    fn handle_request_event(&mut self, client_id: u64, line: String) {

        if !self.connections.contains_key(&client_id) {
            println!("客户端未连接: {client_id}");
            return;
        }

        if let Err(error) = self.handle_request(client_id, &line) {
            eprintln!("daemon 请求处理失败: {error}");
        }
    }
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

/// daemon 主循环接收的 socket 和播放器事件.
enum ServerEvent {

    ClientConnected {
        client_id: u64,
        outbound: mpsc::SyncSender<ServerMessage>,
        shutdown: UnixStream,
    },
    ClientDisconnected { client_id: u64 },
    ClientRequest {
        client_id: u64,
        line: String,
    },
    PropertyChanged, // libmpv 属性变化, 需要广播播放器状态.
    PlaybackEnded,   // 当前歌曲自然结束, 需要推进播放列表.
}

/// 向单个客户端写线程发送消息的连接状态.
struct ClientConnection {
    outbound: mpsc::SyncSender<ServerMessage>, // 写线程读取的有界消息队列.
    shutdown: UnixStream, // 慢客户端断开时用于关闭全部 socket clone.
}

type ClientConnections = HashMap<u64, ClientConnection>;

/// daemon 生命周期内自动清理 socket 文件.
struct SocketGuard { path: PathBuf }
impl Drop for SocketGuard {
    /// 删除 daemon 创建的 Unix socket 文件.
    fn drop(&mut self) { let _ = fs::remove_file(&self.path); }
}


// ========================================================================== //

fn get_player_manager_snapshot_by_json_file(path: &Path) -> Result<DaemonSnapshot, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("无法读取播放器管理器快照文件 {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("无法解析播放器管理器快照文件 {}: {e}", path.display()))
}

fn write_daemon_snapshot_to_json_file(path: &Path, snapshot: &DaemonSnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建 daemon 状态目录 {}: {error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(snapshot)
        .map_err(|e| format!("无法序列化播放器管理器快照: {e}"))?;
    fs::write(path, content)
        .map_err(|e| format!("无法写入播放器管理器快照文件 {}: {e}", path.display()))
}

fn write_playlist_to_json_file(path: &Path, playlist: &Playlist) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建播放列表目录 {}: {error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(playlist)
        .map_err(|e| format!("无法序列化播放列表: {e}"))?;
    fs::write(path, content)
        .map_err(|e| format!("无法写入播放列表文件 {}: {e}", path.display()))
}

/// 将用户提供的播放列表名称限制为单个安全文件名.
fn playlist_file_path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("播放列表名称不能为空且不能包含路径分隔符".to_string());
    }
    Ok(utils::state_dir().join("playlists").join(format!("{name}.json")))
}

/// 从 JSON 文件中读取播放列表.
/// 
/// 参数:
/// - `path`: JSON 文件路径.
///
/// 返回值: 解析得到的播放列表, 失败返回错误信息.
fn get_playlist_by_json_file(path: &Path) -> Result<Playlist, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("无法读取播放列表文件 {}: {e}", path.display()))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("get_playlist_by_json_file 无法解析播放列表文件 {}: {e}", path.display()))
}


fn get_playlist_by_txt_file(path: &Path) -> Result<Playlist, String> {

    let content = fs::read_to_string(path)
        .map_err(|e| format!("无法读取播放列表文件 {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new(""));

    let playlist = content
        .lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let line = l.trim(); // 文本列表允许使用空行分隔条目.
            (!line.is_empty()).then_some((i + 1, line))
        })
        .map(|(i, l)| {
            // 绝对路径直接使用, 相对路径基于列表文件目录解析.
            let media_path = PathBuf::from(l);
            let media_path = if media_path.is_absolute() {
                media_path
            } else {
                base.join(media_path)
            };
            utils::canonical_media_path(&media_path).map_err(|e| {
                format!("get_playlist_by_txt_file 无法解析播放列表 {} 第 {i} 行的媒体路径: {e}", path.display())
            })
        })
        .collect::<Result<Playlist, String>>()?;

    Ok(playlist)
}
