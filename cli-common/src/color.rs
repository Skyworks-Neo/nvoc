use std::sync::atomic::{AtomicBool, Ordering};

use colored::{Color, Colorize};

static NO_COLOR_OVERRIDE: AtomicBool = AtomicBool::new(false);

pub fn init(no_color_flag: bool) {
    NO_COLOR_OVERRIDE.store(no_color_flag, Ordering::Relaxed);
    if no_color_flag {
        colored::control::set_override(false);
    } else {
        colored::control::unset_override();
    }
}

fn colors_enabled() -> bool {
    !NO_COLOR_OVERRIDE.load(Ordering::Relaxed) && std::env::var_os("NO_COLOR").is_none()
}

fn is_numeric_like(token: &str) -> bool {
    let mut has_digit = false;
    for c in token.chars() {
        if c.is_ascii_digit() {
            has_digit = true;
            continue;
        }
        if matches!(c, '+' | '-' | '.' | '#' | ':' | '/' | ',') {
            continue;
        }
        return false;
    }
    has_digit
}

fn split_affixes(token: &str) -> (&str, &str, &str) {
    let start = token
        .char_indices()
        .find(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        .map(|(i, _)| i)
        .unwrap_or(token.len());
    let end = token
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_ascii_alphanumeric() || matches!(c, '%' | '+' | '°'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    if start >= end {
        return (token, "", "");
    }
    (&token[..start], &token[start..end], &token[end..])
}

fn style_keyword(core: &str, is_stderr: bool) -> String {
    let lower = core.to_ascii_lowercase();
    if lower.contains("failed") || lower.contains("error") || lower.contains("crash") {
        return core.red().bold().to_string();
    }
    if lower == "fail" || lower == "fatal" {
        return core.red().bold().to_string();
    }
    if lower.contains("warning") {
        return core.yellow().bold().to_string();
    }
    if lower.contains("skipped") || lower.contains("skip") {
        return core.bright_yellow().bold().to_string();
    }
    if lower.contains("succeed") || lower == "success" || lower == "passed" || lower == "ok" {
        return core.green().bold().to_string();
    }
    if lower.contains("scanner") || lower.contains("point") || lower.contains("gpu") {
        return core.bright_cyan().bold().to_string();
    }
    if lower == "gemm" {
        return core.bright_red().bold().to_string();
    }
    if lower == "memcpy" {
        return core.bright_green().bold().to_string();
    }
    if lower == "memset" {
        return core.bright_yellow().bold().to_string();
    }
    if lower == "transpose" {
        return core.bright_magenta().bold().to_string();
    }
    if lower == "elementwise" || lower == "reduction" {
        return core.bright_cyan().bold().to_string();
    }
    if lower == "atomic" {
        return core.bright_red().bold().to_string();
    }
    if is_stderr {
        core.bright_white().to_string()
    } else {
        core.normal().to_string()
    }
}

fn style_value(core: &str, is_stderr: bool) -> String {
    let lower = core.to_ascii_lowercase();
    if lower.ends_with("khz") || lower.ends_with("mhz") || lower.ends_with("ghz") {
        return core.bright_cyan().bold().to_string();
    }
    if lower.ends_with("uv") || lower.ends_with("mv") || lower.ends_with('v') {
        return core.bright_magenta().bold().to_string();
    }
    if lower.ends_with('%') || lower.contains("percent") {
        return core.bright_yellow().bold().to_string();
    }
    if lower.ends_with("ms") || lower.ends_with('s') {
        return core.bright_green().bold().to_string();
    }
    if is_numeric_like(core) {
        return core.bright_cyan().bold().to_string();
    }
    style_keyword(core, is_stderr)
}

pub fn stylize_title(title: &str) -> String {
    if !colors_enabled() {
        return title.to_string();
    }

    let lower = title.to_ascii_lowercase();
    if lower.contains("failed") || lower.contains("error") || lower.contains("crash") {
        return title.red().bold().to_string();
    }
    if lower.contains("warning") {
        return title.yellow().bold().to_string();
    }
    if lower.contains("success") || lower.contains("succeed") || lower.contains("passed") {
        return title.green().bold().to_string();
    }
    if lower.contains("[scanner]") || lower.contains("scanner") {
        return title.bright_cyan().bold().to_string();
    }
    if lower.contains("power") || lower.contains("tdp") {
        return title.bright_red().bold().to_string();
    }
    if lower.contains("thermal") || lower.contains("temp") {
        return title.bright_yellow().bold().to_string();
    }
    if lower.contains("memory") {
        return title.bright_magenta().bold().to_string();
    }
    if lower.contains("clock") || lower.contains("freq") {
        return title.bright_cyan().bold().to_string();
    }
    if lower.contains("cooler") || lower.contains("fan") {
        return title.bright_green().bold().to_string();
    }
    if lower.contains("voltage") || lower.contains("boost") || lower.contains("lock") {
        return title.bright_magenta().bold().to_string();
    }
    title.bright_white().bold().to_string()
}

pub fn stylize_warning(message: &str) -> String {
    if colors_enabled() {
        message.yellow().bold().to_string()
    } else {
        message.to_string()
    }
}

pub fn stylize(message: &str, is_stderr: bool) -> String {
    if !colors_enabled() {
        return message.to_string();
    }

    if message.chars().all(|c| c == '=') {
        return message.bright_black().to_string();
    }

    message
        .split(' ')
        .map(|token| {
            if token.is_empty() {
                return String::new();
            }
            let (prefix, core, suffix) = split_affixes(token);
            if core.is_empty() {
                return token.to_string();
            }
            format!("{}{}{}", prefix, style_value(core, is_stderr), suffix)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn stylize_config(message: &str) -> String {
    if colors_enabled() {
        message.bright_cyan().bold().to_string()
    } else {
        message.to_string()
    }
}

/// nvoc-cli grouped-help family heading colors. Keys mirror the CLI's
/// `Group::key()` values; hues follow the same family conventions as
/// `stylize_title` (power=red, thermal=yellow, fan=green, clock=cyan,
/// voltage=magenta). Non-adjacent families may share a hue (perf reuses
/// thermal's, scanner clock's) — only neighboring blocks must differ.
pub fn stylize_group_heading(group: &str, text: &str) -> String {
    if !colors_enabled() {
        return text.to_string();
    }
    let color = match group {
        "info" => Color::BrightWhite,
        "power" => Color::BrightRed,
        "thermal" => Color::BrightYellow,
        "fan" => Color::BrightGreen,
        "clock" => Color::BrightCyan,
        "voltage" => Color::BrightMagenta,
        "vfp" => Color::BrightBlue,
        "perf" => Color::BrightYellow,
        "scanner" => Color::BrightCyan,
        _ => Color::BrightWhite,
    };
    text.color(color).bold().to_string()
}

/// A command-name row in the grouped help: the leading verb carries the
/// color (get=read cyan, set/clear/restart=mutate yellow, reset=restore
/// green) so the action category is legible at a glance; the rest of the
/// name stays plain.
pub fn stylize_help_command(name: &str) -> String {
    if !colors_enabled() {
        return name.to_string();
    }
    for (prefix, color) in [
        ("reset-", Color::BrightGreen),
        ("set-", Color::BrightYellow),
        ("clear-", Color::BrightYellow),
        ("restart-", Color::BrightYellow),
        ("get-", Color::BrightCyan),
    ] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return format!("{}{}", prefix.color(color).bold(), rest);
        }
    }
    name.bold().to_string()
}

/// 专为 SCANNER/调试行设计的着色器。
/// 将任何包含数字的 token 渲染为亮黄色加粗，其它 token 使用常规 style_value 规则。
pub fn stylize_scanner(message: &str, is_stderr: bool) -> String {
    if !colors_enabled() {
        return message.to_string();
    }

    message
        .split(' ')
        .map(|token| {
            if token.is_empty() {
                return String::new();
            }
            let (prefix, core, suffix) = split_affixes(token);
            if core.is_empty() {
                return token.to_string();
            }

            // 如果 core token 包含任何数字，将其渲染为亮黄色加粗，突出测量值
            let colored = if core.chars().any(|c| c.is_ascii_digit()) {
                core.bright_yellow().bold().to_string()
            } else {
                // 非数字 token 仍使用标准的值/关键字着色
                style_value(core, is_stderr)
            };

            format!("{}{}{}", prefix, colored, suffix)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{init, stylize_group_heading, stylize_help_command, stylize_warning};
    use std::sync::{Mutex, OnceLock};

    fn color_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn group_heading_styled_per_family_when_enabled() {
        let _guard = color_test_lock();
        init(false);
        if std::env::var_os("NO_COLOR").is_some() {
            return; // colors_enabled() gates on the env; nothing to observe
        }
        // cargo test pipes stdout (not a tty) — force colored to render so
        // the assertion is deterministic.
        colored::control::set_override(true);

        let heading = stylize_group_heading("power", "Power / TGP (12)");
        assert_ne!(heading, "Power / TGP (12)");
        assert!(heading.contains("Power / TGP (12)")); // text survives intact
        // Verb prefixes get their action color (get=read, set=write,
        // reset=restore)...
        let get = stylize_help_command("get-public-power-limit");
        assert!(get.starts_with("\u{1b}["));
        assert!(get.contains("get-") && get.ends_with("power-limit"));
        assert!(stylize_help_command("set-fan-speed").contains("set-"));
        assert!(stylize_help_command("reset-fan-speed").contains("reset-"));
        // ...and unprefixed names fall back to bold.
        assert_ne!(stylize_help_command("list"), "list");

        colored::control::unset_override();
    }

    #[test]
    fn group_heading_plain_when_disabled() {
        let _guard = color_test_lock();
        init(true);

        assert_eq!(
            stylize_group_heading("power", "Power / TGP (12)"),
            "Power / TGP (12)"
        );
        assert_eq!(
            stylize_help_command("get-public-power-limit"),
            "get-public-power-limit"
        );

        init(false);
    }

    #[test]
    fn warning_style_colors_whole_line_when_enabled() {
        let _guard = color_test_lock();
        init(false);
        if std::env::var_os("NO_COLOR").is_some() {
            return; // colors_enabled() gates on the env; nothing to observe
        }
        // cargo test pipes stdout (not a tty) — force colored to render so
        // the assertion is deterministic.
        colored::control::set_override(true);

        let message = "WARNING: V/F optimization intentionally probes unstable GPU settings.";
        let styled = stylize_warning(message);
        assert_ne!(styled, message);
        assert!(styled.contains("\u{1b}["));
        assert!(styled.contains(message));

        colored::control::unset_override();
    }

    #[test]
    fn warning_style_preserves_plain_text_when_disabled() {
        let _guard = color_test_lock();
        init(true);

        let message =
            "Driver resets, display loss, application failure, or a system reboot may occur.";
        assert_eq!(stylize_warning(message), message);

        init(false);
    }
}
