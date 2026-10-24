use tauri::{Manager, Runtime, WebviewWindowBuilder};

#[cfg(windows)]
use tauri::webview::PageLoadEvent;

/// Adds native Windows caption controls to a Tauri webview-window builder.
pub trait WindowControlsBuilderExt: Sized {
    /// Enables native caption controls with the given logical-pixel height.
    ///
    /// Repeated calls override the height; the final call takes precedence.
    fn window_controls_height(self, height: u32) -> Self;
}

impl<R: Runtime, M: Manager<R>> WindowControlsBuilderExt for WebviewWindowBuilder<'_, R, M> {
    fn window_controls_height(self, height: u32) -> Self {
        #[cfg(windows)]
        {
            let height = height.max(1);
            self.decorations(false)
                .shadow(true)
                .on_page_load(move |window, payload| {
                    if payload.event() == PageLoadEvent::Started {
                        let _ = crate::windows::install(&window, height);
                    }
                })
        }

        #[cfg(not(windows))]
        {
            let _ = height;
            self
        }
    }
}
