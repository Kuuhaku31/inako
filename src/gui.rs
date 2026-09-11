
// src/gui.rs
// GUI 窗口和 GTK 事件循环

use gtk::gio::ApplicationFlags;
use gtk::gio::prelude::{ApplicationCommandLineExt, ApplicationExtManual};
use gtk::glib;
use gtk::glib::ExitCode;
use gtk::prelude::*;
use gtk::prelude::ListBoxRowExt;
use gtk::Application;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::models::ipc::{ClientMessage};
use crate::models::playlist_manager::{PlaylistChangeKind, PlaylistChangeMessage};
use crate::models::{MediaContent, PlayerState};
use crate::utils as app_utils;

mod utils;
mod update_ui;
mod events;
mod daemon_connection;
mod widgets;
mod gui_manager;

use gui_manager::GuiManager;


/// GTK 应用 ID, 同时也是 Hyprland 中该窗口的 `class`.
///
/// build_window 用它向 Hyprland 下发浮动窗口规则, 因此这里统一常量,
/// 避免与 run_window 中实际设置的 application_id 出现不一致.
const APPLICATION_ID: &str = "io.github.Kuuhaku31.Inako.GUI";

/// 已解析的 GUI 命令.
#[derive(Clone, PartialEq, Eq)]
pub enum WindowCommand {
    ToggleDisplay { socket_path: Option<PathBuf> },
    Quit,
}

/// 运行单实例 GTK 应用并执行二进制入口提供的已解析命令.
///
/// 使用 `HANDLES_COMMAND_LINE` 让每次调用 (无论是首次启动还是后续调用)
/// 的实际命令行参数都转发给同一个已注册的主实例,
/// 由其 `command-line` 信号处理函数统一执行.
/// 因此这里不再需要区分本进程是否为主实例:
/// GLib 会自动把非主实例收到的参数通过 D-Bus 转发给主实例处理,
/// 本进程只是转发一次性命令后立即退出.
pub fn run_window(parse_command: fn(&[OsString]) -> Result<WindowCommand, String>)
    -> Result<(), String>
{

    // 创建 GTK 应用并设置应用 ID
    // HANDLES_COMMAND_LINE 告诉 GTK: 不要只使用默认的命令行处理逻辑,
    // 而是把命令行参数交给应用注册的 connect_command_line 回调
    let app = Application::builder()
    .application_id(APPLICATION_ID)
    .flags(ApplicationFlags::HANDLES_COMMAND_LINE)
    .build();

    // 处理来自本进程或转发自其他调用的命令行参数
    app.connect_command_line(move |app, command_line| {
        let arguments = command_line.arguments();
        let cmd_res = match parse_command(&arguments) {

            Ok(WindowCommand::Quit) => {
                if let Some(window) = app.active_window() { window.close(); }
                app.quit();
                Ok(())
            },

            Ok(WindowCommand::ToggleDisplay { socket_path }) => {
                let socket_path = app_utils::get_socket_path(socket_path);
                match app.active_window() {

                // 窗口已存在且可见时, 切换为隐藏
                Some(window) if window.is_visible() => {
                    if let Some(geometry) = self::utils::query_geometry() {
                        crate::utils::save_window_geometry(geometry);
                    }
                    window.set_visible(false);
                    Ok(())
                }

                // 窗口已存在但不可见时, 切换为显示
                Some(window) => {
                    let geometry = crate::utils::load_window_geometry();
                    window.set_default_size(geometry.width_i32(), geometry.height_i32());
                    self::utils::ensure_floating_rule(Some(geometry));
                    window.present();
                    Ok(())
                },

                // 窗口不存在时创建新窗口
                None => {
                    let gui = ACTIVE_UI.with(|active| active.borrow().clone());

                    // 初始化 GUI
                    if let Err(e) = gui.init(app, &socket_path)
                        { Err(format!("GUI 初始化失败: {e}")) }
                    else {
                        let gui: &Rc<GuiManager> = &gui;
                        events::connect_events(gui);

                        let provider = gtk::CssProvider::new();
                        provider.load_from_data(include_str!("./gui/style.css"));

                        // 应用 CSS 样式
                        if let Some(display) = gtk::gdk::Display::default() {
                            gtk::style_context_add_provider_for_display(
                                &display,
                                &provider,
                                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                            );
                        }
                        if let Some(settings) = gtk::Settings::default() { settings.set_gtk_application_prefer_dark_theme(true); }
                        {
                            let window = &gui.window();
                            let geometry = crate::utils::load_window_geometry();
                            window.set_default_size(geometry.width_i32(), geometry.height_i32());
                            self::utils::ensure_floating_rule(Some(geometry));
                            window.present();
                        }

                        Ok(())
                    }
                }
                }
            }
            Err(error) => Err(error),
        };

        match cmd_res {
            Ok(_) => ExitCode::SUCCESS,
            Err(err) => {eprintln!("Error: {err}"); ExitCode::FAILURE }
        }
    });

    // 运行 GTK 应用, 使用当前进程真实的命令行参数.
    app.run();
    Ok(())
}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

