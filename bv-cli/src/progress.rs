use std::io::Write;
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use bv_runtime::{PauseGuard, ProgressReporter};

pub struct CliProgressReporter {
    bar: ProgressBar,
    mp: MultiProgress,
}

impl CliProgressReporter {
    fn styled(bar: ProgressBar, mp: MultiProgress) -> Self {
        bar.set_style(
            ProgressStyle::with_template("  {spinner:.cyan} {msg}  [{elapsed}]")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar, mp }
    }

    /// Spinner added to a `MultiProgress` (multi-tool path).
    pub fn for_multi(mp: &MultiProgress) -> Self {
        Self::styled(mp.add(ProgressBar::new_spinner()), mp.clone())
    }

    /// Print a status line above the spinner without tearing it. Use this
    /// for top-level "Pulling X" / "Added X" lines so they interleave cleanly
    /// with the running spinner.
    pub fn println(&self, line: &str) {
        let _ = self.mp.println(line);
    }
}

impl ProgressReporter for CliProgressReporter {
    fn update(&self, message: &str, current: Option<u64>, total: Option<u64>) {
        if message.is_empty() {
            return;
        }
        let msg = match (current, total) {
            (Some(done), Some(n)) if n > 0 => format!("{message}  {done} / {n} layers"),
            _ => message.to_string(),
        };
        self.bar.set_message(msg);
    }

    fn finish(&self, message: &str) {
        self.bar.finish_and_clear();
        if !message.is_empty() {
            eprintln!("  {message}");
        }
    }

    fn pause(&self) -> Box<dyn PauseGuard + '_> {
        // One-shot pause: caller is expected to call `finish()` afterwards.
        // We clear the spinner row and never restore it, which avoids the brief
        // flicker (and stale glyph artifact) you'd otherwise see between
        // "rolling tail done" and "finish_and_clear".
        self.bar.disable_steady_tick();
        self.bar.set_message(String::new());
        eprint!("\r\x1b[2K");
        let _ = std::io::stderr().flush();
        Box::new(SpinnerPauseGuard)
    }
}

struct SpinnerPauseGuard;
impl PauseGuard for SpinnerPauseGuard {}
