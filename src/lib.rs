
// src/lib.rs
// 公共库模块, 被 daemon, CLI 和 GTK 窗口可执行文件共享.

// 每个模块只在对应的 Cargo feature 激活时才编译,
// 使三个可执行文件各自只链接自己需要的依赖
// (daemon 用 libmpv2, cli/gui 用 lofty, gui 额外用 gtk 系).
// 构建单个 bin 时 cargo 会按其 required-features 解析出最小依赖集,
// 避免例如窗口进程被迫加载仅 daemon 需要的 libmpv 系共享库.
#[cfg(feature = "media")]
pub mod cli;

#[cfg(feature = "daemon")]
pub mod daemon;

#[cfg(feature = "gui")]
pub mod gui;

#[cfg(any(feature = "daemon", feature = "media", feature = "gui", test))]
mod models;

#[cfg(any(feature = "daemon", feature = "media", feature = "gui", test))]
mod utils;
