#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
use dagaz_windows::InstanceGuard;
use gpui_kit::*;

fn main() {
    if let Err(error) = run() {
        #[cfg(windows)]
        unsafe {
            let message: Vec<u16> = format!("{error}\0").encode_utf16().collect();
            let title: Vec<u16> = "Dagaz\0".encode_utf16().collect();
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
            );
        }
        #[cfg(not(windows))]
        eprintln!("{error}");
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let profile = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .ok_or("LOCALAPPDATA is unavailable")?
        .join("Dagaz");
    let _instance = InstanceGuard::acquire(&profile)?;
    let runtime = std::sync::Arc::new(tokio::runtime::Runtime::new()?);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        gpui_kit::component::Theme::update(cx, |theme| {
            theme.colors.primary = rgb(0x9ce6c6).into();
            theme.colors.primary_foreground = rgb(0x14362c).into();
            theme.colors.button_primary = rgb(0x9ce6c6).into();
            theme.colors.button_primary_hover = rgb(0xb2eed5).into();
            theme.colors.button_primary_active = rgb(0x80cfae).into();
            theme.colors.button_primary_foreground = rgb(0x14362c).into();
        });
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1200.), px(780.)),
                cx,
            ))),
            window_min_size: Some(size(px(760.), px(580.))),
            ..Default::default()
        };
        let profile = profile.clone();
        let runtime = runtime.clone();
        gpui_kit::open_window(options, cx, move |window, cx| {
            window.set_window_title("Dagaz");
            cx.new(|cx| app::Dagaz::new(profile, runtime, window, cx))
        })
        .expect("failed to open Dagaz window");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
    Ok(())
}
