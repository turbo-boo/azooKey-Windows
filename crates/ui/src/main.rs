use std::time::{Duration, Instant};

use anyhow::Context as _;
use azookey_server::TonicNamedPipeServer;
use ipc::{WindowAction, WindowController, WindowService};
use shared::{proto::window_service_server::WindowServiceServer, AppConfig};
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::platform::windows::{EventLoopBuilderExtWindows, WindowExtWindows};
use tao::{
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::Window,
};
use tokio::sync::mpsc;
use tonic::transport::Server;
use uiaccess::run_with_uiaccess;
use utils::{monitor_work_area, place_at_caret, Placement};
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE,
};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE},
};

pub mod candidate;
pub mod indicator;
pub mod ipc;
pub mod uiaccess;
pub mod utils;

/// How long the mode indicator stays after the input mode changes.
const INDICATOR_DURATION: Duration = Duration::from_millis(500);

/// Candidate window width in CSS pixels when the candidates are short.
const MIN_CANDIDATE_WIDTH: f64 = 225.0;
/// Distance in CSS pixels from the window's left edge to the candidate text
/// (body and main padding), which is aligned with the caret.
const CANDIDATE_OFFSET_X: f64 = 15.0;

#[derive(Debug)]
pub enum UserEvent {
    UpdateHeight(i32),
    /// Place the candidate window again after a scale factor change.
    Relayout,
    /// The candidate page finished loading.
    CandidatePageReady,
    WindowAction(WindowAction),
}

/// What the candidate window's placement depends on. The size is kept in CSS
/// (logical) pixels and converted with the scale factor of the monitor the
/// window goes to.
struct CandidateLayout {
    caret: Option<RECT>,
    width: f64,
    height: f64,
}

impl CandidateLayout {
    fn placement(&self, window: &Window) -> Placement {
        let caret = self.caret.unwrap_or_else(|| {
            let position = window.outer_position().unwrap_or_default();
            RECT {
                left: position.x,
                top: position.y,
                right: position.x,
                bottom: position.y,
            }
        });
        let (work, scale) = monitor_work_area(caret);
        place_at_caret(
            caret,
            work,
            scale,
            self.width,
            self.height,
            CANDIDATE_OFFSET_X,
        )
    }

    fn apply(&self, window: &Window) {
        let placement = self.placement(window);
        unsafe {
            let _ = SetWindowPos(
                HWND(window.hwnd() as *mut std::ffi::c_void),
                HWND_TOPMOST,
                placement.x,
                placement.y,
                placement.width,
                placement.height,
                SWP_NOACTIVATE,
            );
        }
    }
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
                    } else if type_value == "ready" {
                        proxy_clone
                            .send_event(UserEvent::CandidatePageReady)
                            .unwrap();
                    }
                }
            }
        })
        .build(&candidate_window)?;
    let mut candidate_theme = candidate::ThemeState::default();

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

    // The indicator is shown for a moment after the input mode changes. All
    // ShowWindow calls happen on this thread, so a hide cannot overtake a
    // show that is still pending.
    let mut indicator_hide_at: Option<Instant> = None;

    let mut candidate_layout = CandidateLayout {
        caret: None,
        width: MIN_CANDIDATE_WIDTH,
        height: candidate_window
            .inner_size()
            .to_logical::<f64>(candidate_window.scale_factor())
            .height,
    };
    let relayout_proxy = event_loop_proxy.clone();

    event_loop.run(move |event, _, control_flow| {
        let indicator_hwnd = indicator_window.hwnd();

        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            // Moving the window onto a monitor with another scale factor
            // makes tao resize it to keep its logical size and move it to
            // where Windows suggests. Give it the size computed for that
            // monitor and place it again once tao is done.
            Event::WindowEvent {
                window_id,
                event: WindowEvent::ScaleFactorChanged { new_inner_size, .. },
                ..
            } if window_id == candidate_window.id() => {
                let placement = candidate_layout.placement(&candidate_window);
                *new_inner_size =
                    PhysicalSize::new(placement.width as u32, placement.height as u32);
                let _ = relayout_proxy.send_event(UserEvent::Relayout);
            }
            Event::UserEvent(script) => match script {
                UserEvent::UpdateHeight(height) => {
                    candidate_layout.height = f64::from(height);
                    candidate_layout.apply(&candidate_window);
                }
                UserEvent::Relayout => candidate_layout.apply(&candidate_window),
                UserEvent::CandidatePageReady => {
                    candidate_theme.forget_applied();
                    candidate_theme.apply(&candidate_webview, &AppConfig::read_cached().theme);
                }
                UserEvent::WindowAction(action) => {
                    match action {
                        WindowAction::Show => {
                            candidate_theme
                                .apply(&candidate_webview, &AppConfig::read_cached().theme);

                            // if mode indicator is already shown, hide it
                            indicator_hide_at = None;
                            let _ = unsafe {
                                ShowWindow(HWND(indicator_hwnd as *mut std::ffi::c_void), SW_HIDE)
                            };

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
                            candidate_layout.caret = Some(RECT {
                                left,
                                top,
                                right,
                                bottom,
                            });
                            candidate_layout.apply(&candidate_window);

                            unsafe {
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
                                .unwrap_or(0);
                            candidate_layout.width =
                                MIN_CANDIDATE_WIDTH.max(120.0 + max_len as f64 * 18.0);
                            candidate_layout.apply(&candidate_window);

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
                            let _ = unsafe {
                                ShowWindow(
                                    HWND(indicator_hwnd as *mut std::ffi::c_void),
                                    SW_SHOWNOACTIVATE,
                                )
                            };
                            indicator_hide_at = Some(Instant::now() + INDICATOR_DURATION);
                        }
                    }
                }
            },
            _ => (),
        }

        if matches!(*control_flow, ControlFlow::ExitWithCode(_)) {
            return;
        }
        if indicator_hide_at.is_some_and(|deadline| Instant::now() >= deadline) {
            indicator_hide_at = None;
            let _ = unsafe { ShowWindow(HWND(indicator_hwnd as *mut std::ffi::c_void), SW_HIDE) };
        }
        *control_flow = match indicator_hide_at {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        };
    });
}
