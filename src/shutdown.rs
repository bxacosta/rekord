//! Graceful exit when the console window is closed, the user logs off or Windows shuts down.
//!
//! Windows gives a console program a few seconds after such an event before killing
//! it. The handler asks the main loop to quit and holds the process open until the
//! recording has been closed properly (or the grace period is nearly over).

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
#[cfg(windows)]
use std::time::{Duration, Instant};

static REQUESTED: AtomicBool = AtomicBool::new(false);
static COMPLETED: AtomicBool = AtomicBool::new(false);

/// Whether the system asked the app to exit.
pub fn requested() -> bool {
    REQUESTED.load(SeqCst)
}

/// Call once everything is safely on disk; releases a pending close request.
pub fn completed() {
    COMPLETED.store(true, SeqCst);
}

#[cfg(windows)]
pub fn install() {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    // Failing to install only loses the graceful path; recordings remain recoverable.
    let _ = unsafe { SetConsoleCtrlHandler(Some(handler), true) };
}

#[cfg(not(windows))]
pub fn install() {}

#[cfg(windows)]
unsafe extern "system" fn handler(event: u32) -> windows::core::BOOL {
    use windows::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    /// Windows kills the process 5 s after a close event; leave a margin.
    const GRACE: Duration = Duration::from_millis(4500);

    REQUESTED.store(true, SeqCst);
    if event != CTRL_C_EVENT && event != CTRL_BREAK_EVENT {
        // Close, logoff or shutdown: returning lets Windows terminate the process.
        let deadline = Instant::now() + GRACE;
        while !COMPLETED.load(SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    true.into()
}
