//! Session state and control logic shared by the TUI and the plain-text mode.
//! Nothing here touches the terminal, which keeps the behaviour unit-testable.

use crate::audio::{Audio, volume_to_gain};
use crate::config::Settings;
use crate::noise::NoiseKind;
use crate::notify::Notifier;
use crate::timer::Timer;
use std::time::{Duration, Instant};

pub const VOLUME_STEP: u8 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    ConfirmRestart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    TogglePause,
    VolumeUp,
    VolumeDown,
    NextNoise,
    ToggleMute,
    Restart,
    ToggleHelp,
    Confirm,
    Cancel,
    Quit,
}

pub struct App {
    pub timer: Timer,
    pub noise: NoiseKind,
    pub volume: u8,
    pub muted: bool,
    pub finished: bool,
    pub overlay: Overlay,
    pub quit: bool,
    pub audio_error: Option<String>,
    audio: Option<Audio>,
    /// False in tests and after a fatal audio error: never try to open a device.
    audio_enabled: bool,
    notifier: Notifier,
    notifications: bool,
}

impl App {
    pub fn new(s: &Settings, now: Instant, audio_enabled: bool) -> App {
        let mut app = App {
            timer: Timer::new(Duration::from_secs(u64::from(s.duration_min) * 60), now),
            noise: s.noise,
            volume: s.volume,
            muted: s.silent,
            finished: false,
            overlay: Overlay::None,
            quit: false,
            audio_error: None,
            audio: None,
            audio_enabled,
            notifier: Notifier::default(),
            notifications: s.notifications,
        };
        app.sync_audio();
        app
    }

    pub fn is_running(&self) -> bool {
        !self.finished && !self.timer.is_paused()
    }

    /// Whether noise should currently be audible.
    pub fn sound_on(&self) -> bool {
        self.is_running() && !self.muted && self.volume > 0
    }

    #[cfg(test)]
    pub fn has_audio_device(&self) -> bool {
        self.audio.is_some()
    }

    /// Pushes the desired gain/noise to the audio engine, opening the device lazily
    /// the first time sound is actually needed (so `--silent` never opens it).
    fn sync_audio(&mut self) {
        let gain = if self.sound_on() {
            volume_to_gain(self.volume)
        } else {
            0.0
        };
        if gain > 0.0 && self.audio.is_none() && self.audio_enabled {
            match Audio::open(self.noise) {
                Ok(a) => self.audio = Some(a),
                Err(e) => {
                    self.audio_error = Some(e);
                    self.audio_enabled = false;
                }
            }
        }
        if let Some(a) = &mut self.audio {
            a.set_kind(self.noise);
            a.set_gain(gain);
        }
    }

    /// Periodic housekeeping: detects the end of the session and services audio.
    pub fn tick(&mut self, now: Instant) {
        if !self.finished && self.timer.is_finished(now) {
            self.finished = true;
            self.overlay = Overlay::None;
            self.sync_audio(); // fades the noise out
            if self.notifications {
                let mins = self.timer.total().as_secs() / 60;
                self.notifier.send(
                    "Focus session complete",
                    &format!("{mins} min of focus done. Time for a break."),
                );
            }
        }
        if let Some(a) = &mut self.audio {
            if let Some(e) = a.error() {
                self.audio_error = Some(e);
                self.audio = None;
                self.audio_enabled = false;
            } else {
                a.maintain(now);
            }
        }
        self.notifier.reap();
    }

    /// How long the event loop may sleep before something on screen or in audio changes.
    pub fn next_wakeup(&self, now: Instant) -> Duration {
        let mut d = Duration::from_millis(250); // upper bound keeps signal checks prompt
        if self.is_running() {
            d = d.min(self.timer.until_next_second(now) + Duration::from_millis(2));
        }
        if self.audio.as_ref().is_some_and(Audio::needs_maintenance) {
            d = d.min(Duration::from_millis(50));
        }
        d
    }

    pub fn handle(&mut self, action: Action, now: Instant) {
        // Modal overlays capture input first.
        match self.overlay {
            Overlay::ConfirmRestart => {
                match action {
                    Action::Confirm | Action::Restart => self.restart(now),
                    Action::Quit => self.quit = true,
                    _ => {}
                }
                self.overlay = Overlay::None;
                return;
            }
            Overlay::Help => {
                if action == Action::Quit {
                    self.quit = true;
                }
                self.overlay = Overlay::None;
                return;
            }
            Overlay::None => {}
        }
        match action {
            Action::TogglePause if !self.finished => self.timer.toggle(now),
            Action::VolumeUp => {
                self.volume = self.volume.saturating_add(VOLUME_STEP).min(100);
                self.muted = false;
            }
            Action::VolumeDown => {
                self.volume = self.volume.saturating_sub(VOLUME_STEP);
                self.muted = false;
            }
            Action::NextNoise => self.noise = self.noise.next(),
            Action::ToggleMute => self.muted = !self.muted,
            Action::Restart if self.finished => self.restart(now),
            Action::Restart => self.overlay = Overlay::ConfirmRestart,
            Action::ToggleHelp => self.overlay = Overlay::Help,
            Action::Quit => self.quit = true,
            _ => {}
        }
        self.sync_audio();
    }

