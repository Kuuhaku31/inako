
// src/gui/widget_names.rs
// 集中保存 window.blp 中被 Rust 代码按 id 查找的全部控件名称.
//
// 任何需要从 GUI/Builder 取控件的地方都必须通过 GUI::object /
// GUI::with_builder 等统一接口, 并引用本模块的常量, 禁止在调用处
// 直接手写字符串字面量, 避免 id 拼写错误和与 window.blp 失去同步.

pub(super) const WINDOW      : &str = "window";
pub(super) const COVER       : &str = "cover";
pub(super) const TITLE_LABEL : &str = "title_label";
pub(super) const TIME_LABEL  : &str = "time_label";
pub(super) const PROGRESS_SCALE : &str = "progress_scale";
pub(super) const VOLUME_SCALE : &str = "volume_scale";
pub(super) const LYRICS      : &str = "lyrics";
pub(super) const PLAYLIST    : &str = "playlist";

pub(super) const BUTTON_PLAY     : &str = "button_play";
pub(super) const BUTTON_PREVIOUS : &str = "button_previous";
pub(super) const BUTTON_NEXT     : &str = "button_next";
pub(super) const BUTTON_LOCATE   : &str = "button_locate";
pub(super) const BUTTON_SELECT_UP     : &str = "button_select_up";
pub(super) const BUTTON_SELECT_DOWN   : &str = "button_select_down";
pub(super) const BUTTON_SELECT_TOP    : &str = "button_select_top";
pub(super) const BUTTON_SELECT_BOTTOM : &str = "button_select_bottom";
pub(super) const BUTTON_CURRENT_TO_SELECTED_UP   : &str = "button_current_to_selected_up";
pub(super) const BUTTON_SELECTED_TO_CURRENT_DOWN : &str = "button_selected_to_current_down";
pub(super) const BUTTON_REFRESH_PLAYLIST         : &str = "button_refresh_playlist";
pub(super) const BUTTON_TOGGLE_PLAYLIST_DISPLAY  : &str = "button_toggle_playlist_display";
