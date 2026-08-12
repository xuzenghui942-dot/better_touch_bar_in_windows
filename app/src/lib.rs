//! Desktop shell for the Rust three-finger drag implementation.

pub mod external_links;
pub mod startup;
pub mod tray_icon;

mod app_catalog;
mod app_state;
mod commands;

use std::{sync::mpsc, thread, time::Duration};

use app_state::AppState;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, RunEvent, WindowEvent,
};
use three_finger_drag_core::{
    settings::PendingStartupAction,
    win32::{BackendEvent, InputService, TouchpadRuntimeStatus},
};

pub fn run() {
    let arguments: Vec<String> = std::env::args().collect();
    let autostart = arguments.iter().any(|argument| argument == "--autostart");
    let no_elevate = arguments.iter().any(|argument| argument == "--no-elevate");
    if arguments
        .iter()
        .any(|argument| argument == "--elevated-restart")
    {
        thread::sleep(Duration::from_millis(750));
    }

    let state = match AppState::load() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("Unable to initialize settings: {error}");
            return;
        }
    };
    let initial_settings = state.settings();
    let requires_administrator = initial_settings.run_elevated
        || initial_settings.pending_startup_action != PendingStartupAction::None;
    if requires_administrator
        && !startup::is_administrator()
        && !no_elevate
        && startup::request_elevated_restart().is_ok()
    {
        return;
    }

    if startup::is_administrator() {
        let mut settings = state.settings();
        match startup::apply_pending_action(&mut settings) {
            Ok(()) => {
                let _ = state.update_settings(|stored| *stored = settings);
            }
            Err(error) => state
                .logger()
                .record(format!("Unable to apply pending startup action: {error}")),
        }
    }
    if let Err(error) = startup::refresh_current_startup(&state.settings()) {
        state
            .logger()
            .record(format!("Unable to refresh startup registration: {error}"));
    }
    let advanced = state.advanced_settings();
    if let Err(error) =
        startup::refresh_advanced_startup(advanced.launch_at_login, state.settings().run_elevated)
    {
        state.logger().record(format!(
            "Unable to refresh advanced startup registration: {error}"
        ));
    }

    let state_for_setup = state.clone();
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            |app, _arguments, _working_directory| {
                show_main_window(app);
            },
        ))
        .manage(state.clone())
        .setup(move |app| {
            let icon = application_icon();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_icon(icon.clone());
            }
            build_tray(app, icon)?;

            let (event_sender, event_receiver) = mpsc::channel();
            match InputService::start_with_advanced(
                state_for_setup.settings_handle(),
                state_for_setup.advanced_settings_handle(),
                state_for_setup.logger(),
                event_sender,
            ) {
                Ok(service) => state_for_setup.install_input_service(service),
                Err(error) => {
                    let message = format!("触摸板服务初始化失败：{error}");
                    state_for_setup.logger().record(&message);
                    let status = BackendEvent::Status(TouchpadRuntimeStatus {
                        initialized: true,
                        touchpad_exists: false,
                        receiver_installed: false,
                        devices: Vec::new(),
                    });
                    state_for_setup.handle_backend_event(&status);
                    let _ = app.emit("touchpad-event", status);
                    let _ = app.emit("touchpad-event", BackendEvent::Error(message));
                }
            }

            let event_app = app.handle().clone();
            let event_state = state_for_setup.clone();
            thread::Builder::new()
                .name("ThreeFingerDrag UI events".into())
                .spawn(move || {
                    while let Ok(event) = event_receiver.recv() {
                        event_state.handle_backend_event(&event);
                        let _ = event_app.emit("touchpad-event", &event);
                    }
                })?;

            if !autostart {
                show_main_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::save_gesture_settings,
            commands::save_advanced_settings,
            commands::restore_advanced_defaults,
            commands::list_installed_apps,
            commands::list_running_apps,
            commands::advanced_diagnostics,
            commands::set_record_logs,
            commands::set_run_at_startup,
            commands::set_run_elevated,
            commands::save_logs,
            commands::open_touchpad_settings,
            commands::open_external,
            commands::close_settings,
            commands::quit_app,
        ])
        .build(tauri::generate_context!());

    let Ok(application) = application else {
        eprintln!("Unable to build the desktop application.");
        state.stop_input_service();
        return;
    };
    application.run(move |_app, event| {
        if let RunEvent::Exit = event {
            state.stop_input_service();
        }
    });
}

fn build_tray(app: &tauri::App, icon: Image<'static>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出应用", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    TrayIconBuilder::new()
        .icon(icon)
        .tooltip("单击以打开三指拖动设置。")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => quit_application(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn application_icon() -> Image<'static> {
    Image::new_owned(tray_icon::create_tray_rgba(32), 32, 32)
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn quit_application(app: &tauri::AppHandle) {
    app.state::<AppState>().stop_input_service();
    app.exit(0);
}
