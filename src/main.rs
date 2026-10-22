//! work - minimal terminal focus timer with built-in noise.

mod app;
mod audio;
mod cli;
mod config;
mod noise;
mod notify;
mod timer;
mod ui;

use app::{Action, App};
use config::Settings;
use ratatui::crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const SIG_NONE: usize = 0;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("work: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let overrides = match cli::parse(std::env::args_os().skip(1))? {
        cli::Command::Help => {
            print!("{}", cli::HELP);
            return Ok(ExitCode::SUCCESS);
        }
        cli::Command::Version => {
            println!("work {}", env!("CARGO_PKG_VERSION"));
            return Ok(ExitCode::SUCCESS);
        }
        cli::Command::Run(o) => o,
    };

    let mut settings = Settings::default();
    if let Some(path) = config::config_path()
        && let Some(file) = config::load(&path)?
    {
        settings
            .apply_file(&file)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    overrides.apply(&mut settings);

    // Signals only set a flag; the event loop notices it within one wakeup (<= 250 ms)
    // and exits through the normal path, which restores the terminal and fades out audio.
    let signal = Arc::new(AtomicUsize::new(SIG_NONE));
    for sig in [libc_sig::SIGINT, libc_sig::SIGTERM, libc_sig::SIGHUP] {
        signal_hook::flag::register_usize(sig, Arc::clone(&signal), sig as usize)
            .map_err(|e| format!("cannot install signal handler: {e}"))?;
    }

    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let unicode = !overrides.ascii && ui::detect_unicode();
    if settings.tui && interactive {
        let look = ui::Look {
            unicode,
            pal: ui::Palette::new(settings.theme, ui::ColorDepth::detect()),
        };
        run_tui(&settings, &look, &signal)
    } else {
        run_plain(&settings, &signal, unicode)
    }
}

/// Signal numbers on Linux; avoids a direct libc dependency.
mod libc_sig {
    pub const SIGHUP: i32 = 1;
    pub const SIGINT: i32 = 2;
    pub const SIGTERM: i32 = 15;
}

fn exit_code_for(sig: usize) -> ExitCode {
    if sig == SIG_NONE {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(128 + sig as u8)
    }
}

// --- Full-screen TUI -----------------------------------------------------------------------

/// Restores the terminal on drop, so every return path and panic unwinding is covered.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<TerminalGuard> {
        terminal::enable_raw_mode()?;
        let guard = TerminalGuard;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
    }
}

fn map_key(code: KeyCode, mods: KeyModifiers) -> Option<Action> {
    if mods.contains(KeyModifiers::CONTROL) {
        // Raw mode swallows SIGINT, so Ctrl+C / Ctrl+D arrive as keys.
        return matches!(code, KeyCode::Char('c' | 'd')).then_some(Action::Quit);
    }
    Some(match code {
        KeyCode::Char(' ') | KeyCode::Char('p') | KeyCode::Char('P') => Action::TogglePause,
        KeyCode::Char('+' | '=') | KeyCode::Up | KeyCode::Right => Action::VolumeUp,
        KeyCode::Char('-' | '_') | KeyCode::Down | KeyCode::Left => Action::VolumeDown,
        KeyCode::Char('n' | 'N') => Action::NextNoise,
        KeyCode::Char('m' | 'M') => Action::ToggleMute,
        KeyCode::Char('r' | 'R') => Action::Restart,
        KeyCode::Char('?') | KeyCode::Char('h') | KeyCode::F(1) => Action::ToggleHelp,
        KeyCode::Char('q' | 'Q') => Action::Quit,
        KeyCode::Char('y' | 'Y') | KeyCode::Enter => Action::Confirm,
        KeyCode::Esc => Action::Cancel,
        _ => Action::Cancel,
    })
}

