use std::env;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use env_logger::filter::{Builder, Filter};

use log::{Log, Metadata, Record};

/// Small `env_logger`-like logger that reads filters from an environment variable and logs to a
/// provided file.
pub struct Logger {
    file: Mutex<File>,
    filter: Filter,
}

impl Logger {
    pub fn init(env_var: impl AsRef<OsStr>, path: impl AsRef<Path>) {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .expect("could not open log file");

        let mut filter_builder = Builder::new();

        if let Ok(filter) = env::var(env_var) {
            filter_builder.parse(&filter);
        }

        let filter = filter_builder.build();
        let max_level = filter.filter();

        log::set_boxed_logger(Box::new(Logger {
            file: Mutex::new(file),
            filter,
        }))
        .map(|()| log::set_max_level(max_level))
        .expect("could not initialize logger");
    }
}

#[allow(clippy::unwrap_used)]
impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.filter.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if self.filter.matches(record) {
            let mut file = self.file.lock().unwrap();

            let _ = writeln!(
                file,
                "{} {:5} {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {
        let _ = self.file.lock().unwrap().flush();
    }
}
