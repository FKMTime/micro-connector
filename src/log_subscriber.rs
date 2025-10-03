use std::collections::HashMap;
use std::env;
use std::fmt::Write;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::{fmt, sync::atomic::AtomicUsize, write};
use tracing::{
    field::{Field, Visit},
    Id, Level, Subscriber,
};

pub struct StringVisitor<'a> {
    string: &'a mut String,
    fields: HashMap<String, String>,
}
impl<'a> StringVisitor<'a> {
    pub(crate) fn new(string: &'a mut String) -> Self {
        StringVisitor {
            string,
            fields: HashMap::new(),
        }
    }
}

impl<'a> Visit for StringVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let val = format!("{value:?}").trim_matches('"').to_string();
        //let val = format!("{value:?}");
        let name = field.name();
        self.fields.insert(name.to_string(), val);

        if name == "message" {
            write!(self.string, "{value:?} ").expect("");
        } else if name != "file" {
            write!(self.string, "{} = {:?}; ", field.name(), value).expect("");
        }
    }
}

struct LogFilter {
    target: Option<String>,
    level: Option<Level>,
}

type SharedFilesHashMap = Arc<RwLock<HashMap<String, File>>>;
pub struct MinimalTracer {
    enabled: bool,
    filters: Vec<LogFilter>,

    logs_base: PathBuf,
    files: SharedFilesHashMap,
}

fn string_to_level(string: &str) -> Option<Level> {
    match string.to_lowercase().as_str() {
        "info" => Some(Level::INFO),
        "debug" => Some(Level::DEBUG),
        "warn" | "warning" => Some(Level::WARN),
        "trace" => Some(Level::TRACE),
        "error" => Some(Level::ERROR),
        _ => None,
    }
}

fn level_to_usize(level: &Level) -> usize {
    match *level {
        Level::INFO => 0,
        Level::WARN => 1,
        Level::ERROR => 2,
        Level::DEBUG => 3,
        Level::TRACE => 4,
    }
}

fn level_to_color(level: &Level) -> &'static str {
    match *level {
        Level::INFO => "\x1b[32m",
        Level::WARN => "\x1b[33m",
        Level::ERROR => "\x1b[31m",
        Level::DEBUG => "\x1b[34m",
        Level::TRACE => "\x1b[35m",
    }
}

impl MinimalTracer {
    pub fn register(base_dir: PathBuf) -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
        _ = std::fs::create_dir_all(&base_dir);

        let mut enabled = true;
        let mut filters: Vec<LogFilter> = Vec::with_capacity(10);
        if let Ok(env_value) = env::var("RUST_LOG") {
            for filter in env_value.split(',') {
                let mut target = Some(filter);
                let mut level = None;
                if let Some(equals_index) = target.expect("Target none?").find('=') {
                    let (first, second) = filter.split_at(equals_index);
                    target = Some(first);
                    level = string_to_level(&second[1..])
                }
                let target_level = string_to_level(target.expect("Target none?"));

                if let Some(target_level) = target_level {
                    level = Some(target_level);
                    target = None;
                }

                filters.push(LogFilter {
                    target: target.map(|v| v.to_string()),
                    level,
                });
            }
        } else {
            enabled = true;
            filters = vec![LogFilter {
                target: None,
                level: Some(Level::ERROR),
            }];
        }

        tracing::subscriber::set_global_default(MinimalTracer {
            enabled,
            filters,
            logs_base: base_dir,
            files: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

static AUTO_ID: AtomicUsize = AtomicUsize::new(1);
impl Subscriber for MinimalTracer {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        if self.enabled {
            if self.filters.is_empty() {
                return true;
            }

            let mut matches: bool;
            for filter in &self.filters {
                matches = true;
                if let Some(level) = filter.level {
                    let metadata_level = level_to_usize(metadata.level());
                    let log_level = level_to_usize(&level);

                    if metadata_level > log_level {
                        matches = false;
                    }
                }
                if let Some(target) = &filter.target
                    && !metadata.target().starts_with(target) {
                        matches = false;
                    }
                if matches {
                    return true;
                }
            }

            return false;
        }

        false
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        Id::from_u64(AUTO_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u64)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let metadata = event.metadata();
        let level = metadata.level();
        let target = metadata.target();

        let mut text = String::new();
        let mut visitor = StringVisitor::new(&mut text);
        event.record(&mut visitor);

        let time = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
        let color = level_to_color(level);

        let file_field = visitor.fields.get("file");
        if let Some(file_field) = file_field {
            let tmp = format!("{time} {level: >5} {target}: {text}\n");

            let files = self.files.read().expect("cannot lock");
            if let Some(mut file) = files.get(file_field) {
                file.write_all(tmp.as_bytes())
                    .expect("cannot write to file");
            } else {
                drop(files);

                let file_path = self.logs_base.join(format!("{file_field}.log"));
                let mut files = self.files.write().expect("cannot lock");
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(file_path)
                    .expect("cannot open file");

                file.write_all(tmp.as_bytes())
                    .expect("cannot write to file");
                files.insert(file_field.to_string(), file);
            }
        } else {
            println!("{time} {color}{level: >5}\x1b[0m {target}: {text}");
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}
