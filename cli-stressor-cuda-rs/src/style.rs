use anstyle::{AnsiColor, Effects, Style};

fn paint(message: &str, style: Style) -> String {
    format!("{style}{message}{style:#}")
}

#[cfg(feature = "cuda")]
pub fn title(message: &str) -> String {
    paint(
        message,
        AnsiColor::BrightWhite.on_default().effects(Effects::BOLD),
    )
}

pub fn info(message: &str) -> String {
    paint(message, AnsiColor::BrightBlue.on_default())
}

pub fn error(message: &str) -> String {
    paint(
        message,
        AnsiColor::BrightRed.on_default().effects(Effects::BOLD),
    )
}

pub fn stylize(message: &str, is_stderr: bool) -> String {
    if is_stderr {
        error(message)
    } else {
        info(message)
    }
}

#[cfg(feature = "cuda")]
pub fn stylize_title(title: &str) -> String {
    self::title(title)
}

#[cfg(feature = "cuda")]
pub fn stylize_config(message: &str) -> String {
    paint(
        message,
        AnsiColor::BrightCyan.on_default().effects(Effects::BOLD),
    )
}
