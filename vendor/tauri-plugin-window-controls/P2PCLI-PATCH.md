# P2PCLI window controls patch

Upstream: https://github.com/insd47/tauri-plugin-window-controls
Base: crates.io tauri-plugin-window-controls 0.2.1. Original MIT license retained.

Local changes in src/windows.rs:
- Preserve the antialiased GDI coverage instead of squaring it with gamma 2.0.
- Keep enabled icons at alpha 255 regardless of focus (upstream inactive: 82); disabled controls remain 92. Remove the foreground-window dependency from rendering.
- Shorten SUBCLASS_ID to a 32-bit value (upstream 0x7462_6f63_6170 overflows usize on i686, which fails the 32-bit Windows build).

The Win32 child windows, Segoe system glyphs, DPI sizing, hit testing,
Snap Layout support and system window commands are unchanged.
Cargo.toml at the repository root selects this copy with patch.crates-io.
Review this small diff before updating the upstream version.
