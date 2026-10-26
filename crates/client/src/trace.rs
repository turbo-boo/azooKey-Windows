use std::fmt::Write as _;
use std::{fs::File, path::PathBuf};
use tracing::field::{Field, Visit};
use tracing_core::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::{layer::SubscriberExt as _, util::SubscriberInitExt};
use windows::{core::PCWSTR, Win32::System::Diagnostics::Debug::OutputDebugStringW};

use crate::extension::StringExt as _;
use crate::tracing_chrome::{ChromeLayerBuilder, EventOrSpan};

/// `%LOCALAPPDATA%\Azookey\logs`: per user and writable without elevation.
fn log_folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Azookey")
        .join("logs")
}

pub struct StringVisitor<'a> {
    string: &'a mut String,
}

impl<'a> Visit for StringVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // do nothing
        if field.name() == "message" {
            write!(self.string, "{:?}", value).unwrap();
        }
    }
}

/// Writes a Chrome trace of this process. DllMain starts it in debug
/// builds only.
pub fn setup_logger() -> anyhow::Result<()> {
    // Every process loading the text service writes its own trace; the
    // process id keeps two started in the same millisecond apart.
    let folder = log_folder();
    let timestamp = chrono::Local::now().format("%Y-%m-%d-%H.%M.%S%.3f");
    let path = folder.join(format!("{timestamp}-{}.json", std::process::id()));

    // Logging is optional: without a writable folder there is no trace
    // (e.g. in sandboxed processes).
    let writer = match std::fs::create_dir_all(&folder)
        .and_then(|_| File::options().write(true).create_new(true).open(&path))
    {
        Ok(file) => file,
        Err(_) => return Ok(()),
    };

    let builder = ChromeLayerBuilder::new()
        .include_locations(true)
        .include_args(true)
        .name_fn(Box::new(|event_or_span| match event_or_span {
            EventOrSpan::Event(event) => {
                let message = {
                    let mut message = String::new();
                    event.record(&mut StringVisitor {
                        string: &mut message,
                    });
                    message
                };

                let (level, file, line) = {
                    let metadeta = event.metadata();
                    let level = metadeta.level().as_str();
                    let file = metadeta.file().unwrap_or_default();
                    let line = metadeta.line().unwrap_or_default();

                    (level, file, line)
                };

                let str = format!("[{}: {}:{}] {}", level, file, line, message);
                let wide: Vec<u16> = str.as_str().to_wide_16();
                unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };

                message
            }
            EventOrSpan::Span(span) => span.metadata().name().to_string(),
        }));

    let chrome_layer = builder.build(writer);

    // ignore traces from other crates
    let filter = Targets::new()
        .with_target("azookey_windows", LevelFilter::DEBUG)
        .with_default(LevelFilter::OFF);

    tracing_subscriber::registry()
        .with(filter)
        .with(chrome_layer)
        .init();

    Ok(())
}
