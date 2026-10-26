use std::cmp::max;
use std::sync::Arc;

use anyhow::Context as _;
use azookey_server::TonicNamedPipeServer;
use ipc::{WindowAction, WindowController, WindowService};
use shared::{proto::window_service_server::WindowServiceServer, AppConfig};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowExtWindows};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tonic::transport::Server;
use uiaccess::run_with_uiaccess;
use utils::get_candidate_window_position;
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE},
};

pub mod candidate;
pub mod indicator;
pub mod ipc;
pub mod uiaccess;
pub mod utils;

#[derive(Debug)]
pub enum UserEvent {
    UpdateHeight(i32),
    WindowAction(WindowAction),
}

fn webview_data_directory() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Azookey")
        .join("WebView2")
}

fn main() -> anyhow::Result<()> {
    // Without UIAccess the windows still work for normal applications; they
    // just cannot be shown above elevated or immersive ones.
    match run_with_uiaccess() {
        Ok(Some(exit_code)) => std::process::exit(exit_code as i32),
        Ok(None) => {}
        Err(error) => eprintln!("Running without UIAccess: {error:#}"),
    }
    run()
}

#[tokio::main]
async fn run() -> anyhow::Result<()> {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event()
        .with_any_thread(true)
        .build();

    // initialize window controller
    let (tx, mut rx) = mpsc::channel(32);
    let window_controller = WindowController::new(tx.clone());
    let grpc_service = WindowService {
        controller: window_controller.clone(),
    };

    // start grpc server
    let incoming = TonicNamedPipeServer::new(shared::pipe::UI_PIPE)?;
    tokio::spawn(async move {
        println!("WindowServer listening");
        let result = Server::builder()
            .add_service(WindowServiceServer::new(grpc_service))
            .serve_with_incoming(incoming)
            .await;
        // The server also returns Ok when the pipe stream ends, e.g. because
        // another ui.exe owns the pipe. Without its pipe this process can
        // never be reached again; exit so that the launcher restarts it.
        match result {
            Ok(()) => eprintln!("The window service stopped accepting connections"),
            Err(error) => eprintln!("The window service failed: {error}"),
        }
        std::process::exit(1);
    });

    let event_loop_proxy = event_loop.create_proxy();
    let task_guard: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));

    // WebView2 defaults its user data folder to "<exe>.WebView2" next to the
    // executable, which is not writable under Program Files.
    let mut web_context = wry::WebContext::new(Some(webview_data_directory()));

    let proxy_clone = event_loop_proxy.clone();
    let candidate_window = candidate::create_candidate_window(&event_loop)?;
    let candidate_webview_builder = candidate::create_candidate_webview(&mut web_context)?;
    let candidate_webview = candidate_webview_builder
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |message| {
            if let Ok(message) = serde_json::from_str::<serde_json::Value>(message.body()) {
                if let Some(type_value) = message.get("type") {
                    if type_value == "resize" {
                        if let Some(height) = message.get("height") {
                            let height = height.as_f64().unwrap_or(0.0);
                            proxy_clone
                                .send_event(UserEvent::UpdateHeight(height as i32))
                                .unwrap();
                        }
                    }
                }
            }
        })
        .build(&candidate_window)?;

    let theme_json = candidate::theme_payload(&AppConfig::read().theme);
    candidate_webview.evaluate_script(&format!("applyTheme({})", theme_json))?;

    let indicator_window = indicator::create_indicator_window(&event_loop)?;
    let indicator_webview =
        indicator::create_indicator_webview(&indicator_window, &mut web_context)?;

    // handle window actions
    let proxy_clone = event_loop_proxy.clone();
    tokio::spawn(async move {
        while let Some(action) = rx.recv().await {
            match action {
                WindowAction::Show => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::Show))
                        .unwrap();
                }
                WindowAction::Hide => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::Hide))
                        .unwrap();
                }
                WindowAction::SetPosition {
                    top,
                    left,
                    bottom,
                    right,
                } => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::SetPosition {
                            top,
                            left,
                            bottom,
                            right,
                        }))
                        .unwrap();
                }
                WindowAction::SetCandidate { candidates } => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::SetCandidate {
                            candidates,
                        }))
                        .unwrap();
                }
                WindowAction::SetSelection { index } => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::SetSelection {
                            index,
                        }))
                        .unwrap();
                }
                WindowAction::SetInputMode(input_method) => {
                    proxy_clone
                        .send_event(UserEvent::WindowAction(WindowAction::SetInputMode(
                            input_method,
                        )))
                        .unwrap();
                }
            }
        }
    });

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        let indicator_hwnd = indicator_window.hwnd();

        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(script) => match script {
                UserEvent::UpdateHeight(height) => {
                    let width = candidate_window.inner_size().width as i32;
                    candidate_window.set_inner_size(LogicalSize::new(width, height));
                }
                UserEvent::WindowAction(action) => {
                    match action {
                        WindowAction::Show => {
                            let theme_json = candidate::theme_payload(&AppConfig::read().theme);
                            if let Err(error) = candidate_webview
                                .evaluate_script(&format!("applyTheme({})", theme_json))
                            {
                                eprintln!("Failed to apply candidate theme: {error}");
                            }

                            // if mode indicator is already shown, hide it
                            let mut task_guard = match task_guard.try_lock() {
                                Ok(guard) => guard,
                                Err(_) => {
                                    eprintln!(
                                        "Warning: Failed to lock task_guard, skipping cleanup"
                                    );
                                    return;
                                }
                            };
                            if let Some(task) = task_guard.take() {
                                task.abort();
                                let _ = unsafe {
                                    ShowWindow(
                                        HWND(indicator_hwnd as *mut std::ffi::c_void),
                                        SW_HIDE,
                                    )
                                };
                            }

                            let _ = unsafe {
                                ShowWindow(
                                    HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
                                    SW_SHOWNOACTIVATE,
                                )
                            };
                        }
                        WindowAction::Hide => {
                            let _ = unsafe {
                                ShowWindow(
                                    HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
                                    SW_HIDE,
                                )
                            };
                        }
                        WindowAction::SetPosition {
                            top,
                            left,
                            bottom,
                            right,
                        } => {
                            let (x, y) = get_candidate_window_position(
                                top,
                                left,
                                bottom,
                                right,
                                &candidate_window,
                            );

                            unsafe {
                                let _ = SetWindowPos(
                                    HWND(candidate_window.hwnd() as *mut std::ffi::c_void),
                                    HWND_TOPMOST,
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );

                                let _ = SetWindowPos(
                                    HWND(indicator_hwnd as *mut std::ffi::c_void),
                                    HWND_TOPMOST,
                                    0,
                                    0,
                                    0,
                                    0,
                                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                );
                            }
                            candidate_window
                                .set_outer_position(PhysicalPosition::new(x as f64, y as f64));
                            indicator_window.set_outer_position(PhysicalPosition::new(
                                (left - 45) as f64,
                                bottom as f64,
                            ));
                        }
                        WindowAction::SetCandidate { candidates } => {
                            let max_len = candidates
                                .iter()
                                .map(|s| s.chars().count())
                                .max()
                                .unwrap_or(0) as u32;

                            let height = candidate_window.inner_size().height as i32;
                            candidate_window.set_inner_size(PhysicalSize::new(
                                max(225, 120 + max_len * 18),
                                height as u32,
                            ));

                            let candidates = serde_json::to_string(&candidates)
                                .context("Failed to serialize candidates")
                                .unwrap();

                            // Apply right away: re-posting it would put it
                            // behind a Show that is already queued, and the
                            // window would appear with the previous list.
                            candidate_webview
                                .evaluate_script(&format!("updateCandidates({})", candidates))
                                .unwrap();
                        }
                        WindowAction::SetSelection { index } => {
                            candidate_webview
                                .evaluate_script(&format!("updateSelection({})", index))
                                .unwrap();
                        }
                        WindowAction::SetInputMode(input_method) => {
                            indicator_webview
                                .evaluate_script(&indicator::input_mode_script(&input_method))
                                .unwrap();
                            let task_guard = task_guard.try_lock();

                            if let Ok(mut task_guard) = task_guard {
                                if let Some(task) = task_guard.take() {
                                    task.abort();
                                }

                                *task_guard = Some(tokio::spawn(async move {
                                    let _ = unsafe {
                                        ShowWindow(
                                            HWND(indicator_hwnd as *mut std::ffi::c_void),
                                            SW_SHOWNOACTIVATE,
                                        )
                                    };
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                    let _ = unsafe {
                                        ShowWindow(
                                            HWND(indicator_hwnd as *mut std::ffi::c_void),
                                            SW_HIDE,
                                        )
                                    };
                                }));
                            }
                        }
                    }
                }
            },
            _ => (),
        }
    });
}
