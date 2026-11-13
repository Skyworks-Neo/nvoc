#[cfg(feature = "cuda")]
pub fn title(message: &str) -> String {
    nvoc_cli_common::color::stylize_title(message)
}

pub fn stylize(message: &str, is_stderr: bool) -> String {
    nvoc_cli_common::color::stylize(message, is_stderr)
}

#[cfg(feature = "cuda")]
pub fn stylize_title(title: &str) -> String {
    self::title(title)
}

#[cfg(feature = "cuda")]
pub fn stylize_config(message: &str) -> String {
    nvoc_cli_common::color::stylize_config(message)
}
