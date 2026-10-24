//! Native Windows caption controls for Tauri windows.
//!
//! Windows-only by design: on every other target the builder extension builds
//! an ordinary Tauri window and [`init`] registers an empty plugin.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Runtime,
};

#[cfg(windows)]
use tauri::{Manager, RunEvent, WindowEvent};

mod builder;

#[cfg(windows)]
mod windows;

pub use builder::WindowControlsBuilderExt;

/// Initializes the plugin. Register it once on the application builder.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let builder = Builder::new("window-controls");

    #[cfg(windows)]
    let builder = builder.on_event(|app, event| {
        let RunEvent::WindowEvent { label, event, .. } = event else {
            return;
        };
        let WindowEvent::ThemeChanged(theme) = event else {
            return;
        };
        if let Some(window) = app.get_webview_window(label) {
            let _ = windows::set_theme(&window, *theme);
        }
    });

    builder.build()
}
