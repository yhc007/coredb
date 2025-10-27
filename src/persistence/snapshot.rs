use std::collections::HashMap;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::{Write, Read, BufReader, BufWriter};
use std::path::Path;
use crate::error::*;

/// 스냅샷 형식
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapshotFormat {
    Text,   // 사람이 읽을 수 있는 텍스트
    Binary, // 바이너리 (빠르고 작음)
}

/// 스냅샷 관리자
pub struct Snapshot {
    data_directory: String,
}

impl Snapshot {
    pub fn new(data_directory: String) -> Self {
        create_dir_all(&data_directory).expect("Failed to create data directory");
        Self { data_directory }
    }
    
    /// 데이터를 텍스트 파일로 저장
    pub fn save_text(&self, data: &str) -> Result<()> {
        let file_path = format!("{}/db_snapshot.txt", self.data_directory);
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)?;
        
        writeln!(file, "# CoreDB Persistent Database")?;
        writeln!(file, "# Auto-generated snapshot")?;
        writeln!(file, "")?;
        writeln!(file, "{}", data)?;
        
        Ok(())
    }
    
    /// 텍스트 파일에서 데이터 로드
    pub fn load_text(&self) -> Result<String> {
        let file_path = format!("{}/db_snapshot.txt", self.data_directory);
        
        if !Path::new(&file_path).exists() {
            return Err(CoreDBError::Generic {
                message: "No snapshot file found".to_string(),
            });
        }
        
        let mut file = File::open(&file_path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        
        // 헤더 라인 제거
        let lines: Vec<&str> = contents.lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .collect();
        
        Ok(lines.join("\n"))
    }
    
    /// WAL에 작업 기록
    pub fn write_wal(&self, operation: &str) -> Result<()> {
        let wal_path = format!("{}/wal.log", self.data_directory);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(wal_path)?;
        
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();
        writeln!(file, "{}\t{}", timestamp, operation)?;
        
        Ok(())
    }
    
    /// WAL 로그 읽기
    pub fn read_wal(&self) -> Result<Vec<String>> {
        let wal_path = format!("{}/wal.log", self.data_directory);
        
        if !Path::new(&wal_path).exists() {
            return Ok(Vec::new());
        }
        
        let file = File::open(&wal_path)?;
        let reader = BufReader::new(file);
        
        use std::io::BufRead;
        let mut operations = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if let Some((_timestamp, operation)) = line.split_once('\t') {
                operations.push(operation.to_string());
            }
        }
        
        Ok(operations)
    }
    
    /// WAL 클리어
    pub fn clear_wal(&self) -> Result<()> {
        let wal_path = format!("{}/wal.log", self.data_directory);
        if Path::new(&wal_path).exists() {
            std::fs::remove_file(&wal_path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_snapshot_save_load() {
        let snapshot = Snapshot::new("./test_data".to_string());
        
        let test_data = "TEST_DATA\nLINE2\nLINE3";
        snapshot.save_text(test_data).unwrap();
        
        let loaded = snapshot.load_text().unwrap();
        assert_eq!(loaded, test_data);
        
        // 정리
        std::fs::remove_dir_all("./test_data").ok();
    }
    
    #[test]
    fn test_wal_operations() {
        let snapshot = Snapshot::new("./test_wal".to_string());
        
        snapshot.write_wal("INSERT test 1").unwrap();
        snapshot.write_wal("INSERT test 2").unwrap();
        snapshot.write_wal("DELETE test 1").unwrap();
        
        let operations = snapshot.read_wal().unwrap();
        assert_eq!(operations.len(), 3);
        assert_eq!(operations[0], "INSERT test 1");
        assert_eq!(operations[1], "INSERT test 2");
        assert_eq!(operations[2], "DELETE test 1");
        
        snapshot.clear_wal().unwrap();
        let operations = snapshot.read_wal().unwrap();
        assert_eq!(operations.len(), 0);
        
        // 정리
        std::fs::remove_dir_all("./test_wal").ok();
    }
}