thread_local! {
    // GTK 主线程当前显示的唯一 GUI 实例.
    //
    // 始终持有一个有效的 Rc<GUI>, 未创建窗口时为 GUI::empty() 占位实例
    // (builder/connection 均为 None), 通过 GUI::is_ready 判断是否已就绪.
    static ACTIVE_UI: RefCell<Rc<GuiManager>> = RefCell::new(Rc::new(GuiManager::empty()));
}

/// GTK 主线程尚未应用的最新 daemon 更新.
///
/// 播放器状态只保留最新一条.
/// 播放列表更新必须保序排队,
/// 否则会跳过中间的结构编辑.
#[derive(Default)]
struct PendingUpdates {
    player_state : Option<PlayerState>,
    playlist_change_messages : VecDeque<PlaylistChangeMessage>,
}


impl PendingUpdates {

    /// 追加一组更新. 播放器状态替换为最新值, 播放列表更新追加到队尾;
    /// 完整快照会清空队列中此前排队的增量修改, 因为它们已经过期.
    fn push(
        &mut self,
        player_state: Option<PlayerState>,
        playlist_update: Option<PlaylistChangeMessage>,
    ) {
        // 播放器状态只保留最新一条.
        if let Some(state) = player_state {
            self.player_state = Some(state);
        }

        // 播放列表快照替代此前积压的所有增量更新.
        if let Some(msg) = playlist_update {

            // 如果是播放列表重建消息, 则清空此前的增量更新队列
            if matches!(&msg.change, PlaylistChangeKind::Rebuild(_, _)) {
                self.playlist_change_messages.clear();
            }
            self.playlist_change_messages.push_back(msg);
        }
    }

    /// 返回槽位中是否仍有任意待处理更新.
    fn is_empty(&self) -> bool {
        self.player_state.is_none() && self.playlist_change_messages.is_empty()
    }
}


/// 待处理的 UI 更新队列
///
/// Mutex 保护的 PendingUpdates, 需要在 GTK 进程和后台状态线程之间共享
type UiUpdateRequests = Arc<Mutex<PendingUpdates>>;
/// 是否已经把一个 UI 更新任务排进 GTK 主线程的标志
///
/// true 表示当前已经安排好一轮更新, 不要再重复排.
/// 处理完后再置回 false, 准备下次事件
type UiUpdateScheduled = Arc<AtomicBool>;



// ========================================================================== //
// ========================================================================== //
// ========================================================================== //

