
// src/gui/events.rs
// 绑定窗口中全部用户交互事件到 daemon 命令或本地 GUI 状态.

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use crate::gui::update_ui;
use crate::models::playlist_manager::{PlaylistChangeKind, PlaylistChangeMessage};
use crate::models::ipc::ClientMessage;

use super::GuiManager;
use super::widgets as names;


/// 播放列表按钮支持的相对或绝对移动方式.
#[derive(Clone, Copy)]
enum MoveAction {
    Up,
    Down,
    Top,
    Bottom,
    SelectedAfterCurrent,
    CurrentBeforeSelected,
}

/// 拖拽指针相对播放列表可视区域的位置, 用于定时自动滚动.
#[derive(Clone, Copy)]
struct DragScrollPosition {
    from_index: usize, // 被拖拽项目在移动前列表中的索引.
    viewport_y: f64, // 指针相对可视区域顶部的纵坐标.
}


/// 绑定按钮点击后原样发给 daemon 的固定命令, 不需要读取任何 GUI 状态.
fn bind_command(gui: &Rc<GuiManager>, id: &str, command: ClientMessage) {
    let button: gtk::Button = gui.get_widget(id);
    let gui = gui.clone();
    button.connect_clicked(move |_| {
        if let Err(e) = gui.send_message_to_daemon(command.clone()) {
            eprintln!("无法发送命令到 daemon: {e}");
        }
    });
}

/// 绑定按钮点击后移动当前选中的播放列表项 (原 up/down/top/bottom/before/after 按钮).
fn bind_move(gui: &Rc<GuiManager>, id: &str, action: MoveAction) {
    let button: gtk::Button = gui.get_widget(id);
    let gui = gui.clone();
    button.connect_clicked(move |_| move_selected(&gui, action));
}

