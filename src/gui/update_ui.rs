
// src/gui/update_ui.rs
// 负责把 daemon 推送的状态更新合并调度并应用到 GTK 控件, 即 UI 刷新逻辑.

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gdk_pixbuf::PixbufLoader;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::models::{MediaContent, playlist_manager::Playlist};

use super::ACTIVE_UI;
use super::widgets as names;


/// 按播放列表镜像刷新全部 GTK 行,并尽量复用现有行控件.
pub(crate) fn render_playlist_rows(
    playlist_box: &gtk::ListBox,
    playlist: &Playlist,
    current: Option<usize>,
    display_type: &str,
) {
    for (index, path) in playlist.iter().enumerate() {
        let row = playlist_box
            .row_at_index(index as i32)
            .unwrap_or_else(|| {
                let row = new_playlist_row();
                playlist_box.append(&row);
                row
            });
        set_row_content(&row, path, index, Some(index) == current, display_type);
    }
    while let Some(row) = playlist_box.row_at_index(playlist.len() as i32) {
        playlist_box.remove(&row);
    }
}

/// 创建一个空播放列表行 (内容留待 set_row_content 填充), 并绑定鼠标左键
/// 拖动排序所需的 DragSource.
///
/// 拖拽开始前直接从 GtkListBox 捕获选中行, 并将源索引和批量源索引一起
/// 写入拖拽载荷, 避免放置时鼠标手势已经改变原有多选状态.
/// 实际的顺序调整仍然只在 events.rs 的 DropTarget 收到 daemon 广播确认
/// 后由 apply_playlist_change 应用, 这里只负责发起拖拽.
fn new_playlist_row() -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&gtk::Label::new(None)));

    // 右键点击当前行时显示复制文件地址菜单.
    let context_click = gtk::GestureClick::new();
    context_click.set_button(gdk::BUTTON_SECONDARY);
    let row_for_context = row.clone();
    context_click.connect_pressed(move |_gesture, _press_count, x, y| {
        // tooltip 始终由 set_row_content 更新为当前行对应的完整路径.
        let Some(path) = row_for_context
            .child()
            .and_downcast::<gtk::Label>()
            .and_then(|label| label.tooltip_text())
        else {
            return;
        };

        // 菜单锚定到右键位置, 并在关闭时从控件树移除.
        let popover = gtk::Popover::new();
        // 固定标签宽度, 避免超长路径把复制按钮撑出窗口可视区域.
        let copy_button = gtk::Button::with_label("复制地址");
        copy_button.set_tooltip_text(Some(&path));
        copy_button.add_css_class("flat");
        popover.set_child(Some(&copy_button));
        popover.set_parent(&row_for_context);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));

        // 点击菜单项后把当前行地址写入系统剪贴板并关闭菜单.
        let display = row_for_context.display();
        let popover_weak = popover.downgrade();
        copy_button.connect_clicked(move |_| {
            display.clipboard().set_text(&path);
            if let Some(popover) = popover_weak.upgrade() {
                popover.popdown();
            }
        });
        popover.connect_closed(|popover| {
            // 手动挂载的弹出菜单关闭后必须解除父子关系.
            popover.unparent();
        });
        popover.popup();
    });
    row.add_controller(context_click);

    // 在 ListBox 处理单击选择前记录本次拖拽应包含的实际选中行.
    let drag_indices = Rc::new(RefCell::new(Vec::<usize>::new()));
    let press = gtk::GestureClick::new();
    press.set_button(gdk::BUTTON_PRIMARY);
    press.set_propagation_phase(gtk::PropagationPhase::Capture);
    let row_for_press = row.clone();
    let indices_for_press = drag_indices.clone();
    press.connect_pressed(move |_gesture, _press_count, _x, _y| {
        let source_index = row_for_press.index() as usize;
        let selected = ACTIVE_UI.with(|active| active.borrow().selected_indices());
        let sources = if selected.binary_search(&source_index).is_ok() {
            selected
        } else {
            vec![source_index]
        };
        indices_for_press.replace(sources);
    });
    row.add_controller(press);

    let drag_source = gtk::DragSource::new();
    drag_source.set_button(gdk::BUTTON_PRIMARY); // 仅响应鼠标左键, 避免和右键菜单等冲突.
    drag_source.set_actions(gdk::DragAction::MOVE); // 与播放列表上的 DropTarget 使用相同动作, 确保松开鼠标时接受放置.
    let row_for_drag = row.clone();
    let indices_for_prepare = drag_indices.clone();
    drag_source.connect_prepare(move |_source, _x, _y| {
        // JSON 载荷同时保留实际按下的行和按下时的完整多选集合.
        let payload = serde_json::to_string(&(
            row_for_drag.index() as usize,
            indices_for_prepare.borrow().as_slice(),
        ))
        .ok()?;
        Some(gdk::ContentProvider::for_value(&payload.to_value()))
    });
    let row_for_drag = row.clone();
    let indices_for_begin = drag_indices.clone();
    drag_source.connect_drag_begin(move |_source, drag| {
        // 已选行被拖动时整组选中项进入暗淡状态,预览显示移动项目数量.
        let (text, dragged_rows) = ACTIVE_UI.with(|active| {
            let gui = active.borrow().clone();
            let playlist = gui.get_widget::<gtk::ListBox>(names::PLAYLIST);
            let indices = indices_for_begin.borrow();
            let rows: Vec<gtk::ListBoxRow> = indices
                .iter()
                .filter_map(|index| playlist.row_at_index(*index as i32))
                .collect();
            for row in &rows {
                row.add_css_class("playlist-drag-source");
            }
            let text = if rows.len() > 1 {
                format!("移动 {} 个项目", rows.len())
            } else {
                row_for_drag
                    .child()
                    .and_downcast::<gtk::Label>()
                    .map(|label| label.text().to_string())
                    .unwrap_or_default()
            };
            (text, rows)
        });
        let preview = gtk::Label::new(Some(&text));
        preview.set_xalign(0.0);
        preview.set_size_request(row_for_drag.width(), row_for_drag.height());
        preview.add_css_class("playlist-drag-icon");
        gtk::DragIcon::for_drag(drag).set_child(Some(&preview));
        unsafe {
            row_for_drag.set_data("playlist-drag-rows", dragged_rows);
        }
    });
    let row_for_drag = row.clone();
    let indices_for_end = drag_indices;
    drag_source.connect_drag_end(move |_source, _drag, _delete_data| {
        // 本次拖拽结束后丢弃临时索引, 下次按下时重新读取 ListBox.
        indices_for_end.borrow_mut().clear();
        let dragged_rows = unsafe {
            row_for_drag.steal_data::<Vec<gtk::ListBoxRow>>("playlist-drag-rows")
        };
        if let Some(dragged_rows) = dragged_rows {
            for row in dragged_rows {
                row.remove_css_class("playlist-drag-source");
            }
        }
    });
    row.add_controller(drag_source);

    row
}

