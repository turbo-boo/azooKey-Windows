#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use shared::AppConfig;
use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, thread};
use windows::core::w;
use windows::Win32::{
    Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::{CreateMutexW, ABOVE_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW},
};

/// A process that ran at least this long is considered to have worked, so
/// the next restart starts again with the shortest delay.
const STABLE_RUN: Duration = Duration::from_secs(60);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(60);

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
    // The launcher restarts the server and the UI whenever they exit. A
    // second launcher in the same session would start copies that cannot
    // get the pipes, exit and get restarted forever, so only one runs. The
    // handle stays open until the process exits.
    match unsafe { CreateMutexW(None, false, w!("Local\\Azookey.Launcher")) } {
        Ok(_) if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS => {}
        // Access is denied when the other launcher runs at a higher integrity
        // level; it exists either way.
        _ => {
            eprintln!("Another launcher is already running in this session");
            return Ok(());
        }
    }

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

    let supervisors =
        [("azookey-server.exe", "[server]"), ("ui.exe", "[ui]")].map(|(exe, prefix)| {
            let base_dir = exe_path.clone();
            thread::spawn(move || supervise(&base_dir, exe, prefix))
        });
    for supervisor in supervisors {
        if supervisor.join().is_err() {
            anyhow::bail!("a process supervisor panicked");
        }
    }
    Ok(())
}

/// Keeps `exe` running: the converter or the candidate window can crash, and
/// nothing else would start them again before the next logon. Restarts are
/// delayed more and more while the process keeps failing quickly.
fn supervise(base_dir: &Path, exe: &str, prefix: &str) {
    let mut quick_failures = 0;
    loop {
        let started = Instant::now();
        match start_process(base_dir, exe, prefix) {
            Ok(mut child) => match child.wait() {
                Ok(status) => eprintln!("{prefix}: exited with {status}"),
                Err(error) => {
                    eprintln!("{prefix}: cannot wait for the process: {error}");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            },
            Err(error) => eprintln!("{prefix}: failed to start {exe}: {error}"),
        }

        if started.elapsed() >= STABLE_RUN {
            quick_failures = 0;
        }
        let delay = restart_delay(quick_failures);
        quick_failures += 1;
        eprintln!("{prefix}: restarting in {} s", delay.as_secs());
        thread::sleep(delay);
    }
}

/// 1 s after the first quick failure, doubling up to [`MAX_RESTART_DELAY`].
fn restart_delay(quick_failures: u32) -> Duration {
    Duration::from_secs(1)
        .checked_mul(1 << quick_failures.min(31))
        .map_or(MAX_RESTART_DELAY, |delay| delay.min(MAX_RESTART_DELAY))
}

fn opt_out_of_power_throttling(child: &Child) {
    if let Err(error) = shared::opt_out_of_power_throttling(HANDLE(child.as_raw_handle())) {
        eprintln!("Failed to disable power throttling: {error}");
    }
}

fn start_process(base_dir: &Path, exe: &str, prefix: &str) -> std::io::Result<Child> {
    let executable = base_dir.join(exe);
    let mut child = Command::new(&executable)
        .current_dir(base_dir)
        .creation_flags((CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY_CLASS).0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
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

    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::{restart_delay, select_backend_dir, MAX_RESTART_DELAY};
    use std::time::Duration;

    #[test]
    fn restart_delay_doubles_up_to_the_maximum() {
        assert_eq!(restart_delay(0), Duration::from_secs(1));
        assert_eq!(restart_delay(1), Duration::from_secs(2));
        assert_eq!(restart_delay(5), Duration::from_secs(32));
        assert_eq!(restart_delay(6), MAX_RESTART_DELAY);
        assert_eq!(restart_delay(u32::MAX), MAX_RESTART_DELAY);
    }

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
