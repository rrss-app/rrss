use rrss_core::platform::{Os, WindowControls};

use crate::views::{WindowChromeView, WindowControlsStyle};

/// 主窗口的自绘标题栏：前端按这里画窗口三键。
pub fn window_chrome() -> WindowChromeView {
    window_chrome_for(Os::current(), rrss_core::platform::tray_available())
}

fn window_chrome_for(os: Os, tray: bool) -> WindowChromeView {
    let controls = match os.window_controls() {
        WindowControls::Windows => WindowControlsStyle::Windows,
        WindowControls::Gnome => WindowControlsStyle::Gnome,
    };
    WindowChromeView { controls, tray }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_names_the_control_style_per_platform() {
        assert_eq!(
            window_chrome_for(Os::Windows, true).controls,
            WindowControlsStyle::Windows
        );
        assert_eq!(
            window_chrome_for(Os::Linux, true).controls,
            WindowControlsStyle::Gnome
        );
        let json = serde_json::to_string(&window_chrome_for(Os::Windows, true)).unwrap();
        assert_eq!(json, r#"{"controls":"windows","tray":true}"#);
    }
}