/// 保证同一时刻最多存在一个待执行的 GTK 主线程更新任务.
///
/// 执行期间到达的新事件保留在 pending 中, 当前任务结束后会再次调度,
/// 从而兼顾实时性并避免主线程任务队列无限增长.
fn gtk_schedule_update(requests: UiUpdateRequests, scheduled: UiUpdateScheduled) {

    // 安排 GTK 主线程执行 UI 更新任务
    glib::MainContext::default().invoke(move || {

        // 执行 UI 更新回调
        ACTIVE_UI.with(|g| update_ui_callback(g, &requests));

        // 标记当前 UI 更新任务已完成
        scheduled.store(false, Ordering::Release);

        // 检查是否有待处理的 UI 更新任务
        // 仅在确有新请求时置位. 否则会把 scheduled 留在 true 而没有任务执行.
        let has_request = requests.lock().map(|r| !r.is_empty()).unwrap_or(false);
        if has_request {
            let need = scheduled.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok();
            if need { gtk_schedule_update(requests, scheduled); }
        }

    });

//}

/// 刷新 UI 逻辑入口
fn update_ui_callback(active_gui: &RefCell<Rc<GuiManager>>, requests: &UiUpdateRequests) {

    // 从请求队列中获取最新的播放器状态和播放列表更新列表
    let (latest_player_state, playlist_update_list) = requests.lock()
    .map(|mut r| { (r.player_state.take(), std::mem::take(&mut r.playlist_change_messages)) })
    .unwrap_or_default();

    let gui = active_gui.borrow().clone();

    // 更新播放列表 GTK 列表控件
    for msg in playlist_update_list {

        let playlist_box = &gui.get_widget::<gtk::ListBox>(widgets::PLAYLIST);
        let previous_selected = playlist_box
            .selected_rows()
            .into_iter()
            .map(|row| row.index() as usize)
            .collect::<Vec<_>>(); // 移动前的全部选择项索引.
        let previous_playlist_len = gui.playlist_manager().get_playlist_ref().len();
        let selection_index_map = if matches!(&msg.change, PlaylistChangeKind::Move(_)) {
            match app_utils::playlist_index_map(&msg.change, previous_playlist_len) {
                Ok(index_map) => Some(index_map),
                Err(error) => {
                    eprintln!("无法计算播放列表选择项的新索引: {error}");
                    if let Err(error) = gui.send_message_to_daemon(ClientMessage::GetPlaylist) {
                        eprintln!("无法发送 GetPlaylist 消息到 daemon: {error}");
                    }
                    break;
                }
            }
        } else {
            None
        };
        let previous_current_visible = {
            // 在申请可变借用前释放只读借用, 避免 RefCell 借用冲突.
            let playlist_manager = gui.playlist_manager();
            playlist_manager
                .get_current()
                .is_some_and(|c| app_utils::playlist_row_is_partly_visible(playlist_box, c))
        };

        // 应用播放列表变更
        let change_msg = match gui.playlist_manager_mut().apply_playlist_change(msg) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("无法应用 daemon 播放列表变更: {e}");
                if let Err(e) = gui.send_message_to_daemon(ClientMessage::GetPlaylist) {
                    eprintln!("无法发送 GetPlaylist 消息到 daemon: {e}");
                }
                break;
            }
        };

        // 刷新 UI
        let playlist_manager = gui.playlist_manager();
        let playlist = playlist_manager.get_playlist_ref();

        let new_current = playlist_manager.get_current(); // 当前播放索引
        let new_current_path = playlist_manager.get_current_path(); // 当前播放项路径

        // 重建 GTK 列表
        update_ui::render_playlist_rows(playlist_box, playlist, new_current, &gui.get_playlist_display_style());

        // 移动操作: 恢复移动后的选择项并确保其位于可见区域内.
        if matches!(&change_msg.change, PlaylistChangeKind::Move(_)) {
            let new_selected = selection_index_map
                .map(|index_map| {
                    previous_selected
                        .into_iter()
                        .filter_map(|old_index| index_map.get(old_index).copied().flatten())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // 确保移动后的选择项在可见区域内
            update_ui::ensure_playlist_row_visible_mut(playlist_box.clone(), new_selected.clone());

            // 重绘后按完整索引映射恢复全部选择项.
            playlist_box.unselect_all();
            for index in new_selected {
                if let Some(row) = playlist_box.row_at_index(index as i32) {
                    playlist_box.select_row(Some(&row));
                }
            }
        }

        // 只有 ResetCurrent 表示播放实体切换.结构编辑仅改变同一实体的索引,
        // 不应触发播放项切换时的自动滚动交互.
        if matches!(change_msg.change, PlaylistChangeKind::ResetCurrent(_))
        || matches!(change_msg.change, PlaylistChangeKind::Rebuild(_ , _ ))
        {
            if let Some(c) = new_current && previous_current_visible {
                update_ui::ensure_playlist_row_visible(playlist_box.clone(), c);
            }

            // 根据当前地址获取媒体内容
            println!("开始加载媒体内容: current_path: {:?}", new_current_path);
            let content = if let Some(path) = new_current_path.as_deref()
            && let Ok(content) = app_utils::read_media(path) {content }
            else { MediaContent::default() };

            // 刷新封面等媒体信息
            gui.with_builder(|builder| update_ui::update_ui_media_content(builder, &content));
        }
    }


    // 更新播放器状态显示
    if let Some(state) = latest_player_state {
        gui.get_widget::<gtk::Label>(widgets::TIME_LABEL).set_text(&format!(
            "{} / {}",
            app_utils::format_time(state.position),
            app_utils::format_time(state.duration)
        ));
        gui.get_widget::<gtk::Button>(widgets::BUTTON_PLAY).set_label(if state.paused { "▶" } else { "⏸" });
        let progress = gui.get_widget::<gtk::Scale>(widgets::PROGRESS_SCALE);
        let duration = state.duration.max(0.0);
        if duration > 0.0 {
            progress.set_range(0.0, duration);
            progress.set_value(state.position.clamp(0.0, duration));
            progress.set_sensitive(true);
        } else {
            progress.set_range(0.0, 1.0);
            progress.set_value(0.0);
            progress.set_sensitive(false);
        }
        gui.get_widget::<gtk::Scale>(widgets::VOLUME_SCALE)
            .set_value(state.volume.clamp(0.0, 100.0));

        let current_path = gui.playlist_manager().get_current_path();
        if current_path != state.path {
            if let Some(path) = state.path.as_deref() {
                if let Ok(content) = app_utils::read_media(path) {
                    gui.with_builder(|builder| update_ui::update_ui_media_content(builder, &content));
                }
            } else {
                gui.with_builder(|builder| update_ui::update_ui_media_content(builder, &MediaContent::default()));
            }
        }

        let lyric_box = gui.get_widget::<gtk::ListBox>(widgets::LYRICS).clone();
        let position = state.position;
        let mut active_row = None;
        let mut previous_row = None;
        let mut index = 0;
        while let Some(row) = lyric_box.row_at_index(index) {
            if row.has_css_class("current-lyric") {
                previous_row = Some(index);
            }
            let timestamp = unsafe {
                row.data::<f64>("lyric-timestamp")
                    .map(|value| *value.as_ref())
            };
            match timestamp {
                Some(timestamp) if timestamp <= position => active_row = Some(index),
                Some(_) => break,
                None => {}
            }
            index += 1;
        }
        let changed = previous_row != active_row;
        let mut index = 0;
        while let Some(row) = lyric_box.row_at_index(index) {
            if Some(index) == active_row {
                row.add_css_class("current-lyric");
            } else {
                row.remove_css_class("current-lyric");
            }
            index += 1;
        }
        if changed {
            if let Some(active_row) = active_row {
                glib::idle_add_local_once(move || {
                    let Some(row) = lyric_box.row_at_index(active_row) else {
                        return;
                    };
                    if !row.has_css_class("current-lyric") {
                        return;
                    }
                    let Some(adjustment) = lyric_box
                        .ancestor(gtk::ScrolledWindow::static_type())
                        .and_downcast::<gtk::ScrolledWindow>()
                        .map(|scroll| scroll.vadjustment())
                    else {
                        return;
                    };
                    let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    let row_center = row.allocation().y() as f64 + row.allocation().height() as f64 / 2.0;
                    let target = row_center - adjustment.page_size() / 2.0;
                    adjustment.set_value(target.clamp(adjustment.lower(), upper));
                });
            }
        }
    }
}}


// ========================================================================== //
// ========================================================================== //
// ========================================================================== //


#[cfg(test)]
mod tests {
    use super::*;

    /// 验证完整快照会替代尚未交给 GTK 的旧增量消息.
    #[test]
    fn pending_rebuild_discards_stale_incremental_updates() {
        let mut pending = PendingUpdates::default();

        // 先排入旧版本增量, 再用较新的完整快照覆盖它.
        pending.push(None, Some(PlaylistChangeMessage {
            state_number: 0,
            change: PlaylistChangeKind::Add(vec![(0, PathBuf::from("old"))]),
        }));
        pending.push(None, Some(PlaylistChangeMessage {
            state_number: 4,
            change: PlaylistChangeKind::Rebuild(vec![PathBuf::from("new")], Some(0)),
        }));

        assert_eq!(pending.playlist_change_messages.len(), 1);
        let message = pending.playlist_change_messages.front().unwrap();
        assert_eq!(message.state_number, 4);
        assert!(matches!(&message.change, PlaylistChangeKind::Rebuild(_, Some(0))));
    }
}
