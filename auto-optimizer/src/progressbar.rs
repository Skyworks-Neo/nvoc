use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use once_cell::sync::Lazy;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod scan_cli_color {
    pub(super) fn init(no_color_flag: bool) {
        nvoc_cli_common::color::init(no_color_flag);
    }

    pub(super) fn stylize(message: &str, is_stderr: bool) -> String {
        nvoc_cli_common::color::stylize_scanner(message, is_stderr)
    }
}

pub(crate) fn init_scan_cli_color(no_color_flag: bool) {
    scan_cli_color::init(no_color_flag);
}

static ACTIVE_SCAN_PROGRESS: Lazy<Mutex<Option<Arc<ScanProgress>>>> =
    Lazy::new(|| Mutex::new(None));

fn set_active_scan_progress(progress: Option<Arc<ScanProgress>>) {
    if let Ok(mut slot) = ACTIVE_SCAN_PROGRESS.lock() {
        *slot = progress;
    }
}

fn active_scan_progress() -> Option<Arc<ScanProgress>> {
    ACTIVE_SCAN_PROGRESS
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().cloned())
}

pub(crate) struct ActiveScanProgressGuard;

impl ActiveScanProgressGuard {
    pub(crate) fn enter(progress: Arc<ScanProgress>) -> Self {
        set_active_scan_progress(Some(progress));
        Self
    }
}

impl Drop for ActiveScanProgressGuard {
    fn drop(&mut self) {
        set_active_scan_progress(None);
    }
}

pub(crate) struct ScanProgress {
    _mp: Arc<MultiProgress>,
    total_bar: ProgressBar,
    status_bar: ProgressBar,
    test_bar: ProgressBar,
}

