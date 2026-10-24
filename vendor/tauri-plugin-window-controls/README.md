# Native Window Controls for Tauri

<img width="1102" height="810" alt="hero image" src="https://github.com/user-attachments/assets/c15681f2-3622-40e8-87ca-ea6f997d8951" />

Native Windows caption controls for borderless Tauri windows, including Windows 11 Snap Layout support.

## Highlights

- **Native rendering and input**: minimize, maximize/restore, and close are rendered in Win32 child windows above WebView2.
- **Native Snap Layouts**: the maximize region returns `HTMAXBUTTON`, allowing Windows 11 to provide its standard Snap Layout flyout.
- **System glyphs**: controls use Segoe Fluent Icons with Segoe MDL2 Assets as the Windows 10 fallback.
- **DPI and window-state aware**: geometry follows move, resize, maximize, DPI, theme, activation, and fullscreen changes.
- **No frontend integration**: the plugin injects no HTML, CSS, JavaScript, commands, or permissions.

## Install

```toml
[dependencies]
tauri-plugin-window-controls = "0.2"
```

Register the plugin once:

```rust
tauri::Builder::default()
    .plugin(tauri_plugin_window_controls::init())
```

## Windows manifest

Layered child windows require the executable to declare Windows 8 or newer compatibility. Add a manifest to the Tauri application and load it from `build.rs`:

```rust
fn main() {
    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes).expect("failed to run Tauri build script");
}
```

```xml
<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}" />
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}" />
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
</assembly>
```

## Usage

Set the control height anywhere in the window-builder chain:

```rust
use tauri::{WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_window_controls::WindowControlsBuilderExt;

let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
    .visible(false)
    .transparent(true)
    .inner_size(960.0, 640.0)
    .window_controls_height(44)
    .build()?;

window.show()?;
```

The height is expressed in logical pixels. Each caption button is 46 logical pixels wide. The builder configures a borderless window with its native shadow intact.

Calling `window_controls_height` more than once is supported. The final value takes precedence.

Application content remains responsible for the title-bar layout and drag region:

```html
<header data-tauri-drag-region>
  <span data-tauri-drag-region>Application title</span>
  <button type="button">Application action</button>
</header>
```

The controls hide automatically in fullscreen mode and return when the normal caption style is restored.

## Platform behavior

On non-Windows targets, `window_controls_height` builds an ordinary Tauri window without native-control setup.

## License

[MIT](./LICENSE)