    fn restart(&mut self, now: Instant) {
        self.timer.restart(now);
        self.finished = false;
        self.sync_audio();
    }

    /// Fades out and releases the audio device. Called on every exit path.
    pub fn shutdown(&mut self) {
        if let Some(mut a) = self.audio.take() {
            a.shutdown();
        }
        self.notifier.finish();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(now: Instant) -> App {
        let s = Settings {
            duration_min: 1,
            notifications: false,
            ..Settings::default()
        };
        App::new(&s, now, false)
    }

    #[test]
    fn pause_resume_and_finish() {
        let t0 = Instant::now();
        let mut a = app(t0);
        assert!(a.sound_on());
        a.handle(Action::TogglePause, t0 + Duration::from_secs(10));
        assert!(!a.is_running() && !a.sound_on());
        a.tick(t0 + Duration::from_secs(500));
        assert!(!a.finished, "paused timer must not finish");
        a.handle(Action::TogglePause, t0 + Duration::from_secs(500));
        a.tick(t0 + Duration::from_secs(549));
        assert!(!a.finished);
        a.tick(t0 + Duration::from_secs(550));
        assert!(a.finished && !a.sound_on());
        // Pause is ignored once finished.
        a.handle(Action::TogglePause, t0 + Duration::from_secs(551));
        assert!(a.finished);
    }

    #[test]
    fn volume_is_clamped_and_unmutes() {
        let t0 = Instant::now();
        let mut a = app(t0);
        a.handle(Action::ToggleMute, t0);
        assert!(a.muted && !a.sound_on());
        for _ in 0..30 {
            a.handle(Action::VolumeUp, t0);
        }
        assert_eq!(a.volume, 100);
        assert!(!a.muted);
        for _ in 0..30 {
            a.handle(Action::VolumeDown, t0);
        }
        assert_eq!(a.volume, 0);
        assert!(!a.sound_on(), "volume 0 is silent");
    }

    #[test]
    fn restart_requires_confirmation_while_running() {
        let t0 = Instant::now();
        let mut a = app(t0);
        let t1 = t0 + Duration::from_secs(30);
        a.handle(Action::Restart, t1);
        assert_eq!(a.overlay, Overlay::ConfirmRestart);
        a.handle(Action::Cancel, t1);
        assert_eq!(a.overlay, Overlay::None);
        assert_eq!(a.timer.remaining(t1), Duration::from_secs(30));
        a.handle(Action::Restart, t1);
        a.handle(Action::Confirm, t1);
        assert_eq!(a.timer.remaining(t1), Duration::from_secs(60));
    }

    #[test]
    fn restart_after_finish_is_immediate() {
        let t0 = Instant::now();
        let mut a = app(t0);
        let t1 = t0 + Duration::from_secs(61);
        a.tick(t1);
        assert!(a.finished);
        a.handle(Action::Restart, t1);
        assert!(!a.finished && a.is_running());
        assert_eq!(a.overlay, Overlay::None);
    }

    #[test]
    fn help_overlay_swallows_keys_but_quit_works() {
        let t0 = Instant::now();
        let mut a = app(t0);
        a.handle(Action::ToggleHelp, t0);
        a.handle(Action::NextNoise, t0);
        assert_eq!(a.noise, NoiseKind::White, "key only closes help");
        assert_eq!(a.overlay, Overlay::None);
        a.handle(Action::NextNoise, t0);
        assert_eq!(a.noise, NoiseKind::Pink);
        a.handle(Action::ToggleHelp, t0);
        a.handle(Action::Quit, t0);
        assert!(a.quit);
    }

    #[test]
    fn silent_start_is_muted() {
        let s = Settings {
            silent: true,
            notifications: false,
            ..Settings::default()
        };
        let a = App::new(&s, Instant::now(), false);
        assert!(a.muted && !a.sound_on() && !a.has_audio_device());
    }

    #[test]
    fn wakeup_is_bounded_and_aligned_to_seconds() {
        let t0 = Instant::now();
        let mut a = app(t0);
        let w = a.next_wakeup(t0 + Duration::from_millis(400));
        assert_eq!(w, Duration::from_millis(250));
        let w = a.next_wakeup(t0 + Duration::from_millis(900));
        assert_eq!(w, Duration::from_millis(102));
        a.handle(Action::TogglePause, t0);
        assert_eq!(a.next_wakeup(t0), Duration::from_millis(250));
    }
}