/// 连接窗口中的全部用户事件和 daemon 状态订阅.
pub(super) fn connect_events(gui: &Rc<GuiManager>) {

    // 只发送固定命令, 不读取 GUI 状态的按钮 (原 connect_buttons 的一部分).
    bind_command(gui, names::BUTTON_PREVIOUS, ClientMessage::Previous);
    bind_command(gui, names::BUTTON_PLAY, ClientMessage::Toggle);
    bind_command(gui, names::BUTTON_NEXT, ClientMessage::Next);
    bind_command(gui, names::BUTTON_REFRESH_PLAYLIST, ClientMessage::GetPlaylist);

    // 切换播放列表显示按钮
    {
        let btn = gui.get_widget::<gtk::Button>(names::BUTTON_TOGGLE_PLAYLIST_DISPLAY);
        let gui = gui.clone();
        btn.connect_clicked(move |_| {
            println!("切换播放列表显示按钮被点击");
            let style = gui.get_playlist_display_style();
            println!("当前播放列表显示样式: {style}");

            if style == "name" {
                gui.set_playlist_display_style("path");
            } else {
                gui.set_playlist_display_style("name");
            }

            let style_after = gui.get_playlist_display_style();
            println!("切换播放列表显示样式后: {style_after}");

            // 切换播放列表显示样式后, 需要重新渲染播放列表行.
            let playlist_box = &gui.get_widget::<gtk::ListBox>(names::PLAYLIST);
            let binding = gui.playlist_manager();
            let playlist = binding.get_playlist_ref();
            let new_current = gui.playlist_manager().get_current();
            update_ui::render_playlist_rows(playlist_box, playlist, new_current, &gui.get_playlist_display_style());
        });
    }

    // GtkRange 的 change-value 只表示用户请求的值变化,播放器状态调用
    // set_value 刷新进度时不会触发该信号,因此不会形成 Seek 回路.
    {
        let progress: gtk::Scale = gui.get_widget(names::PROGRESS_SCALE);
        progress.set_increments(1.0, 10.0);
        let gui = gui.clone();
        progress.connect_change_value(move |_progress, _scroll_type, value| {
            if let Err(e) = gui.send_message_to_daemon(ClientMessage::Seek(value.max(0.0))) {
                eprintln!("无法发送命令到 daemon: {e}");
            }
            glib::Propagation::Proceed
        });
    }

    // 音量滑块使用固定 0 到 100 范围,用户操作通过 daemon 写入 mpv,
    // 后续播放器状态广播会将实际生效值同步回所有 GUI 客户端.
    {
        let volume: gtk::Scale = gui.get_widget(names::VOLUME_SCALE);
        volume.set_range(0.0, 100.0);
        volume.set_increments(1.0, 5.0);
        volume.set_value(100.0);
        let gui = gui.clone();
        volume.connect_change_value(move |_volume, _scroll_type, value| {
            if let Err(e) = gui.send_message_to_daemon(ClientMessage::SetVolume(value.clamp(0.0, 100.0))) {
                eprintln!("无法发送命令到 daemon: {e}");
            }
            glib::Propagation::Proceed
        });
    }

    // 播放列表移动按钮共用同一套索引计算和 daemon 请求逻辑.
    for (id, action) in [
        (names::BUTTON_SELECT_UP, MoveAction::Up),
        (names::BUTTON_SELECT_DOWN, MoveAction::Down),
        (names::BUTTON_SELECT_TOP, MoveAction::Top),
        (names::BUTTON_SELECT_BOTTOM, MoveAction::Bottom),
        (
            names::BUTTON_CURRENT_TO_SELECTED_UP,
            MoveAction::CurrentBeforeSelected,
        ),
        (
            names::BUTTON_SELECTED_TO_CURRENT_DOWN,
            MoveAction::SelectedAfterCurrent,
        ),
    ] {
        bind_move(gui, id, action);
    }

    // 选中当前播放行, 并将其滚动到播放列表可视区域中央附近 (原 locate_current).
    {
        let locate: gtk::Button = gui.get_widget(names::BUTTON_LOCATE);
        let gui = gui.clone();
        locate.connect_clicked(move |_| {
            let result: Result<(), String> = (|| {
                let playlist: gtk::ListBox = gui.get_widget(names::PLAYLIST);
                // 从本地播放列表镜像读取当前播放索引.
                let current_index = gui.playlist_manager().get_current()
                    .ok_or_else(|| "播放列表未选择当前歌曲".to_string())?;

                let Some(row) = playlist.row_at_index(current_index as i32) else {
                    return Err("无法找到当前播放的歌曲在播放列表中的行".to_string());
                };

                playlist.unselect_all();
                playlist.select_row(Some(&row));
                let adjustment = playlist
                    .ancestor(gtk::ScrolledWindow::static_type())
                    .and_downcast::<gtk::ScrolledWindow>()
                    .map(|scroll| scroll.vadjustment());
                if let Some(adjustment) = adjustment {
                    let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                    let value = row.allocation().y() as f64 - adjustment.page_size() / 2.0;
                    adjustment.set_value(value.clamp(adjustment.lower(), upper));
                }
                Ok(())
            })();
            if let Err(error) = result { eprintln!("{error}"); }
        });
    }

    // 使用 GtkListBox 原生多选作为所有播放列表操作的选择状态来源.
    {
        let playlist: gtk::ListBox = gui.get_widget(names::PLAYLIST);

        {
            let gui = gui.clone();
            playlist.connect_row_activated(move |_, row| {
                // 从头播放用户激活的播放列表项.
                if let Err(e) = gui.send_message_to_daemon(ClientMessage::Play(row.index() as usize, Duration::ZERO, true)) {
                    eprintln!("无法发送命令到 daemon: {e}");
                }
            });
        }

        // 鼠标左键按住播放列表行拖动调整顺序 (行上的 DragSource 见
        // update_ui::new_playlist_row). 鼠标位于目标行上半部分时插入该行
        // 之前, 位于下半部分时插入该行之后, CSS 插入线同步展示最终位置.
        //
        // 这里只发送 PlaylistChange::Move 请求, 不直接移动 GTK 行,
        // 真正的界面更新仍然只在收到 daemon 广播确认后由
        // apply_playlist_change 应用.
        let drop_target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::MOVE);
        drop_target.set_preload(true); // motion 阶段提前读取源索引, 以便实时计算和显示插入位置.
        let highlighted_row = Rc::new(RefCell::new(None::<gtk::ListBoxRow>));
        let scroll_position = Rc::new(Cell::new(None::<DragScrollPosition>));
        let scroll_source = Rc::new(RefCell::new(None::<glib::SourceId>));
        {
            let highlighted_row = highlighted_row.clone();
            let playlist = playlist.clone();
            let scroll_position = scroll_position.clone();
            let scroll_source = scroll_source.clone();
            drop_target.connect_motion(move |target, _x, y| {
                let Some(payload) = target.value_as::<String>() else {
                    return gdk::DragAction::empty();
                };
                let Ok((from_index, _sources)) =
                    serde_json::from_str::<(usize, Vec<usize>)>(&payload)
                else {
                    return gdk::DragAction::empty();
                };
                let Some(scrolled_window) = playlist
                    .ancestor(gtk::ScrolledWindow::static_type())
                    .and_downcast::<gtk::ScrolledWindow>()
                else {
                    return gdk::DragAction::empty();
                };
                let adjustment = scrolled_window.vadjustment();
                scroll_position.set(Some(DragScrollPosition {
                    from_index,
                    viewport_y: y - adjustment.value(),
                }));
                show_drop_indicator(&playlist, &highlighted_row, y, from_index);

                // 指针停在边缘不动时仍需持续滚动, 因此使用独立定时器,
                // 而不是只在 GDK motion 事件到达时滚动一次.
                if scroll_source.borrow().is_none() {
                    let playlist = playlist.clone();
                    let highlighted_row = highlighted_row.clone();
                    let scroll_position = scroll_position.clone();
                    let source_id = glib::timeout_add_local(Duration::from_millis(16), move || {
                        auto_scroll_playlist(&playlist, &highlighted_row, &scroll_position);
                        glib::ControlFlow::Continue
                    });
                    scroll_source.replace(Some(source_id));
                }
                gdk::DragAction::MOVE
            });
        }
        {
            let highlighted_row = highlighted_row.clone();
            let scroll_position = scroll_position.clone();
            let scroll_source = scroll_source.clone();
            drop_target.connect_leave(move |_target| {
                clear_drop_indicator(&highlighted_row);
                stop_playlist_auto_scroll(&scroll_position, &scroll_source);
            });
        }
        {
            let gui = gui.clone();
            let playlist = playlist.clone();
            let highlighted_row = highlighted_row.clone();
            let scroll_position = scroll_position.clone();
            let scroll_source = scroll_source.clone();
            drop_target.connect_drop(move |_target, value, _x, y| {
                clear_drop_indicator(&highlighted_row);
                stop_playlist_auto_scroll(&scroll_position, &scroll_source);
                let Ok(payload) = value.get::<String>() else { return false };
                let Ok((_from_index, sources)) =
                    serde_json::from_str::<(usize, Vec<usize>)>(&payload)
                else {
                    return false;
                };
                // 恢复按下拖拽源时的 GTK 多选状态, 供 daemon 广播后按索引映射.
                {
                    let this = &gui;
                    let indices: &[usize] = &sources;
                    let playlist: gtk::ListBox = this.get_widget(names::PLAYLIST);
                    playlist.unselect_all();
                    for index in indices {
                            if let Some(row) = playlist.row_at_index(*index as i32) {
                                playlist.select_row(Some(&row));
                            }
                        }
                };
                let Some((boundary, _row, _after)) =
                    playlist_drop_destination(&playlist, y)
                else {
                    return false;
                };
                let moves = move_group_to_boundary(
                    &sources,
                    boundary,
                    playlist.observe_children().n_items() as usize,
                );
                if moves.iter().all(|(from, to)| from == to) {
                    return true;
                }

                let msg = PlaylistChangeMessage {
                    // 读取版本号时保留本地播放列表镜像.
                    state_number: gui.playlist_manager().get_state_number(),
                    change: PlaylistChangeKind::Move(moves),
                };
                if let Err(e) = gui.send_message_to_daemon(ClientMessage::PlaylistChange(msg)) {
                    eprintln!("无法发送命令到 daemon: {e}");
                }
                true
            });
        }
        playlist.add_controller(drop_target);
    }

    // 点击歌词行时请求 daemon 跳转到该行对应的时间戳 (原 connect_lyrics).
    //
    // 时间戳在 update_media 构建歌词行时通过 qdata 绑定在 "lyric-timestamp" 上,
    // 没有该数据的行 (例如 "文件中没有歌词" 占位行) 直接忽略点击.
    {
        let lyrics: gtk::ListBox = gui.get_widget(names::LYRICS);
        let gui = gui.clone();
        lyrics.connect_row_activated(move |_, row| {
            let timestamp = unsafe {
                row.data::<f64>("lyric-timestamp")
                    .map(|value| *value.as_ref())
            };
            if let Some(timestamp) = timestamp {
                if let Err(e) = gui.send_message_to_daemon(ClientMessage::Seek(timestamp)) {
                    eprintln!("无法发送命令到 daemon: {e}");
                }
            }
        });
    }
}

