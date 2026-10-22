//! Command-line parsing. Flags override values from the config file.

use crate::config::{Settings, parse_noise, validate_duration, validate_volume};
use crate::noise::NoiseKind;
use lexopt::prelude::*;

pub const HELP: &str = "\
work - minimal focus timer with built-in noise

USAGE:
    work [MINUTES] [OPTIONS]

ARGS:
    MINUTES                  Session length in minutes (default: 30)

OPTIONS:
    -t, --time <MINUTES>     Session length in minutes (same as MINUTES)
    -n, --noise <KIND>       Noise colour: white, pink, brown (default: white)
    -v, --volume <0-100>     Noise volume, independent of system volume (default: 35)
    -s, --silent             Start without sound
        --no-tui             Plain text mode instead of the full-screen interface
        --no-notify          Do not send a desktop notification when done
        --ascii              Use ASCII symbols only (auto-detected from locale)
    -h, --help               Show this help
    -V, --version            Show version

KEYS (TUI):
    Space   pause / resume       +/-   volume
    N       next noise           M     mute / unmute
    R       restart session      Q     quit
    ?       help

CONFIG:
    $XDG_CONFIG_HOME/work/config.toml (usually ~/.config/work/config.toml)
        duration = 30
        noise = \"white\"
        volume = 35
        notifications = true
        theme = \"minimal\"          # or \"mono\" for no colours

    NO_COLOR=1 disables colours.

EXAMPLES:
    work                          30 minutes of white noise
    work 45                       45 minutes
    work 50 --noise brown -v 25   50 minutes of quiet brown noise
    work -t 25 --silent --no-tui  25 minutes, no sound, plain output
";

#[derive(Debug, Default, PartialEq)]
pub struct Overrides {
    pub duration_min: Option<u32>,
    pub noise: Option<NoiseKind>,
    pub volume: Option<u8>,
    pub silent: bool,
    pub no_tui: bool,
    pub no_notify: bool,
    pub ascii: bool,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Overrides),
    Help,
    Version,
}

fn parse_minutes(s: &str) -> Result<u32, String> {
    let n: u32 = s
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration '{s}': expected whole minutes, e.g. 25"))?;
    validate_duration(n)
}

fn parse_volume(s: &str) -> Result<u8, String> {
    let s = s.trim().trim_end_matches('%');
    let n: u32 = s
        .parse()
        .map_err(|_| format!("invalid volume '{s}': expected a number from 0 to 100"))?;
    validate_volume(n)
}

fn value_str(parser: &mut lexopt::Parser, flag: &str) -> Result<String, String> {
    parser
        .value()
        .map_err(|_| format!("{flag} requires a value"))?
        .into_string()
        .map_err(|_| format!("{flag}: value is not valid UTF-8"))
}

fn set_duration(slot: &mut Option<u32>, v: u32) -> Result<(), String> {
    if slot.is_some_and(|old| old != v) {
        return Err("duration given twice (use either MINUTES or --time)".into());
    }
    *slot = Some(v);
    Ok(())
}

pub fn parse<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator,
    I::Item: Into<std::ffi::OsString>,
{
    let mut o = Overrides::default();
    let mut parser = lexopt::Parser::from_args(args);
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('V') | Long("version") => return Ok(Command::Version),
            Short('t') | Long("time") => {
                let v = parse_minutes(&value_str(&mut parser, "--time")?)?;
                set_duration(&mut o.duration_min, v)?;
            }
            Short('n') | Long("noise") => {
                o.noise = Some(parse_noise(&value_str(&mut parser, "--noise")?)?);
            }
            Short('v') | Long("volume") => {
                o.volume = Some(parse_volume(&value_str(&mut parser, "--volume")?)?);
            }
            Short('s') | Long("silent") => o.silent = true,
            Long("no-tui") => o.no_tui = true,
            Long("no-notify") => o.no_notify = true,
            Long("ascii") => o.ascii = true,
            Value(v) => {
                let s = v
                    .into_string()
                    .map_err(|_| "argument is not valid UTF-8".to_string())?;
                let m = parse_minutes(&s)?;
                set_duration(&mut o.duration_min, m)?;
            }
            other => return Err(format!("{}. Try 'work --help'", other.unexpected())),
        }
    }
    Ok(Command::Run(o))
}

impl Overrides {
    pub fn apply(&self, s: &mut Settings) {
        if let Some(d) = self.duration_min {
            s.duration_min = d;
        }
        if let Some(n) = self.noise {
            s.noise = n;
        }
        if let Some(v) = self.volume {
            s.volume = v;
        }
        s.silent |= self.silent;
        s.tui &= !self.no_tui;
        s.notifications &= !self.no_notify;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Overrides {
        match parse(args.iter().copied()).unwrap() {
            Command::Run(o) => o,
            c => panic!("unexpected {c:?}"),
        }
    }

    fn err(args: &[&str]) -> String {
        parse(args.iter().copied()).unwrap_err()
    }

    #[test]
    fn defaults() {
        assert_eq!(run(&[]), Overrides::default());
    }

    #[test]
    fn positional_and_time_flag() {
        assert_eq!(run(&["45"]).duration_min, Some(45));
        assert_eq!(run(&["-t", "25"]).duration_min, Some(25));
        assert_eq!(run(&["--time=25"]).duration_min, Some(25));
        assert_eq!(run(&["-t25"]).duration_min, Some(25));
        assert_eq!(run(&["25", "-t", "25"]).duration_min, Some(25));
        assert!(err(&["25", "-t", "30"]).contains("twice"));
    }

    #[test]
    fn combined_flags() {
        let o = run(&[
            "50",
            "--noise",
            "brown",
            "--volume",
            "25",
            "--no-notify",
            "--no-tui",
        ]);
        assert_eq!(o.duration_min, Some(50));
        assert_eq!(o.noise, Some(NoiseKind::Brown));
        assert_eq!(o.volume, Some(25));
        assert!(o.no_notify && o.no_tui && !o.silent);
        assert_eq!(run(&["-v", "40%"]).volume, Some(40));
        assert!(run(&["--silent"]).silent);
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(["--help"]).unwrap(), Command::Help);
        assert_eq!(parse(["45", "-h"]).unwrap(), Command::Help);
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
    }

    #[test]
    fn invalid_values_have_clear_errors() {
        assert!(err(&["abc"]).contains("invalid duration"));
        assert!(err(&["0"]).contains("between 1 and"));
        assert!(err(&["-5"]).contains("unexpected") || err(&["-5"]).contains("invalid"));
        assert!(err(&["--volume", "101"]).contains("between 0 and 100"));
        assert!(err(&["--volume", "loud"]).contains("invalid volume"));
        assert!(err(&["--noise", "blue"]).contains("unknown noise"));
        assert!(err(&["--noise"]).contains("requires a value"));
        assert!(err(&["--bogus"]).contains("--help"));
    }

    #[test]
    fn flags_override_file_settings() {
        let mut s = Settings {
            volume: 10,
            notifications: true,
            ..Settings::default()
        };
        run(&["-v", "60", "--no-notify", "--silent"]).apply(&mut s);
        assert_eq!(s.volume, 60);
        assert!(!s.notifications && s.silent);
    }
}
