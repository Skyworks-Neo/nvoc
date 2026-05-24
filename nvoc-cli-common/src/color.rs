use colored::Colorize;

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
    if lower.contains("warning") {
        return core.yellow().bold().to_string();
    }
    if lower.contains("skipped") || lower.contains("skip") {
        return core.bright_yellow().bold().to_string();
    }
    if lower.contains("succeed") || lower == "success" || lower == "passed" {
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
    if lower == "elementwise" {
        return core.bright_cyan().bold().to_string();
    }
    if lower == "reduction" {
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
    if std::env::var_os("NO_COLOR").is_some() {
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

pub fn stylize(message: &str, is_stderr: bool) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
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
    if std::env::var_os("NO_COLOR").is_some() {
        message.to_string()
    } else {
        message.bright_cyan().bold().to_string()
    }
}

/// 专为 SCANNER/调试行设计的着色器。
/// 将任何包含数字的 token 渲染为亮黄色加粗，其它 token 使用常规 style_value 规则。
pub fn stylize_scanner(message: &str, is_stderr: bool) -> String {
    if std::env::var_os("NO_COLOR").is_some() {
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
