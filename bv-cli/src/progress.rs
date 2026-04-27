use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use bv_runtime::{PauseGuard, ProgressReporter};

pub struct CliProgressReporter {
    bar: ProgressBar,
}

impl CliProgressReporter {
    fn styled(bar: ProgressBar) -> Self {
        bar.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar }
    }

    /// Spinner added to a `MultiProgress` (multi-tool path).
    pub fn for_multi(mp: &MultiProgress) -> Self {
        Self::styled(mp.add(ProgressBar::new_spinner()))
    }
}

impl ProgressReporter for CliProgressReporter {
    fn update(&self, message: &str, _current: Option<u64>, _total: Option<u64>) {
        if !message.is_empty() {
            self.bar.set_message(message.to_string());
        }
    }

    fn finish(&self, message: &str) {
        self.bar.finish_and_clear();
        if !message.is_empty() {
            eprintln!("  {message}");
        }
    }

    fn pause(&self) -> Box<dyn PauseGuard + '_> {
        self.bar.disable_steady_tick();
        let saved = self.bar.message();
        self.bar.set_message(String::new());
        self.bar.tick();
        Box::new(SpinnerPauseGuard {
            bar: &self.bar,
            saved,
        })
    }
}

struct SpinnerPauseGuard<'a> {
    bar: &'a ProgressBar,
    saved: String,
}

impl PauseGuard for SpinnerPauseGuard<'_> {}

impl Drop for SpinnerPauseGuard<'_> {
    fn drop(&mut self) {
        self.bar.set_message(std::mem::take(&mut self.saved));
        self.bar.enable_steady_tick(Duration::from_millis(80));
    }
}
