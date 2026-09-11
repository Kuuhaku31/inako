
// src/gui/utils.rs
// 窗口相关的工具函数

use std::process::Command;

use serde_json::Value;

use crate::models::WindowGeometry;
use crate::gui::APPLICATION_ID;


// Hyprland 窗口交互.

// GTK4/Wayland 协议不允许应用查询自己在屏幕上的绝对位置,
// 因此这里通过 hyprctl 和窗口 pid 查询/修改窗口几何信息.
// 窗口作为普通顶层窗口交由 Hyprland 管理为可拖拽/可调整大小的浮动窗口.
//
// 目标环境的 hyprctl 使用基于 Lua 的配置/分发系统:
// `hyprctl keyword` 接口不可用 ("keyword can't work with non-legacy parsers"),
// 所有分发/规则操作都统一通过 `hyprctl eval` 执行 Lua 代码.

/// 调用 hyprctl 查询 pid 对应窗口客户端当前几何信息.
pub(super) fn query_geometry() -> Option<WindowGeometry> {
    let output = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let clients: Value = serde_json::from_slice(&output.stdout).ok()?;
    let client = clients
        .as_array()?
        .iter()
        .find(|client| client.get("pid").and_then(Value::as_u64) == Some(std::process::id() as u64))?;
    let at = client.get("at")?.as_array()?;
    let size = client.get("size")?.as_array()?;
    Some(WindowGeometry::from_vector(at, size)?)
}

/// 在窗口映射之前下发一条持久的浮动规则, 让窗口一出现就是浮动状态并且
/// 直接落在保存的位置/大小上, 不再需要映射后再移动/调整大小.
///
/// Hyprland 支持按 class 匹配的持久窗口规则, 在窗口首次映射时就把
/// 浮动状态和几何信息一起应用, 从根源上避免窗口闪烁.
pub(super) fn ensure_floating_rule(geometry: Option<WindowGeometry>) {
    let class_pattern = format!("^({APPLICATION_ID})$");
    let extra_fields = geometry
        .map(|geometry| {
            format!(
                r#", size={{{}, {}}}, move={{{}, {}}}"#,
                geometry.width, geometry.height, geometry.x, geometry.y
            )
        })
        .unwrap_or_default();

    let _ = Command::new("hyprctl")
    .args([
        "eval",
        &format!(r#"hl.window_rule({{float=true{extra_fields}, match={{class="{class_pattern}"}}}})"#)
    ]).output();
}