impl ScanProgress {
    pub(crate) fn new(lower_point: usize, upper_point: usize) -> Self {
        const PROGRESS_APPEAR_DELAY_SECS: u64 = 3;

        // Start hidden to avoid early redraw noise; reveal the multi-progress shortly after start.
        let mp = Arc::new(MultiProgress::with_draw_target(ProgressDrawTarget::hidden()));
        {
            let mp_reveal = Arc::clone(&mp);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(PROGRESS_APPEAR_DELAY_SECS));
                mp_reveal.set_draw_target(ProgressDrawTarget::stderr());
            });
        }
        let total_len = upper_point.saturating_sub(lower_point).max(1) as u64;

        let total_bar = mp.add(ProgressBar::new(total_len));
        total_bar.set_style(
            ProgressStyle::with_template(
                "{msg:.bold.white} [{bar:40.magenta/red}] {pos}/{len} points ({percent}%)",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        total_bar.set_message("Scan progress".to_string());
        total_bar.set_position(0);

        let status_bar = mp.add(ProgressBar::new(0));
        status_bar.set_style(
            ProgressStyle::with_template("{msg:.bold}")
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        status_bar.set_message("Status: idle".to_string());
        status_bar.set_position(0);

        let test_bar = mp.add(ProgressBar::new(1));
        test_bar.set_style(
            ProgressStyle::with_template(
                "{msg:.bold.green} [{bar:40.green/red}] {pos}/{len}s ({percent}%)",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        test_bar.set_message("Run idle".to_string());
        test_bar.set_position(0);

        Self {
            _mp: mp,
            total_bar,
            status_bar,
            test_bar,
        }
    }

    pub(crate) fn set_total_point(
        &self,
        current_point: usize,
        lower_point: usize,
        upper_point: usize,
    ) {
        let len = upper_point.saturating_sub(lower_point).max(1) as u64;
        let pos = current_point
            .saturating_sub(lower_point)
            .min(upper_point.saturating_sub(lower_point)) as u64;

        self.total_bar.set_length(len);
        self.total_bar.set_position(pos);
        self.total_bar
            .set_message(format!("Scan point {} / {}", current_point, upper_point));
    }

    pub(crate) fn total_bar(&self) -> ProgressBar {
        self.total_bar.clone()
    }

    fn println(&self, message: impl AsRef<str>) {
        self.total_bar.println(message.as_ref());
    }

    pub(crate) fn set_status(&self, message: impl Into<String>) {
        self.status_bar
            .set_message(stylize_status_message(&message.into()));
    }

    pub(crate) fn begin_test(
        &self,
        label: impl Into<String>,
        duration_secs: u64,
    ) -> TestProgressGuard {
        let duration_secs = duration_secs.max(1).saturating_mul(5);
        let label = label.into();
        self.test_bar.set_length(duration_secs);
        self.test_bar.set_position(0);
        self.test_bar.set_message(label.clone());

        let bar = self.test_bar.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let handle = thread::spawn(move || {
            let started_at = Instant::now();
            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                let elapsed = started_at.elapsed().as_secs().min(duration_secs);
                bar.set_position(elapsed);
                if elapsed >= duration_secs {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
            bar.set_position(duration_secs);
            bar.set_message(format!("{} done", label));
        });

        TestProgressGuard {
            stop,
            join: Some(handle),
            bar: self.test_bar.clone(),
            duration_secs,
        }
    }
}

fn stylize_status_message(message: &str) -> String {
    // Keep non-state lines unchanged to avoid over-coloring regular status messages.
    if !message.contains("State:") {
        return message.to_string();
    }

    let mut out = message.to_string();
    for key in ["State", "Pt", "V", "default", "current", "delta", "thrm"] {
        let plain = format!("{}:", key);
        let styled = format!("{}:", key.bright_cyan().bold());
        out = out.replace(&plain, &styled);
    }

    out.replace("HIGH", &"HIGH".bright_red().bold().to_string())
        .replace("LOW", &"LOW".bright_green().bold().to_string())
}

pub(crate) struct TestProgressGuard {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    bar: ProgressBar,
    duration_secs: u64,
}

impl Drop for TestProgressGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
        self.bar.set_position(self.duration_secs.max(1));
    }
}

pub(crate) fn progress_print(progress: Option<&ScanProgress>, message: impl AsRef<str>) {
    if let Some(progress) = progress {
        progress.println(message);
        return;
    }

    if let Some(active) = active_scan_progress() {
        active.println(message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

pub(crate) fn forward_child_output<R: std::io::Read + Send + 'static>(
    reader: R,
    logger: Option<ProgressBar>,
    is_stderr: bool,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        let mut last_print = Instant::now() - Duration::from_secs(10);
        let throttle = Duration::from_millis(500);

        fn strip_ansi(input: &str) -> String {
            let mut out = String::with_capacity(input.len());
            let mut chars = input.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    if let Some('[') = chars.peek().cloned() {
                        let _ = chars.next();
                        while let Some(&nc) = chars.peek() {
                            let code = nc as u32;
                            if ('@' as u32) <= code && code <= ('~' as u32) {
                                let _ = chars.next();
                                break;
                            } else {
                                let _ = chars.next();
                            }
                        }
                        continue;
                    } else {
                        continue;
                    }
                }

                if (c as u32) < 0x20 && c != '\n' && c != '\r' && c != '\t' {
                    continue;
                }
                out.push(c);
            }

            let mut res = String::with_capacity(out.len());
            let chars: Vec<char> = out.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if chars[i].is_ascii_digit() {
                    let mut j = i;
                    let mut has_semicolon = false;
                    while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ';') {
                        if chars[j] == ';' {
                            has_semicolon = true;
                        }
                        j += 1;
                    }
                    if j < chars.len() && chars[j] == 'm' && has_semicolon && (j - i) <= 10 {
                        i = j + 1;
                        continue;
                    }
                }
                res.push(chars[i]);
                i += 1;
            }
            res
        }

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let raw = line.trim_end_matches(['\r', '\n']);
                    let clean = strip_ansi(raw);

                    let lower = clean.to_ascii_lowercase();
                    let important = lower.contains("warning")
                        || lower.contains("error")
                        || lower.contains("failed")
                        || lower.contains("crash")
                        || lower.contains("tdr")
                        || lower.contains("voltage")
                        || lower.contains("throttl");

                    let now = Instant::now();
                    if let Some(logger) = &logger {
                        if important || now.duration_since(last_print) >= throttle {
                            let styled = scan_cli_color::stylize(&clean, is_stderr);
                            logger.println(styled);
                            last_print = now;
                        }
                    } else {
                        let styled = scan_cli_color::stylize(&clean, is_stderr);
                        if is_stderr {
                            eprintln!("{}", styled);
                        } else {
                            println!("{}", styled);
                        }
                    }
                }
                Err(e) => {
                    let warn = format!("Warning: failed to read child output: {}", e);
                    let styled = scan_cli_color::stylize(&warn, true);
                    if let Some(logger) = &logger {
                        logger.println(styled);
                    } else {
                        eprintln!("{}", styled);
                    }
                    break;
                }
            }
        }
    })
}
