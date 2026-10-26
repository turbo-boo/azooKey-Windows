#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use shared::AppConfig;
use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::{env, thread};
use windows::Win32::{
    Foundation::HANDLE,
    System::Threading::{ABOVE_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW},
};

fn file_on_search_path(filename: &str) -> bool {
    env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .filter(|entry| !entry.is_empty())
        .map(std::path::PathBuf::from)
        .any(|path| path.join(filename).is_file())
}

fn select_backend_dir(configured: &str, cuda_available: bool, vulkan_available: bool) -> &'static str {
    match configured {
        "cuda" if cuda_available => "llama_cuda",
        "vulkan" if vulkan_available => "llama_vulkan",
        "cpu" => "llama_cpu",
        _ => "llama_cpu",
    }
}

fn main() -> anyhow::Result<()> {
    let config = AppConfig::new();

    let exe_path = env::current_exe()?.parent().unwrap().to_path_buf();
    let cuda_available =
        file_on_search_path("cudart64_12.dll") && file_on_search_path("cublas64_12.dll");
    let vulkan_available = file_on_search_path("vulkan-1.dll");
    let backend_dir =
        select_backend_dir(config.zenzai.backend.as_str(), cuda_available, vulkan_available);

    let backend_path = exe_path.join(backend_dir);
    let backend_path_str = backend_path.to_string_lossy();

    let mut new_path = env::var("PATH").unwrap_or_else(|_| String::new());
    new_path = format!("{};{}", backend_path_str, new_path);
    env::set_var("PATH", &new_path);

    let server_process = start_process(&exe_path, "azookey-server.exe", "[server]");
    let ui_process = start_process(&exe_path, "ui.exe", "[ui]");

    if let (Some(mut server), Some(mut ui)) = (server_process, ui_process) {
        let server_handle = thread::spawn(move || server.wait());
        let ui_handle = thread::spawn(move || ui.wait());

        let _ = server_handle.join();
        let _ = ui_handle.join();
    }

    Ok(())
}

fn opt_out_of_power_throttling(child: &Child) {
    if let Err(error) = shared::opt_out_of_power_throttling(HANDLE(child.as_raw_handle())) {
        eprintln!("Failed to disable power throttling: {error}");
    }
}

fn start_process(base_dir: &std::path::Path, exe: &str, prefix: &str) -> Option<Child> {
    let executable = base_dir.join(exe);
    let mut child = Command::new(&executable)
        .current_dir(base_dir)
        .creation_flags((CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY_CLASS).0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("Failed to start {}: {error}", executable.display()));
    opt_out_of_power_throttling(&child);

    let stdout = child.stdout.take().expect("Failed to capture stdout");
    let stdout_reader = BufReader::new(stdout);
    let prefix_stdout = prefix.to_string();
    thread::spawn(move || {
        for line in stdout_reader.lines() {
            if let Ok(line) = line {
                println!("{}: {}", prefix_stdout, line);
            }
        }
    });

    let stderr = child.stderr.take().expect("Failed to capture stderr");
    let stderr_reader = BufReader::new(stderr);
    let prefix_stderr = prefix.to_string();
    thread::spawn(move || {
        for line in stderr_reader.lines() {
            if let Ok(line) = line {
                eprintln!("{}: {}", prefix_stderr, line);
            }
        }
    });

    Some(child)
}


#[cfg(test)]
mod tests {
    use super::select_backend_dir;

    #[test]
    fn vulkan_is_used_when_runtime_is_available() {
        assert_eq!(select_backend_dir("vulkan", false, true), "llama_vulkan");
    }

    #[test]
    fn unavailable_accelerated_backend_falls_back_to_cpu() {
        assert_eq!(select_backend_dir("vulkan", false, false), "llama_cpu");
        assert_eq!(select_backend_dir("cuda", false, true), "llama_cpu");
    }

    #[test]
    fn cuda_is_used_only_when_required_runtime_is_available() {
        assert_eq!(select_backend_dir("cuda", true, true), "llama_cuda");
    }

    #[test]
    fn unknown_backend_falls_back_to_cpu() {
        assert_eq!(select_backend_dir("unknown", true, true), "llama_cpu");
    }
}
