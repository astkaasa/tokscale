use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(crate) struct LightSpinner {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LightSpinner {
    const WIDTH: usize = 8;
    const HOLD_START: usize = 30;
    const HOLD_END: usize = 9;
    const TRAIL_LENGTH: usize = 4;
    const TRAIL_COLORS: [u8; 6] = [51, 44, 37, 30, 23, 17];
    const INACTIVE_COLOR: u8 = 240;
    const FRAME_MS: u64 = 40;

    pub(crate) fn start(message: &'static str) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let running_thread = Arc::clone(&running);
        let message = message.to_string();

        let handle = thread::spawn(move || {
            let mut frame = 0usize;
            let mut stderr = io::stderr().lock();

            let _ = write!(stderr, "\x1b[?25l");
            let _ = stderr.flush();

            while running_thread.load(Ordering::Relaxed) {
                let spinner = Self::frame(frame);
                let _ = write!(stderr, "\r\x1b[K  {} {}", spinner, message);
                let _ = stderr.flush();
                frame = frame.wrapping_add(1);
                thread::sleep(Duration::from_millis(Self::FRAME_MS));
            }

            let _ = write!(stderr, "\r\x1b[K\x1b[?25h");
            let _ = stderr.flush();
        });

        Self {
            running,
            handle: Some(handle),
        }
    }

    pub(crate) fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn frame(frame: usize) -> String {
        let (position, forward) = Self::scanner_state(frame);
        let mut out = String::new();

        for i in 0..Self::WIDTH {
            let distance = if forward {
                if position >= i {
                    position - i
                } else {
                    usize::MAX
                }
            } else if i >= position {
                i - position
            } else {
                usize::MAX
            };

            if distance < Self::TRAIL_LENGTH {
                let color = Self::TRAIL_COLORS[distance.min(Self::TRAIL_COLORS.len() - 1)];
                out.push_str(&format!("\x1b[38;5;{}m■\x1b[0m", color));
            } else {
                out.push_str(&format!("\x1b[38;5;{}m⬝\x1b[0m", Self::INACTIVE_COLOR));
            }
        }

        out
    }

    fn scanner_state(frame: usize) -> (usize, bool) {
        let forward_frames = Self::WIDTH;
        let backward_frames = Self::WIDTH - 1;
        let total_cycle = forward_frames + Self::HOLD_END + backward_frames + Self::HOLD_START;
        let normalized = frame % total_cycle;

        if normalized < forward_frames {
            (normalized, true)
        } else if normalized < forward_frames + Self::HOLD_END {
            (Self::WIDTH - 1, true)
        } else if normalized < forward_frames + Self::HOLD_END + backward_frames {
            (
                Self::WIDTH - 2 - (normalized - forward_frames - Self::HOLD_END),
                false,
            )
        } else {
            (0, false)
        }
    }
}

impl Drop for LightSpinner {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_0() {
        let frame = LightSpinner::frame(0);
        assert!(frame.contains("■"));
        assert!(frame.contains("⬝"));
    }

    #[test]
    fn frame_1() {
        let frame = LightSpinner::frame(1);
        assert!(frame.contains("■"));
        assert!(frame.contains("⬝"));
    }

    #[test]
    fn frame_2() {
        let frame = LightSpinner::frame(2);
        assert!(frame.contains("■"));
        assert!(frame.contains("⬝"));
    }

    #[test]
    fn scanner_state_forward_start() {
        let (position, forward) = LightSpinner::scanner_state(0);
        assert_eq!(position, 0);
        assert!(forward);
    }

    #[test]
    fn scanner_state_forward_mid() {
        let (position, forward) = LightSpinner::scanner_state(4);
        assert_eq!(position, 4);
        assert!(forward);
    }

    #[test]
    fn scanner_state_forward_end() {
        let (position, forward) = LightSpinner::scanner_state(7);
        assert_eq!(position, 7);
        assert!(forward);
    }

    #[test]
    fn scanner_state_hold_end() {
        let (position, forward) = LightSpinner::scanner_state(8);
        assert_eq!(position, 7);
        assert!(forward);
    }

    #[test]
    fn scanner_state_backward_start() {
        let (position, forward) = LightSpinner::scanner_state(17);
        assert_eq!(position, 6);
        assert!(!forward);
    }

    #[test]
    fn scanner_state_backward_end() {
        let (position, forward) = LightSpinner::scanner_state(23);
        assert_eq!(position, 0);
        assert!(!forward);
    }

    #[test]
    fn scanner_state_hold_start() {
        let (position, forward) = LightSpinner::scanner_state(24);
        assert_eq!(position, 0);
        assert!(!forward);
    }

    #[test]
    fn scanner_state_cycle_wrap() {
        let (position1, forward1) = LightSpinner::scanner_state(0);
        let (position2, forward2) = LightSpinner::scanner_state(54);
        assert_eq!(position1, position2);
        assert_eq!(forward1, forward2);
    }
}