/// 根据鼠标纵坐标计算拖拽项最终索引, 并返回用于绘制插入线的目标行和方向.
///
/// boundary 是移动前列表中的插入间隙. 当间隙位于源项之后时, 先移除源项
/// 会使最终索引减一, 这里据此转换成 daemon Move 使用的目标索引.
fn playlist_drop_destination(
    playlist: &gtk::ListBox,
    y: f64,
) -> Option<(usize, gtk::ListBoxRow, bool)> {
    let item_count = playlist.observe_children().n_items() as usize;
    if item_count == 0 {
        return None;
    }

    let (row, after) = if let Some(row) = playlist.row_at_y(y as i32) {
        let midpoint = row.allocation().y() as f64 + row.allocation().height() as f64 / 2.0;
        (row, y >= midpoint)
    } else if y < 0.0 {
        (playlist.row_at_index(0)?, false)
    } else {
        (playlist.row_at_index(item_count as i32 - 1)?, true)
    };
    let boundary = row.index() as usize + usize::from(after);
    Some((boundary.min(item_count), row, after))
}

/// 将一组选中项移动到原列表的指定插入间隙,保持其原有相对顺序.
fn move_group_to_boundary(
    sources: &[usize],
    boundary: usize,
    playlist_len: usize,
) -> Vec<(usize, usize)> {
    let removed_before = sources.partition_point(|source| *source < boundary);
    let start = boundary
        .saturating_sub(removed_before)
        .min(playlist_len.saturating_sub(sources.len()));
    sources
        .iter()
        .copied()
        .zip(start..start + sources.len())
        .collect()
}

