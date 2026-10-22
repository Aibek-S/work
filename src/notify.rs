//! Desktop notifications via the system `notify-send` (libnotify). Optional:
//! if it is missing or there is no notification daemon, the call silently does nothing.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Notifier {
    child: Option<Child>,
}

impl Notifier {
    pub fn send(&mut self, title: &str, body: &str) {
        self.finish();
        self.child = Command::new("notify-send")
            .args([
                "--app-name=work",
                "--icon=alarm-symbolic",
                "--urgency=normal",
                title,
                body,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }

    /// Non-blocking reap so a finished `notify-send` never lingers as a zombie.
    pub fn reap(&mut self) {
        if let Some(c) = &mut self.child
            && !matches!(c.try_wait(), Ok(None))
        {
            self.child = None;
        }
    }

    /// Waits briefly for a pending notification, then gives up and kills it.
    pub fn finish(&mut self) {
        let Some(mut c) = self.child.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(c.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = c.kill();
        let _ = c.wait();
    }
}
