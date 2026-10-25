//! Append across restarts and bound disk usage, including long-running development sessions.
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const ARCHIVES: usize = 4;

pub struct LogWriter {
    path: PathBuf,
    file: Option<File>,
    bytes: u64,
}

impl LogWriter {
    pub fn open(directory: &Path) -> io::Result<Self> {
        let path = directory.join("p2pcli.log");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let bytes = file.metadata()?.len();
        Ok(Self { path, file: Some(file), bytes })
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Close before renaming on Windows; always reopen even if rotation fails.
        self.file.take();
        let result = (|| {
            for index in (1..=ARCHIVES).rev() {
                let destination = self.path.with_file_name(format!("p2pcli.log.{index}"));
                let source = if index == 1 { self.path.clone() }
                    else { self.path.with_file_name(format!("p2pcli.log.{}", index - 1)) };
                if source.exists() {
                    if destination.exists() { std::fs::remove_file(&destination)?; }
                    std::fs::rename(source, destination)?;
                }
            }
            Ok(())
        })();
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        self.bytes = file.metadata()?.len();
        self.file = Some(file);
        result
    }
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes >= MAX_BYTES {
            if let Err(error) = self.rotate() { eprintln!("日志轮转失败：{error}"); }
        }
        let file = self.file.as_mut().ok_or_else(|| io::Error::other("日志文件不可用"))?;
        let written = file.write(bytes)?;
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file { Some(file) => file.flush(), None => Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_previous_runs_and_rotates_without_losing_latest_record() {
        let directory = std::env::temp_dir().join(format!("p2pcli-log-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&directory).unwrap();
        {
            let mut first = LogWriter::open(&directory).unwrap();
            first.write_all(b"previous run\n").unwrap();
        }
        {
            let mut next = LogWriter::open(&directory).unwrap();
            next.write_all(b"next run\n").unwrap();
            assert_eq!(std::fs::read(directory.join("p2pcli.log")).unwrap(), b"previous run\nnext run\n");
            next.file.as_ref().unwrap().set_len(MAX_BYTES).unwrap();
            next.bytes = MAX_BYTES;
            next.write_all(b"failure evidence\n").unwrap();
            assert_eq!(std::fs::read(directory.join("p2pcli.log")).unwrap(), b"failure evidence\n");
            assert_eq!(std::fs::metadata(directory.join("p2pcli.log.1")).unwrap().len(), MAX_BYTES);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