/// 移除上一条拖拽插入线, 保证列表中最多显示一个目标位置.
fn clear_drop_indicator(highlighted_row: &RefCell<Option<gtk::ListBoxRow>>) {
    if let Some(row) = highlighted_row.borrow_mut().take() {
        row.remove_css_class("playlist-drop-before");
        row.remove_css_class("playlist-drop-after");
    }
}

/// 清除旧插入线并按当前指针位置绘制新的插入线.
fn show_drop_indicator(
    playlist: &gtk::ListBox,
    highlighted_row: &RefCell<Option<gtk::ListBoxRow>>,
    y: f64,
    _from_index: usize,
) {
    clear_drop_indicator(highlighted_row);
    let Some((_to_index, row, after)) =
        playlist_drop_destination(playlist, y)
    else {
        return;
    };
    row.add_css_class(if after {
        "playlist-drop-after"
    } else {
        "playlist-drop-before"
    });
    highlighted_row.replace(Some(row));
}

/// 指针靠近可视区域上下边缘时滚动播放列表, 并同步刷新插入线.
fn auto_scroll_playlist(
    playlist: &gtk::ListBox,
    highlighted_row: &RefCell<Option<gtk::ListBoxRow>>,
    scroll_position: &Cell<Option<DragScrollPosition>>,
) {
    const EDGE_SIZE: f64 = 56.0; // 开始自动滚动的边缘区域高度.
    const MAX_STEP: f64 = 12.0; // 每帧最大滚动像素数.

    let Some(position) = scroll_position.get() else { return };
    let Some(scrolled_window) = playlist
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
    else {
        return;
    };
    let adjustment = scrolled_window.vadjustment();
    let page_size = adjustment.page_size();
    let step = if position.viewport_y < EDGE_SIZE {
        -MAX_STEP * (1.0 - position.viewport_y.max(0.0) / EDGE_SIZE)
    } else if position.viewport_y > page_size - EDGE_SIZE {
        MAX_STEP * (1.0 - (page_size - position.viewport_y).max(0.0) / EDGE_SIZE)
    } else {
        0.0
    };
    if step == 0.0 {
        return;
    }

    let upper = (adjustment.upper() - page_size).max(adjustment.lower());
    let old_value = adjustment.value();
    let new_value = (old_value + step).clamp(adjustment.lower(), upper);
    if new_value == old_value {
        return;
    }

    adjustment.set_value(new_value);
    let pointer_y = new_value + position.viewport_y;
    show_drop_indicator(playlist, highlighted_row, pointer_y, position.from_index);
}

