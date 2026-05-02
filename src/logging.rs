use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;

pub fn log_path() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ckwriter")
        .join("ckwriter.log")
}

struct TeeWriter {
    stderr: io::Stderr,
    file: std::fs::File,
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = self.stderr.write_all(buf);
        let _ = self.file.write_all(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let _ = self.stderr.flush();
        let _ = self.file.flush();
        Ok(())
    }
}

pub fn init() {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let writer: Box<dyn Write + Send + 'static> = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(file) => Box::new(TeeWriter { stderr: io::stderr(), file }),
        Err(e) => {
            eprintln!(
                "ckwriter: failed to open log file {}: {e} -- falling back to stderr only",
                path.display()
            );
            Box::new(io::stderr())
        }
    };

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(writer))
        .init();

    log::info!(
        "ckwriter starting (pid {}) -> log {}",
        std::process::id(),
        path.display()
    );
}
