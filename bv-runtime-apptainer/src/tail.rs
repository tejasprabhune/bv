use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

const MAX_LINES: usize = 10;
const REDRAW_INTERVAL: Duration = Duration::from_millis(80);

/// Update emitted by a reader thread.
pub enum Chunk {
    /// Replace the trailing line (carriage-return semantics).
    Replace(String),
    /// Append a new line (newline semantics).
    Append(String),
    /// Reader closed.
    Eof,
}

/// Drain `reader` on a background thread, splitting on `\n` and `\r` and
/// emitting `Chunk`s through `tx`. The thread exits when the pipe closes.
pub fn spawn_reader<R: Read + Send + 'static>(mut reader: R, tx: Sender<Chunk>) {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut current = String::new();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            for &b in &buf[..n] {
                match b {
                    b'\n' => {
                        let _ = tx.send(Chunk::Append(std::mem::take(&mut current)));
                    }
                    b'\r' => {
                        let _ = tx.send(Chunk::Replace(std::mem::take(&mut current)));
                    }
                    _ => current.push(b as char),
                }
            }
        }
        if !current.is_empty() {
            let _ = tx.send(Chunk::Append(std::mem::take(&mut current)));
        }
        let _ = tx.send(Chunk::Eof);
    });
}

/// Renders a rolling, dimmed window of up to `MAX_LINES` lines from one or
/// more reader threads. Drops most updates between redraws so a stream of
/// progress-bar carriage returns can't hog the terminal.
pub struct RollingTail {
    lines: VecDeque<String>,
    rendered_rows: usize,
    last_draw: Instant,
}

impl RollingTail {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(MAX_LINES),
            rendered_rows: 0,
            last_draw: Instant::now() - REDRAW_INTERVAL,
        }
    }

    fn append(&mut self, line: String) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    fn replace_last(&mut self, line: String) {
        if let Some(last) = self.lines.back_mut() {
            *last = line;
        } else {
            self.lines.push_back(line);
        }
    }

    /// Run until every reader has signalled `Eof`. Output is written to stderr.
    pub fn run(mut self, rx: Receiver<Chunk>, readers: usize) {
        let mut alive = readers;
        let mut dirty = false;
        while alive > 0 {
            match rx.recv_timeout(REDRAW_INTERVAL) {
                Ok(Chunk::Append(line)) => {
                    if !line.is_empty() {
                        self.append(line);
                        dirty = true;
                    }
                }
                Ok(Chunk::Replace(line)) => {
                    if !line.is_empty() {
                        self.replace_last(line);
                        dirty = true;
                    }
                }
                Ok(Chunk::Eof) => alive -= 1,
                Err(_) => {} // timeout, fall through to redraw
            }
            if dirty && self.last_draw.elapsed() >= REDRAW_INTERVAL {
                self.redraw();
                dirty = false;
            }
        }
        if dirty {
            self.redraw();
        }
        self.clear();
    }

    fn redraw(&mut self) {
        let mut out = String::new();
        // Move cursor up over the previous render and clear each row.
        for _ in 0..self.rendered_rows {
            out.push_str("\x1b[1A\x1b[2K");
        }
        for line in &self.lines {
            // Truncate runaway-long progress bars so they fit on one row.
            let truncated: String = line.chars().take(200).collect();
            // Dim grey: ANSI faint.
            out.push_str("\x1b[2m");
            out.push_str(&truncated);
            out.push_str("\x1b[0m\n");
        }
        self.rendered_rows = self.lines.len();
        let _ = std::io::stderr().write_all(out.as_bytes());
        let _ = std::io::stderr().flush();
        self.last_draw = Instant::now();
    }

    fn clear(&mut self) {
        let mut out = String::new();
        for _ in 0..self.rendered_rows {
            out.push_str("\x1b[1A\x1b[2K");
        }
        self.rendered_rows = 0;
        let _ = std::io::stderr().write_all(out.as_bytes());
        let _ = std::io::stderr().flush();
    }
}

pub fn make_channel() -> (Sender<Chunk>, Receiver<Chunk>) {
    channel()
}
