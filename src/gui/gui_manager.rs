
// src/gui/gui_manager.rs
// GUI 主线程持有的控件状态和 daemon 连接.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use gtk::{Application, ApplicationWindow, glib};
use gtk::gio::prelude::ApplicationExt;
use gtk::prelude::{GtkWindowExt, ListBoxRowExt};

use crate::gui::{PendingUpdates, UiUpdateRequests, UiUpdateScheduled, utils, widgets};
use crate::models::playlist_manager::PlaylistManager;
use crate::models::ipc::ClientMessage;
use crate::gui::daemon_connection::DaemonConnection;


/// GUI 主线程持有的控件状态和 daemon 连接.
///
/// GTK 控件只能在主线程访问. 当前播放项只由 daemon 消息渲染.
pub(super) struct GuiManager {

    builder: Cell<Option<gtk::Builder>>, // Blueprint 构建器, 按需从中查找控件, 不缓存控件字段.

    connection: Cell<Option<DaemonConnection>>, // GUI 进程的 daemon 长连接.

    // 本地播放列表镜像, 随 daemon 推送的完整快照/增量修改同步更新,
    playlist_manager: RefCell<PlaylistManager>, // 本地播放列表镜像.

    playlist_display_style: RefCell<String>,
}

impl GuiManager {

    /// ACTIVE_UI 在窗口创建前/关闭后使用的占位实例, builder 和 connection 均为空.
    pub(super) fn empty() -> GuiManager {
        GuiManager {
            builder: Cell::new(None),
            connection: Cell::new(None),
            playlist_manager: RefCell::new(PlaylistManager::new()),
            playlist_display_style: RefCell::new(String::from("name")),
        }
    }

    /// 初始化
    pub(super) fn init(&self, app: &Application, socket_path: &std::path::Path) -> Result<(), String> {
        // 初始化逻辑, 如建立与 daemon 的连接, 加载 UI 等
        // 尝试连接到守护进程, 失败时退出应用
        let connection = match DaemonConnection::connect(socket_path) {
            Ok(connection) => connection,
            Err(error) => {
                app.quit();
                return Err(format!("无法连接到守护进程: {error}"));
            }
        };

        // 直接嵌入构建脚本在 OUT_DIR 中生成的 GtkBuilder XML.
        let builder = gtk::Builder::from_string(include_str!(concat!(env!("OUT_DIR"), "/window.ui")));

        let mut states = connection.states();
        self.builder.set(Some(builder));
        self.connection.set(Some(connection));

        // 在 ACTIVE_UI 就绪之后再启动订阅,
        // 避免后台线程在 ACTIVE_UI 就绪前分发初始状态导致播放列表和歌词丢失首次更新.
        thread::spawn(move || {

            let requests: UiUpdateRequests = Arc::new(Mutex::new(PendingUpdates::default()));
            let scheduled: UiUpdateScheduled = Arc::new(AtomicBool::new(false));

            // 处理来自守护进程的状态更新事件循环
            for (player_state, playlist_update) in &mut states {

                if let Ok(mut r) = requests.lock() {
                    r.push(player_state, playlist_update); // 把本次更新加入待处理队列
                }

                // 如果在本次更新期间有新事件到达, 再次调度下一轮 GTK 主线程更新.
                if scheduled
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                { super::gtk_schedule_update(requests.clone(), scheduled.clone()); }
            }

            // 所有状态更新完成后, 关闭 GTK 窗口 (退出应用)
            glib::MainContext::default().invoke(|| {
                super::ACTIVE_UI.with(|active| {
                    let ui = active.borrow().clone();
                    ui.get_widget::<ApplicationWindow>(widgets::WINDOW).close();
                });
            });
        });

        // 关闭前通过 hyprctl 查询窗口当前的位置和大小并保存, 供下次启动
        // 恢复用户手动拖拽/调整过的窗口状态 (原 connect_window_close).
        {
            let app = app.clone();
            self.get_widget::<gtk::ApplicationWindow>(widgets::WINDOW).connect_close_request(move |_| {
                if let Some(geometry) = utils::query_geometry() { crate::utils::save_window_geometry(geometry); }
                super::ACTIVE_UI.with(|active| *active.borrow_mut() = Rc::new(GuiManager::empty())); // 清空 ACTIVE_UI, 避免 GTK 主线程在窗口关闭后仍然尝试更新 UI
                app.quit();
                glib::Propagation::Proceed
            });
        }

        // 将 GTK 窗口与 GTK 应用关联, 以便在窗口关闭时退出应用.
        let window = self.get_widget::<gtk::ApplicationWindow>(widgets::WINDOW);
        window.set_application(Some(app));

        Ok(())
    }

    /// 按 id 从 Builder 中查找控件.
    ///
    /// 不再提供 window/title/cover 等具名 getter, 调用方必须在使用处显式
    /// 指定目标类型和 Blueprint id, 禁止把控件句柄缓存或封装成独立访问器.
    pub(super) fn get_widget<T: gtk::prelude::IsA<gtk::glib::Object>>(&self, id: &str) -> T {
        let builder = self.builder.take().expect("GUI::object 在 builder 未就绪时被调用");
        let object = builder.object(id).unwrap_or_else(|| panic!("Blueprint object '{id}' is missing"));
        self.builder.set(Some(builder));
        object
    }

    /// 直接从 GtkListBox 返回升序排列的当前选中行索引.
    pub(super) fn selected_indices(&self) -> Vec<usize> {
        let playlist: gtk::ListBox = self.get_widget(widgets::PLAYLIST);
        let mut selected = playlist
            .selected_rows()
            .into_iter()
            .map(|row| row.index() as usize)
            .collect::<Vec<_>>();
        selected.sort_unstable();
        selected
    }

    /// 把内部 builder 借给闭包使用后原样放回.
    ///
    /// 供 update_media 等只需要 Builder, 不需要完整 GUI 状态(选择, 连接等)
    /// 的自由函数调用, 避免它们依赖整个 GUI 结构体.
    pub(super) fn with_builder<R>(&self, f: impl FnOnce(&gtk::Builder) -> R) -> R {
        let builder = self.builder.take().expect("GUI::with_builder 在 builder 未就绪时被调用");
        let result = f(&builder);
        self.builder.set(Some(builder));
        result
    }

    /// 发送消息到 daemon, 如果 connection 未就绪则 panic.
    pub(super) fn send_message_to_daemon(&self, msg: ClientMessage) -> Result<(), String> {
        let connection = self.connection.take().expect("GUI::connection 在 connection 未就绪时被调用");
        connection.send(msg);
        // Cell::take 会清空槽位, 发送后必须放回连接供后续操作复用.
        self.connection.set(Some(connection));
        Ok(())
    }

    /// 借用 playlist_manager 的不可变引用.
    pub(super) fn playlist_manager(&self) -> std::cell::Ref<'_, PlaylistManager> {
        self.playlist_manager.borrow()
    }

    /// 借用 playlist_manager 的可变引用.
    pub(super) fn playlist_manager_mut(&self) -> std::cell::RefMut<'_, PlaylistManager> {
        self.playlist_manager.borrow_mut()
    }

    /// 获取 window 控件的引用.
    pub(super) fn window(&self) -> gtk::ApplicationWindow {
        self.get_widget(widgets::WINDOW)
    }

    /// 设置播放列表显示样式
    pub(super) fn set_playlist_display_style(&self, style: &str) {
        self.playlist_display_style.replace(style.to_string());
    }

    /// 获取当前播放列表显示样式.
    pub(super) fn get_playlist_display_style(&self) -> String {
        self.playlist_display_style.borrow().clone()
    }
}