/// 读取音频标签并按 "专辑 · 歌手 · 标题" 顺序拼接展示文本, 缺失的字段直接跳过.
/// 标题缺失或标签读取失败时, 整体回退为文件名, 避免播放列表行显示空白.
fn track_tags_display_text(path: &Path) -> String {
    let fallback_name = || {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    };

    let Some((title, artist, album)) = crate::utils::read_track_tags(path) else {
        return fallback_name();
    };

    let title = if title.is_empty() { fallback_name() } else { title };

    [album, artist, title]
        .into_iter()
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 设置一行播放列表控件的编号, 文件路径, 提示文本和当前播放标记.
///
/// tooltip 同时保存完整路径, 供 reindex_rows/apply_playlist_change 在
/// 没有完整 Playlist 快照时按行读回路径.
fn set_row_content(row: &gtk::ListBoxRow, path: &Path, index: usize, is_current: bool, display_type: &str) {
    let label = row
        .child()
        .and_downcast::<gtk::Label>()
        .expect("playlist row child must be a GtkLabel");

    // display_type 为 "name" 只显示文件名, "tags" 显示 专辑 · 歌手 · 标题,
    // 其余情况 (包括 "path") 显示完整路径.

    // 行文本根据 display_type 决定显示内容.
    let display_text = if display_type == "name" {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string()
    } else if display_type == "tags" {
        track_tags_display_text(path)
    } else {
        path.display().to_string()
    };
    label.set_text(&format!("{:02}  {}", index + 1, display_text));
    label.set_xalign(0.0);
    // 保留标签自然宽度, 由外层 ScrolledWindow 提供水平滚动.
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_tooltip_text(Some(&path.to_string_lossy()));
    if is_current {
        row.add_css_class("current-playlist");
    } else {
        row.remove_css_class("current-playlist");
    }
}

/// 使用 daemon 推送的媒体内容重建标题, 歌词行和封面.
pub(super) fn update_ui_media_content(builder: &gtk::Builder, content: &MediaContent) {

    let mut title = content.title.clone();
    if !content.artist.is_empty() {
        title = format!("{} - {title}", content.artist);
    }
    if !content.album.is_empty() {
        title.push_str(&format!("  ·  {}", content.album));
    }
    builder.object::<gtk::Label>(names::TITLE_LABEL).unwrap_or_else(|| panic!("Blueprint object '{}' is missing", names::TITLE_LABEL)).set_text(&title);

    // 每行保存时间戳, 播放进度更新时无需重新解析歌词.
    let lyrics_box = builder.object::<gtk::ListBox>(names::LYRICS).unwrap_or_else(|| panic!("Blueprint object '{}' is missing", names::LYRICS));
    // 删除列表中的所有现有行 (原 clear_list).
    while let Some(child) = lyrics_box.first_child() {
        lyrics_box.remove(&child);
    }
    if let Some(lyrics) = &content.lyrics {
        if lyrics.is_empty() {
            let label = gtk::Label::new(Some("文件中没有歌词"));
            label.add_css_class("dim-label");
            lyrics_box.append(&label);
        } else {
            for lyric in lyrics {
                let label = gtk::Label::new(Some(&lyric.text));
                label.set_wrap(true);
                label.set_margin_top(5);
                label.set_margin_bottom(5);
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&label));
                // qdata 将时间戳直接绑定到对应 GTK 行.
                unsafe {
                    row.set_data("lyric-timestamp", lyric.at);
                }
                lyrics_box.append(&row);
            }
        }
    } else {
        let label = gtk::Label::new(Some("文件中没有歌词"));
        label.add_css_class("dim-label");
        lyrics_box.append(&label);
    }

    if let Some(data) = content.cover.as_deref() {
        let loader = PixbufLoader::new();
        if loader.write(data).is_ok() && loader.close().is_ok() {
            builder.object::<gtk::Picture>(names::COVER).unwrap_or_else(|| panic!("Blueprint object '{}' is missing", names::COVER)).set_pixbuf(loader.pixbuf().as_ref());
        } else {
            builder.object::<gtk::Picture>(names::COVER).unwrap_or_else(|| panic!("Blueprint object '{}' is missing", names::COVER)).set_pixbuf(None);
        }
    } else {
        builder.object::<gtk::Picture>(names::COVER).unwrap_or_else(|| panic!("Blueprint object '{}' is missing", names::COVER)).set_pixbuf(None);
    }
}


