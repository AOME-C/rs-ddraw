use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init() {
    if let Ok(mut guard) = LOG.lock() {
        if guard.is_none() {
            if let Ok(f) = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open("rs_ddraw.log")
            {
                let _ = writeln!(&f, "=== rs-ddraw log started ===");
                *guard = Some(f);
            }
        }
    }
}

pub fn log(msg: &str) {
    if let Ok(mut guard) = LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "{}", msg);
            let _ = f.flush();
        }
    }
}

#[macro_export]
macro_rules! dd_log {
    ($($arg:tt)*) => {
        $crate::log::log(&format!($($arg)*))
    };
}
