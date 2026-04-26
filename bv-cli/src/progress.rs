use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use bv_runtime::ProgressReporter;

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
        bar.enable_steady_tick(std::time::Duration::from_millis(80));
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
}
