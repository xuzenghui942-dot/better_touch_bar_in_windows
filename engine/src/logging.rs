use std::{
    collections::VecDeque,
    fs, io,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Clone)]
pub struct RingLogger {
    inner: Arc<LoggerInner>,
}

struct LoggerInner {
    enabled: AtomicBool,
    capacity: usize,
    entries: Mutex<VecDeque<String>>,
}

impl Default for RingLogger {
    fn default() -> Self {
        Self::with_capacity(10_000)
    }
}

impl RingLogger {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(LoggerInner {
                enabled: AtomicBool::new(false),
                capacity,
                entries: Mutex::new(VecDeque::with_capacity(capacity.min(1_024))),
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.enabled.store(enabled, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Acquire)
    }

    pub fn record(&self, message: impl AsRef<str>) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f").to_string();
        self.record_at(message, &timestamp);
    }

    pub fn record_at(&self, message: impl AsRef<str>, timestamp: &str) {
        if !self.is_enabled() || self.inner.capacity == 0 {
            return;
        }

        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while entries.len() >= self.inner.capacity {
            entries.pop_front();
        }
        entries.push_back(format!("[{timestamp}] {}", message.as_ref()));
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn export(&self, path: &Path) -> io::Result<()> {
        let mut contents = self.snapshot().join("\r\n");
        if !contents.is_empty() {
            contents.push_str("\r\n");
        }
        fs::write(path, contents)
    }
}
