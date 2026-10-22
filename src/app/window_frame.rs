//! Windows: the system title bar's icon and colours.
//!
//! winit doesn't give the window the icon embedded in the exe (build.rs),
//! so the title bar showed a blank one; it's set here with `WM_SETICON`,
//! as Nebula Reader does. The title bar's colours follow the app's theme:
//! dark mode (Windows 10 2004 and later), and on Windows 11 the exact
//! background and text colours of the header bar below it, so the two
//! read as one bar. Older Windows ignores what it doesn't know.

/// What the title bar should look like: dark or light, and its caption
/// and text colours as 0x00BBGGRR (Windows' COLORREF).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub dark: bool,
    pub caption: u32,
    pub text: u32,
}

impl Frame {
    pub fn of(theme: &cosmic::Theme) -> Self {
        let cosmic = theme.cosmic();
        // Opaque: a native frame means the window isn't transparent.
        let background = cosmic.background(false);
        Frame { dark: cosmic.is_dark, caption: colorref(background.base), text: colorref(background.on) }
    }
}

fn colorref(c: cosmic::cosmic_theme::palette::Srgba) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(c.red) | byte(c.green) << 8 | byte(c.blue) << 16
}

/// Sets the icon (once) and the title bar colours on the live window.
#[cfg(windows)]
pub fn apply<T: Send + 'static>(id: cosmic::iced::window::Id, frame: Frame, icon: bool) -> cosmic::iced::Task<T> {
    use cosmic::iced::window;
    use cosmic::iced::window::raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, LoadImageW, SendMessageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON,
        SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_SETICON,
    };

    window::run_with_handle(id, move |handle| {
        let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
        let hwnd = win.hwnd.get() as *mut core::ffi::c_void;
        // SAFETY: plain Win32 calls on our own live window, with the icon
        // resource build.rs embedded in this exe and attribute values of
        // the sizes DWM documents (BOOL and COLORREF, both 4 bytes).
        unsafe {
            if icon {
                let module = GetModuleHandleW(core::ptr::null());
                for (which, cx, cy) in [(ICON_SMALL, SM_CXSMICON, SM_CYSMICON), (ICON_BIG, SM_CXICON, SM_CYICON)] {
                    let icon = LoadImageW(
                        module,
                        1 as *const u16, // MAKEINTRESOURCEW(1), the exe's icon
                        IMAGE_ICON,
                        GetSystemMetrics(cx),
                        GetSystemMetrics(cy),
                        LR_DEFAULTCOLOR,
                    );
                    if !icon.is_null() {
                        SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
                    }
                }
            }
            let set = |attribute: i32, value: u32| {
                DwmSetWindowAttribute(hwnd, attribute as u32, (&value as *const u32).cast(), 4)
            };
            // Windows 10 before 20H1 knew dark mode as attribute 19.
            if set(DWMWA_USE_IMMERSIVE_DARK_MODE, frame.dark as u32) != 0 {
                set(19, frame.dark as u32);
            }
            set(DWMWA_CAPTION_COLOR, frame.caption);
            set(DWMWA_TEXT_COLOR, frame.text);
        }
    })
    .discard()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_bgr() {
        let c = cosmic::cosmic_theme::palette::Srgba::new(1.0, 0.5, 0.0, 1.0);
        assert_eq!(colorref(c), 0x00_00_80_FF);
    }
}
