
// src/daemon/mpv_player.rs
// 内嵌 libmpv 播放器的状态和操作封装.

use crate::daemon::{ServerEvent};
use crate::models::PlayerState;

use libmpv2::events::Event;
use libmpv2::{mpv_end_file_reason, Format, Mpv};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;


pub struct EmbeddedPlayer {
    mpv: Arc<Mpv>,
}

impl EmbeddedPlayer {

    /// 创建隔离配置且不输出视频的内嵌 libmpv 播放核心.
    pub(super) fn new(event_sender: mpsc::Sender<ServerEvent>) -> Self {

        let mpv = Mpv::with_initializer(|initializer| {
            initializer.set_option("video", false)?;
            Ok(())
        })
        .unwrap_or_else(|error| panic!("无法初始化 libmpv: {error}"));
        for (id, (name, format)) in [
            (1, ("path", Format::String)),
            (2, ("pause", Format::Flag)),
            (3, ("time-pos", Format::Double)),
            (4, ("duration", Format::Double)),
            (5, ("idle-active", Format::Flag)),
            (6, ("metadata/by-key/title", Format::String)),
            (7, ("metadata/by-key/artist", Format::String)),
            (8, ("metadata/by-key/album", Format::String)),
            (9, ("volume", Format::Double)),
        ] {
            mpv.observe_property(name, format, id)
                .unwrap_or_else(|error| panic!("无法监听 libmpv 属性 {name}: {error}"));
        }

        let mpv = Arc::new(mpv);

        // 专用后台线程阻塞等待 libmpv 事件, 取代 set_wakeup_callback.
        // 只有这一个线程调用 wait_event, 事件队列不会被多线程争抢.
        // 直接把事件语义转换成具体的 ServerEvent 变体通知 daemon, 不再
        // 使用中间的原子标记和 take_events() 轮询.
        let event_mpv = Arc::clone(&mpv);
        thread::spawn(move || loop {
            match event_mpv.wait_event(-1.0) {
                Some(Ok(Event::Shutdown)) => break,
                Some(Ok(Event::EndFile(reason))) if reason == mpv_end_file_reason::Eof => {
                    if event_sender.send(ServerEvent::PlaybackEnded).is_err() {
                        panic!("daemon 主循环已退出, libmpv 事件线程无法发送 PlaybackEnded");
                    }
                }
                Some(Ok(Event::PropertyChange { .. })) => {
                    if event_sender.send(ServerEvent::PropertyChanged).is_err() {
                        panic!("daemon 主循环已退出, libmpv 事件线程无法发送 PropertyChanged");
                    }
                }
                Some(Ok(_)) | None => {}
                Some(Err(_)) => {}
            }
        });

        Self { mpv }
    }

    /// 聚合 libmpv 属性并生成当前播放状态.
    pub(super) fn get_player_state(&self) -> PlayerState {
        PlayerState {
            path: self.property::<String>("path").map(PathBuf::from),
            paused: self.property::<bool>("pause").unwrap_or(false),
            position: self.property::<f64>("time-pos").unwrap_or(0.0),
            duration: self.property::<f64>("duration").unwrap_or(0.0),
            volume: self.property::<f64>("volume").unwrap_or(100.0),
        }
    }

    /// 切换播放和暂停状态.
    pub(super) fn toggle(&self) -> Result<(), String> {
        self.mpv.command("cycle", &["pause"])
        .map_err(|error| format!("libmpv 切换播放状态失败: {error}"))
    }


    /// 使用单文件替换模式从指定秒数加载, 并根据 is_resume 决定是否立即播放.
    pub(super) fn play(&self, path: &Path, position: Option<Duration>, is_resume: bool) -> Result<(), String> {
        let path = path
        .canonicalize()
        .map_err(|error| format!("无法读取 {}: {error}", path.display()))?;
        let path = path.to_string_lossy();
        // pause=yes 作为单文件选项与 start 一起加载, 避免媒体加载后短暂播放.
        // 如果 is_resume 为 true, 则加载后立即播放, 否则保持暂停.
        let options = format!("start={},pause={}", position
        .unwrap_or(Duration::ZERO).as_secs_f64().max(0.0), if is_resume { "no" } else { "yes" });
        self.mpv
        // mpv 0.41 的第三个参数是播放列表插入索引, -1 表示默认位置.
        .command("loadfile", &[path.as_ref(), "replace", "-1", &options])
        .map_err(|error| format!("libmpv 加载暂停媒体失败: {error}"))
    }

    /// 停止当前媒体, 但不修改 daemon 保存的播放列表选择.
    pub(super) fn stop(&self) -> Result<(), String> {
        self.mpv
        .command("stop", &[])
        .map_err(|error| format!("libmpv 停止播放失败: {error}"))
    }

    /// 跳转到当前歌曲的绝对秒数位置.
    pub(super) fn seek(&self, seconds: f64) -> Result<(), String> {
        self.mpv
        .set_property("time-pos", seconds.max(0.0))
        .map_err(|error| format!("libmpv seek 失败: {error}"))
    }

    /// 将播放器音量限制在 GUI 支持的 0 到 100 范围内.
    pub(super) fn set_volume(&self, volume: f64) -> Result<(), String> {
        self.mpv
        .set_property("volume", volume.clamp(0.0, 100.0))
        .map_err(|error| format!("libmpv 设置音量失败: {error}"))
    }

    /// 读取 libmpv 当前实际生效的音量.
    pub(super) fn volume(&self) -> f64 {
        self.property::<f64>("volume").unwrap_or(100.0)
    }

    /// 读取已加载媒体当前实际生效的播放位置.
    pub(crate) fn playback_position(&self) -> Option<Duration> {
        self.property::<f64>("time-pos")
            .filter(|position| position.is_finite())
            .map(|position| Duration::from_secs_f64(position.max(0.0)))
    }


    // ====================================================================== //

    /// 查询一个 libmpv 属性, 不可用时返回 `None`.
    fn property<T: libmpv2::GetData>(&self, name: &str) -> Option<T> {
        self.mpv.get_property(name).ok()
    }
}