/// 确保指定播放列表行完整出现在可视区域内.
///
/// 行已经完全可见时保持当前滚动位置,
/// 行不可见或仅部分可见时执行最小距离滚动,
/// 直到靠近视图的一侧刚好完整显示.
pub(super) fn ensure_playlist_row_visible(playlist_box: gtk::ListBox, target_index: usize) {
    glib::idle_add_local_once(move || {

        let Some(row) = playlist_box.row_at_index(target_index as i32) else { return;}; // 获取目标行控件
        let Some(adjustment) = playlist_box
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>()
            .map(|scroll| scroll.vadjustment())
        else { return; }; // 获取垂直滚动条的调整器

        let visible_top = adjustment.value(); // 当前可视区域顶部位置
        let visible_bottom = visible_top + adjustment.page_size(); // 当前可视区域底部位置

        let row_top = row.allocation().y() as f64; // 目标行顶部位置
        let row_bottom = row_top + row.allocation().height() as f64; // 目标行底部位置

        if row_top >= visible_top && row_bottom <= visible_bottom { return; } // 如果目标行已经完全可见, 不需要滚动

        // 计算滚动条的最大值, 确保不会滚动超出范围:
        // 公式: 最大滚动值 = 滚动条上限(内容总高度) - 可视区域高度, 并且不能小于滚动条下限.
        // 表示视口滚动到底部时的滚动位置.
        // 例如内容高 1000, 可视区域高 300, 最大滚动位置是 1000 - 300 = 700, 滚动条范围是 [0, 700].
        let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());

        // 按目标行更接近的边缘对齐,使部分可见时也只滚动必要距离.
        let target = if row_top < visible_top {
            row_top // 目标行顶部
        } else {
            row_bottom - adjustment.page_size() // 目标行底部减去可视区域高度, 使其顶部对齐可视区域底部
        };

        adjustment.set_value(target.clamp(adjustment.lower(), upper)); // 设置滚动条的新值, 确保在滚动范围内
    });
}


/// 确保指定的多个播放列表行完整出现在可视区域内.
///
/// 行已经完全可见时保持当前滚动位置,
/// 行不可见或仅部分可见时执行最小距离滚动,
/// 直到靠近视图的一侧刚好完整显示.
pub(super) fn ensure_playlist_row_visible_mut(playlist_box: gtk::ListBox, target_index: Vec<usize>) {
glib::idle_add_local_once(move || {

    // 获取索引最小的行, 如果 None 就返回
    let Some(min_index) = target_index.iter().min().copied() else { return; };
    let Some(first_row) = playlist_box.row_at_index(min_index as i32) else { return; };

    let Some(max_index) = target_index.iter().max().copied() else { return; };
    let Some(last_row) = playlist_box.row_at_index(max_index as i32) else { return; };

    // 计算目标行的顶部和底部位置
    let range_top = first_row.allocation().y() as f64;
    let range_bottom = last_row.allocation().y() as f64 + last_row.allocation().height() as f64;

    let Some(adjustment) = playlist_box
    .ancestor(gtk::ScrolledWindow::static_type())
    .and_downcast::<gtk::ScrolledWindow>()
    .map(|scroll| scroll.vadjustment())
    else { return; };

    let visible_top = adjustment.value();
    let visible_bottom = visible_top + adjustment.page_size();
    // let row_top = row.allocation().y() as f64;
    // let row_bottom = row_top + row.allocation().height() as f64;
    if range_top >= visible_top && range_bottom <= visible_bottom { return; } // 如果目标行已经完全可见, 不需要滚动

    // 按目标行更接近的边缘对齐,使部分可见时也只滚动必要距离.
    let target = if range_top < visible_top {
        range_top
    } else {
        range_bottom - adjustment.page_size()
    };

    let upper = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    adjustment.set_value(target.clamp(adjustment.lower(), upper));

});}
