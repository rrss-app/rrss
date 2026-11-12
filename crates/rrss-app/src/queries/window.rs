use rrss_core::platform::{Os, WindowControls};

use crate::views::{WindowChromeView, WindowControlsStyle};

/// 主窗口的自绘标题栏：前端按这里画窗口三键。
pub fn window_chrome() -> WindowChromeView {
    window_chrome_for(Os::current())
}

fn window_chrome_for(os: Os) -> WindowChromeView {
    let controls = match os.window_controls() {
        WindowControls::Windows => WindowControlsStyle::Windows,
        WindowControls::Gnome => WindowControlsStyle::Gnome,
    };
    WindowChromeView { controls }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_names_the_control_style_per_platform() {
        assert_eq!(
            window_chrome_for(Os::Windows).controls,
            WindowControlsStyle::Windows
        );
        assert_eq!(
            window_chrome_for(Os::Linux).controls,
            WindowControlsStyle::Gnome
        );
        let json = serde_json::to_string(&window_chrome_for(Os::Windows)).unwrap();
        assert_eq!(json, r#"{"controls":"windows"}"#);
    }
}
