
// src/bin/inako-dae.rs
// 守护进程可执行文件入口

use inako::daemon;
use std::env;


fn main() {
    let result = parse_command().and_then(daemon::run_socket);
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}


/// 解析守护进程的命令行参数并返回可选的 Unix Socket 路径
fn parse_command() -> Result<Option<String>, String> {
    let mut arguments = env::args().skip(1);
    let mut socket = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "start" => {}
            "--socket" | "--socket-path" => {
                socket = Some(arguments.next().ok_or("--socket 需要路径参数")?);
            }
            "help" | "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "version" | "--version" | "-V" => {
                print_version();
                std::process::exit(0);
            }
            _ => {
                return Err(format!(
                    "未知参数: {argument}\n运行 inako-dae --help 查看用法"
                ));
            }
        }
    }
    if socket.as_deref().is_some_and(str::is_empty) {
        return Err("socket 路径不能为空".to_string());
    }
    Ok(socket)
}

/// Print daemon-specific usage text.
fn print_help() {
    println!(
        "inako-dae {}\n\n用法:\n  inako-dae [选项]\n\n选项:\n  --socket <路径>                     指定守护进程 Unix Socket 路径\n  --help                              显示帮助信息\n  --version                           显示版本信息",
        env!("CARGO_PKG_VERSION")
    );
}

/// Print the daemon version.
fn print_version() {
    println!("inako-dae {}", env!("CARGO_PKG_VERSION"));
}