fn run_tui(settings: &Settings, look: &ui::Look, signal: &AtomicUsize) -> Result<ExitCode, String> {
    // Keep the default panic message readable: restore the terminal before it prints.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        default_hook(info);
    }));

    let guard = TerminalGuard::enter().map_err(|e| format!("cannot initialise terminal: {e}"))?;
    let mut term = ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))
        .map_err(|e| format!("cannot initialise terminal: {e}"))?;

    let mut app = App::new(settings, Instant::now(), true);
    let result = event_loop(&mut term, &mut app, look, signal);

    // Order matters: fade the audio out while the final frame is still on screen,
    // then hand the terminal back.
    app.shutdown();
    drop(term);
    drop(guard);

    if let Some(e) = &app.audio_error {
        eprintln!("work: audio unavailable: {e}");
    }
    result?;
    Ok(exit_code_for(signal.load(Ordering::Relaxed)))
}

fn event_loop<B: ratatui::backend::Backend>(
    term: &mut ratatui::Terminal<B>,
    app: &mut App,
    look: &ui::Look,
    signal: &AtomicUsize,
) -> Result<(), String>
where
    B::Error: std::fmt::Display,
{
    let err = |e: &dyn std::fmt::Display| format!("terminal error: {e}");
    let mut dirty = true;
    loop {
        let now = Instant::now();
        app.tick(now);
        if app.quit || signal.load(Ordering::Relaxed) != SIG_NONE {
            return Ok(());
        }
        if dirty || app.is_running() {
            term.draw(|f| ui::draw(f, app, now, look))
                .map_err(|e| err(&e))?;
            dirty = false;
        }

        // Sleep in the kernel until input arrives or the next second boundary: no busy-wait.
        let timeout = app.next_wakeup(Instant::now());
        if event::poll(timeout).map_err(|e| err(&e))? {
            // Drain everything that is already queued before redrawing.
            loop {
                match event::read().map_err(|e| err(&e))? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if let Some(a) = map_key(k.code, k.modifiers) {
                            app.handle(a, Instant::now());
                        }
                    }
                    Event::Resize(..) => term.autoresize().map_err(|e| err(&e))?,
                    _ => {}
                }
                dirty = true;
                if !event::poll(Duration::ZERO).map_err(|e| err(&e))? {
                    break;
                }
            }
        }
    }
}

// --- Plain text mode -------------------------------------------------------------------------

fn run_plain(settings: &Settings, signal: &AtomicUsize, unicode: bool) -> Result<ExitCode, String> {
    let mut app = App::new(settings, Instant::now(), true);
    let mut out = io::stdout();
    let tty = out.is_terminal();
    let sound = if settings.silent {
        "silent".to_string()
    } else {
        format!("{} noise, volume {}%", app.noise, app.volume)
    };
    let _ = writeln!(
        out,
        "work: {} min focus session ({sound}). Ctrl+C to stop.",
        settings.duration_min
    );

    let mut last_shown = None;
    let mut warned = false;
    loop {
        let now = Instant::now();
        app.tick(now);
        if !warned && let Some(e) = &app.audio_error {
            let _ = writeln!(io::stderr(), "work: audio unavailable: {e}");
            warned = true;
        }
        let sig = signal.load(Ordering::Relaxed);
        let rem = ui_remaining(&app, now);
        if tty {
            if last_shown.as_ref() != Some(&rem) {
                let (fill, empty) = ui::progress_bar(app.timer.progress(now), 30, unicode);
                let _ = write!(
                    out,
                    "\r\x1b[2K  {rem}  {fill}{empty} {:>3}%",
                    (app.timer.progress(now) * 100.0) as u32
                );
                let _ = out.flush();
                last_shown = Some(rem);
            }
        } else if last_shown.is_none() {
            last_shown = Some(rem); // non-tty: only start and end lines, no spam in logs
        }
        if app.finished || sig != SIG_NONE {
            if tty {
                let _ = writeln!(out);
            }
            let msg = if app.finished {
                "Session complete. Time for a break."
            } else {
                "Stopped."
            };
            let _ = writeln!(out, "work: {msg}");
            app.shutdown();
            return Ok(exit_code_for(sig));
        }
        std::thread::sleep(app.next_wakeup(Instant::now()));
    }
}

fn ui_remaining(app: &App, now: Instant) -> String {
    timer::format_remaining(app.timer.remaining(now))
}