/// 停止当前拖拽的自动滚动定时器并丢弃指针位置.
fn stop_playlist_auto_scroll(
    scroll_position: &Cell<Option<DragScrollPosition>>,
    scroll_source: &RefCell<Option<glib::SourceId>>,
) {
    scroll_position.set(None);
    if let Some(source_id) = scroll_source.borrow_mut().take() {
        source_id.remove();
    }
}

/// 根据按钮动作计算全部选中项的目标索引,并请求 daemon 批量移动.
fn move_selected(gui: &GuiManager, action: MoveAction) {
    let selected = gui.selected_indices();
    if selected.is_empty() {
        return;
    }
    let playlist_len = gui.get_widget::<gtk::ListBox>(names::PLAYLIST).observe_children().n_items() as usize;
    if playlist_len == 0 {
        return;
    }
    // 移动计算只读取当前项, 不应清空本地播放列表镜像.
    let current = gui.playlist_manager().get_current();
    let moves = match action {
        MoveAction::Up => {
            if selected[0] == 0 {
                return;
            }
            selected.iter().map(|index| (*index, index - 1)).collect()
        }
        MoveAction::Down => {
            if selected.last().copied() == Some(playlist_len - 1) {
                return;
            }
            selected.iter().map(|index| (*index, index + 1)).collect()
        }
        MoveAction::Top => move_group_to_boundary(&selected, 0, playlist_len),
        MoveAction::Bottom => move_group_to_boundary(&selected, playlist_len, playlist_len),
        MoveAction::SelectedAfterCurrent => {
            let Some(current) = current else { return };
            if selected.binary_search(&current).is_ok() {
                return;
            }
            move_group_to_boundary(&selected, current + 1, playlist_len)
        }
        MoveAction::CurrentBeforeSelected => {
            let Some(current) = current else { return };
            if selected.binary_search(&current).is_ok() {
                return;
            }
            // 当前播放项单独移动到最前一个选中项之前,选中项保持原位.
            move_group_to_boundary(&[current], selected[0], playlist_len)
        }
    };
    if moves.iter().all(|(from, to)| from == to) {
        return;
    }

    // 通过 PlaylistChange::Move 携带本地 state_number 发送, 界面只在
    // 收到 daemon 广播的确认后才真正应用 (见 apply_playlist_change).
    let msg = PlaylistChangeMessage {
        change: PlaylistChangeKind::Move(moves),
        // 请求携带当前版本, 同时保留等待广播确认所需的本地镜像.
        state_number: gui.playlist_manager().get_state_number(),
    };
    if let Err(e) = gui.send_message_to_daemon(ClientMessage::PlaylistChange(msg)) {
        eprintln!("无法发送命令到 daemon: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::move_group_to_boundary;

    /// 验证非连续选择整体插入时保持原顺序并扣除目标前已移除的源项.
    #[test]
    fn moves_selected_group_to_insertion_boundary() {
        assert_eq!(
            move_group_to_boundary(&[1, 3, 5], 7, 8),
            vec![(1, 4), (3, 5), (5, 6)]
        );
        assert_eq!(
            move_group_to_boundary(&[2, 4], 1, 6),
            vec![(2, 1), (4, 2)]
        );
    }

    /// 验证当前播放项无论位于选中项哪一侧,都能移动到首个选中项之前.
    #[test]
    fn moves_current_before_selected_items() {
        assert_eq!(move_group_to_boundary(&[1], 4, 7), vec![(1, 3)]);
        assert_eq!(move_group_to_boundary(&[6], 2, 7), vec![(6, 2)]);
    }

    /// 验证选中组移动到当前播放项之后时保持相对顺序.
    #[test]
    fn moves_selected_items_after_current() {
        assert_eq!(
            move_group_to_boundary(&[1, 4], 4, 7),
            vec![(1, 3), (4, 4)]
        );
        assert_eq!(
            move_group_to_boundary(&[3, 5], 2, 7),
            vec![(3, 2), (5, 3)]
        );
    }
}
