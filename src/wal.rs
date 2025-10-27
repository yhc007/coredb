use std::path::PathBuf;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncWriteExt, AsyncReadExt, BufWriter, SeekFrom};
use serde::{Serialize, Deserialize};
use crate::schema::{PartitionKey, ClusteringKey, Row};
use crate::error::*;

/// 커밋 로그 엔트리
#[derive(Debug, Serialize, Deserialize)]
pub struct CommitLogEntry {
    pub keyspace: String,
    pub table: String,
    pub mutation: Mutation,
    pub timestamp: i64,
}

/// 뮤테이션 타입
#[derive(Debug, Serialize, Deserialize)]
pub enum Mutation {
    Insert(Row),
    Delete { 
        partition_key: PartitionKey, 
        clustering_key: Option<ClusteringKey> 
    },
    PartitionDelete { 
        partition_key: PartitionKey 
    },
}

/// 커밋 로그
pub struct CommitLog {
    current_segment: BufWriter<File>,
    segment_size_limit: u64,
    current_segment_size: u64,
    base_directory: PathBuf,
    segment_id: u64,
}

impl CommitLog {
    pub async fn new(base_dir: PathBuf) -> Result<Self> {
        tokio::fs::create_dir_all(&base_dir).await?;
        
        let segment_path = base_dir.join(format!("commitlog-{}.log", 0));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(segment_path)
            .await?;
        
        Ok(Self {
            current_segment: BufWriter::new(file),
            segment_size_limit: 32 * 1024 * 1024, // 32MB
            current_segment_size: 0,
            base_directory: base_dir,
            segment_id: 0,
        })
    }
    
    pub async fn append(&mut self, entry: CommitLogEntry) -> Result<()> {
        let serialized = bincode::serialize(&entry)?;
        let entry_size = serialized.len() as u64;
        
        // 세그먼트 크기 초과 시 새 세그먼트 생성
        if self.current_segment_size + entry_size + 4 > self.segment_size_limit {
            self.rotate_segment().await?;
        }
        
        // 엔트리 크기 + 데이터 쓰기
        self.current_segment.write_u32(serialized.len() as u32).await?;
        self.current_segment.write_all(&serialized).await?;
        self.current_segment.flush().await?;
        
        self.current_segment_size += entry_size + 4; // +4 for length prefix
        
        Ok(())
    }
    
    async fn rotate_segment(&mut self) -> Result<()> {
        self.current_segment.flush().await?;
        
        self.segment_id += 1;
        let new_segment_path = self.base_directory
            .join(format!("commitlog-{}.log", self.segment_id));
        
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(new_segment_path)
            .await?;
        
        self.current_segment = BufWriter::new(file);
        self.current_segment_size = 0;
        
        Ok(())
    }
    
    /// 복구를 위한 replay 기능
    pub async fn replay_from_segment(&self, segment_id: u64) -> Result<Vec<CommitLogEntry>> {
        let segment_path = self.base_directory
            .join(format!("commitlog-{}.log", segment_id));
        
        if !segment_path.exists() {
            return Ok(Vec::new());
        }
        
        let mut file = File::open(segment_path).await?;
        let mut entries = Vec::new();
        
        loop {
            // 엔트리 크기 읽기
            let mut size_buf = [0u8; 4];
            match file.read_exact(&mut size_buf).await {
                Ok(_) => {
                    let entry_size = u32::from_le_bytes(size_buf) as usize;
                    
                    // 엔트리 데이터 읽기
                    let mut entry_buf = vec![0u8; entry_size];
                    file.read_exact(&mut entry_buf).await?;
                    
                    // 역직렬화
                    let entry: CommitLogEntry = bincode::deserialize(&entry_buf)?;
                    entries.push(entry);
                },
                Err(_) => break, // 파일 끝
            }
        }
        
        Ok(entries)
    }
    
    /// 모든 세그먼트에서 replay
    pub async fn replay_all(&self) -> Result<Vec<CommitLogEntry>> {
        let mut all_entries = Vec::new();
        let mut segment_id = 0;
        
        loop {
            let entries = self.replay_from_segment(segment_id).await?;
            if entries.is_empty() {
                break;
            }
            all_entries.extend(entries);
            segment_id += 1;
        }
        
        Ok(all_entries)
    }
    
    /// 오래된 세그먼트 정리
    pub async fn cleanup_old_segments(&self, keep_segments: u64) -> Result<()> {
        let mut segment_id = 0;
        
        loop {
            let segment_path = self.base_directory
                .join(format!("commitlog-{}.log", segment_id));
            
            if !segment_path.exists() {
                break;
            }
            
            if segment_id < self.segment_id.saturating_sub(keep_segments) {
                tokio::fs::remove_file(&segment_path).await?;
            }
            
            segment_id += 1;
        }
        
        Ok(())
    }
    
    /// 현재 세그먼트 ID
    pub fn current_segment_id(&self) -> u64 {
        self.segment_id
    }
    
    /// 현재 세그먼트 크기
    pub fn current_segment_size(&self) -> u64 {
        self.current_segment_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, Cell};
    use std::collections::HashMap;
    
    fn create_test_row() -> Row {
        Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(1)],
            },
            clustering_key: Some(ClusteringKey {
                components: vec![CassandraValue::BigInt(1000)],
            }),
            cells: {
                let mut cells = HashMap::new();
                cells.insert("value".to_string(), Cell {
                    value: CassandraValue::Text("test".to_string()),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }
    
    #[tokio::test]
    async fn test_commit_log_append_and_replay() {
        let temp_dir = std::env::temp_dir().join("coredb_wal_test");
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        
        let mut commit_log = CommitLog::new(temp_dir.clone()).await.unwrap();
        
        let entry = CommitLogEntry {
            keyspace: "test_keyspace".to_string(),
            table: "test_table".to_string(),
            mutation: Mutation::Insert(create_test_row()),
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        commit_log.append(entry).await.unwrap();
        
        // 세그먼트 강제 플러시
        commit_log.current_segment.flush().await.unwrap();
        
        let entries = commit_log.replay_from_segment(0).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].keyspace, "test_keyspace");
        assert_eq!(entries[0].table, "test_table");
        
        // 정리
        tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
    }
    
    #[tokio::test]
    async fn test_commit_log_segment_rotation() {
        let temp_dir = std::env::temp_dir().join("coredb_wal_rotation_test");
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        
        let mut commit_log = CommitLog::new(temp_dir.clone()).await.unwrap();
        
        // 작은 세그먼트 크기로 설정
        commit_log.segment_size_limit = 1024; // 1KB
        
        // 여러 엔트리 추가하여 세그먼트 로테이션 트리거
        for i in 0..10 {
            let entry = CommitLogEntry {
                keyspace: "test_keyspace".to_string(),
                table: "test_table".to_string(),
                mutation: Mutation::Insert(create_test_row()),
                timestamp: chrono::Utc::now().timestamp_micros(),
            };
            commit_log.append(entry).await.unwrap();
        }
        
        // 세그먼트가 로테이션되었는지 확인
        assert!(commit_log.segment_id > 0);
        
        // 정리
        tokio::fs::remove_dir_all(&temp_dir).await.unwrap();
    }
}
