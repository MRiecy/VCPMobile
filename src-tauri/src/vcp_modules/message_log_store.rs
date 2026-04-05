use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

pub struct MessageLogStore {
    base_path: PathBuf,
}

impl MessageLogStore {
    pub fn new(path: PathBuf) -> Self {
        Self { base_path: path }
    }

    /// Clears the file and prepares it for fresh writing
    pub fn truncate(&self) -> Result<(), String> {
        File::create(&self.base_path).map_err(|e| format!("Failed to truncate file: {}", e))?;
        Ok(())
    }

    /// Appends raw bytes to the file and returns (offset, length)
    pub fn append_raw(&self, bytes: &[u8]) -> Result<(u64, u64), String> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.base_path)
            .map_err(|e| format!("Failed to open file {}: {}", self.base_path.display(), e))?;

        let offset = file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;

        // Sync to OS to ensure it's written before DB update
        file.sync_data().map_err(|e| e.to_string())?;

        Ok((offset, bytes.len() as u64))
    }

    /// Specifically for astbin append (alias for append_raw)
    pub fn append_astbin(&self, bytes: &[u8]) -> Result<(u64, u64), String> {
        self.append_raw(bytes)
    }

    /// Reads raw bytes from the file at given offset and length
    pub fn read_at(&self, offset: u64, length: u64) -> Result<Vec<u8>, String> {
        if length == 0 {
            return Ok(Vec::new());
        }

        let mut file = File::open(&self.base_path)
            .map_err(|e| format!("Failed to open file {}: {}", self.base_path.display(), e))?;
        let file_len = file.metadata().map_err(|e| e.to_string())?.len();

        if offset + length > file_len {
            return Err(format!(
                "Read out of bounds: offset {} + length {} > file_len {}",
                offset, length, file_len
            ));
        }

        file.seek(SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;

        let mut buffer = vec![0u8; length as usize];
        use std::io::Read;
        file.read_exact(&mut buffer).map_err(|e| e.to_string())?;
        Ok(buffer)
    }

    /// Appends a single JSON line to history.jsonl and returns (offset, length)
    pub fn append_jsonl(&self, json_line: &str) -> Result<(u64, u64), String> {
        // Ensure it ends with newline
        let mut content = json_line.to_string();
        if !content.ends_with('\n') {
            content.push('\n');
        }

        self.append_raw(content.as_bytes())
    }

    /// Reads a JSONL line from the file at given offset and length
    pub fn read_jsonl_at(&self, offset: u64, length: u64) -> Result<String, String> {
        let buffer = self.read_at(offset, length)?;
        String::from_utf8(buffer).map_err(|e| format!("Invalid UTF-8: {}", e))
    }
}
