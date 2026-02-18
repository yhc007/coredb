use std::fs::{File, create_dir_all};
use std::io::{Write, Read, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use crate::schema::{TableSchema, CassandraValue, Row, PartitionKey, ClusteringKey, Cell};
use crate::error::*;

/// 백업 메타데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub version: String,
    pub created_at: DateTime<Utc>,
    pub database_name: String,
    pub keyspace_count: usize,
    pub table_count: usize,
    pub total_rows: usize,
    pub compression: Option<String>,
}

/// 키스페이스 백업 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyspaceBackup {
    pub name: String,
    pub replication_factor: u32,
    pub tables: Vec<TableBackup>,
}

/// 테이블 백업 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableBackup {
    pub name: String,
    pub schema: TableSchemaBackup,
    pub rows: Vec<RowBackup>,
    pub indexes: Vec<IndexBackup>,
}

/// 테이블 스키마 백업
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchemaBackup {
    pub partition_key_columns: Vec<ColumnBackup>,
    pub clustering_key_columns: Vec<ColumnBackup>,
    pub regular_columns: Vec<ColumnBackup>,
    pub static_columns: Vec<ColumnBackup>,
}

/// 컬럼 백업
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnBackup {
    pub name: String,
    pub data_type: String,
    pub is_static: bool,
}

/// 행 백업
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowBackup {
    pub partition_key: Vec<CassandraValue>,
    pub clustering_key: Option<Vec<CassandraValue>>,
    pub cells: HashMap<String, CellBackup>,
    pub timestamp: i64,
}

/// 셀 백업
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellBackup {
    pub value: CassandraValue,
    pub timestamp: i64,
    pub ttl: Option<u32>,
}

/// 인덱스 백업
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBackup {
    pub name: String,
    pub column: String,
}

/// 전체 백업 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullBackup {
    pub metadata: BackupMetadata,
    pub keyspaces: Vec<KeyspaceBackup>,
}

/// 백업 포맷
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackupFormat {
    Json,       // 사람이 읽을 수 있는 JSON
    JsonPretty, // 포맷팅된 JSON
    Binary,     // bincode (빠르고 작음)
}

/// 백업 매니저
pub struct BackupManager {
    backup_directory: PathBuf,
}

impl BackupManager {
    pub fn new<P: AsRef<Path>>(backup_directory: P) -> Self {
        let path = backup_directory.as_ref().to_path_buf();
        create_dir_all(&path).expect("Failed to create backup directory");
        Self { backup_directory: path }
    }
    
    /// 백업 파일 경로 생성
    fn backup_path(&self, name: &str, format: BackupFormat) -> PathBuf {
        let ext = match format {
            BackupFormat::Json | BackupFormat::JsonPretty => "json",
            BackupFormat::Binary => "bin",
        };
        self.backup_directory.join(format!("{}.{}", name, ext))
    }
    
    /// 백업 생성
    pub fn create_backup(&self, backup: &FullBackup, name: &str, format: BackupFormat) -> Result<PathBuf> {
        let path = self.backup_path(name, format);
        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);
        
        match format {
            BackupFormat::Json => {
                serde_json::to_writer(&mut writer, backup)?;
            },
            BackupFormat::JsonPretty => {
                serde_json::to_writer_pretty(&mut writer, backup)?;
            },
            BackupFormat::Binary => {
                let data = bincode::serialize(backup)?;
                writer.write_all(&data)?;
            },
        }
        
        writer.flush()?;
        Ok(path)
    }
    
    /// 백업에서 복원
    pub fn restore_backup(&self, name: &str, format: BackupFormat) -> Result<FullBackup> {
        let path = self.backup_path(name, format);
        
        if !path.exists() {
            return Err(CoreDBError::Generic {
                message: format!("Backup file not found: {:?}", path),
            });
        }
        
        let file = File::open(&path)?;
        let reader = BufReader::new(file);
        
        let backup: FullBackup = match format {
            BackupFormat::Json | BackupFormat::JsonPretty => {
                serde_json::from_reader(reader)?
            },
            BackupFormat::Binary => {
                let mut data = Vec::new();
                let mut file = File::open(&path)?;
                file.read_to_end(&mut data)?;
                bincode::deserialize(&data)?
            },
        };
        
        Ok(backup)
    }
    
    /// 백업 파일에서 직접 복원
    pub fn restore_from_file<P: AsRef<Path>>(&self, path: P) -> Result<FullBackup> {
        let path = path.as_ref();
        
        if !path.exists() {
            return Err(CoreDBError::Generic {
                message: format!("Backup file not found: {:?}", path),
            });
        }
        
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        
        let backup: FullBackup = match ext {
            "json" => serde_json::from_reader(reader)?,
            "bin" => {
                let mut data = Vec::new();
                let mut file = File::open(path)?;
                file.read_to_end(&mut data)?;
                bincode::deserialize(&data)?
            },
            _ => {
                return Err(CoreDBError::Generic {
                    message: format!("Unknown backup format: {}", ext),
                });
            }
        };
        
        Ok(backup)
    }
    
    /// 백업 목록 조회
    pub fn list_backups(&self) -> Result<Vec<BackupInfo>> {
        let mut backups = Vec::new();
        
        for entry in std::fs::read_dir(&self.backup_directory)? {
            let entry = entry?;
            let path = entry.path();
            
            if let Some(ext) = path.extension() {
                if ext == "json" || ext == "bin" {
                    let name = path.file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    
                    let metadata = entry.metadata()?;
                    let size = metadata.len();
                    let modified = metadata.modified().ok();
                    
                    let format = if ext == "json" {
                        BackupFormat::Json
                    } else {
                        BackupFormat::Binary
                    };
                    
                    backups.push(BackupInfo {
                        name,
                        path,
                        size,
                        modified,
                        format,
                    });
                }
            }
        }
        
        // 최신순 정렬
        backups.sort_by(|a, b| b.modified.cmp(&a.modified));
        
        Ok(backups)
    }
    
    /// 백업 삭제
    pub fn delete_backup(&self, name: &str, format: BackupFormat) -> Result<()> {
        let path = self.backup_path(name, format);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
    
    /// 백업 검증
    pub fn verify_backup(&self, name: &str, format: BackupFormat) -> Result<BackupVerification> {
        let backup = self.restore_backup(name, format)?;
        
        let mut total_rows = 0;
        let mut tables_verified = Vec::new();
        
        for keyspace in &backup.keyspaces {
            for table in &keyspace.tables {
                total_rows += table.rows.len();
                tables_verified.push(format!("{}.{}", keyspace.name, table.name));
            }
        }
        
        Ok(BackupVerification {
            is_valid: true,
            keyspace_count: backup.keyspaces.len(),
            table_count: tables_verified.len(),
            row_count: total_rows,
            tables: tables_verified,
            errors: vec![],
        })
    }
}

/// 백업 정보
#[derive(Debug, Clone)]
pub struct BackupInfo {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
    pub format: BackupFormat,
}

/// 백업 검증 결과
#[derive(Debug, Clone)]
pub struct BackupVerification {
    pub is_valid: bool,
    pub keyspace_count: usize,
    pub table_count: usize,
    pub row_count: usize,
    pub tables: Vec<String>,
    pub errors: Vec<String>,
}

impl FullBackup {
    /// 새 백업 생성 (빈 상태)
    pub fn new(database_name: &str) -> Self {
        Self {
            metadata: BackupMetadata {
                version: "1.0".to_string(),
                created_at: Utc::now(),
                database_name: database_name.to_string(),
                keyspace_count: 0,
                table_count: 0,
                total_rows: 0,
                compression: None,
            },
            keyspaces: Vec::new(),
        }
    }
    
    /// 키스페이스 추가
    pub fn add_keyspace(&mut self, keyspace: KeyspaceBackup) {
        self.metadata.table_count += keyspace.tables.len();
        for table in &keyspace.tables {
            self.metadata.total_rows += table.rows.len();
        }
        self.metadata.keyspace_count += 1;
        self.keyspaces.push(keyspace);
    }
}

impl From<&Row> for RowBackup {
    fn from(row: &Row) -> Self {
        let cells: HashMap<String, CellBackup> = row.cells.iter()
            .map(|(name, cell)| {
                (name.clone(), CellBackup {
                    value: cell.value.clone(),
                    timestamp: cell.timestamp,
                    ttl: cell.ttl,
                })
            })
            .collect();
        
        Self {
            partition_key: row.partition_key.components.clone(),
            clustering_key: row.clustering_key.as_ref().map(|ck| ck.components.clone()),
            cells,
            timestamp: row.timestamp,
        }
    }
}

impl RowBackup {
    /// Row로 변환
    pub fn to_row(&self) -> Row {
        let cells: HashMap<String, Cell> = self.cells.iter()
            .map(|(name, cell)| {
                (name.clone(), Cell {
                    value: cell.value.clone(),
                    timestamp: cell.timestamp,
                    ttl: cell.ttl,
                    is_deleted: false,
                })
            })
            .collect();
        
        Row {
            partition_key: PartitionKey {
                components: self.partition_key.clone(),
            },
            clustering_key: self.clustering_key.as_ref().map(|ck| ClusteringKey {
                components: ck.clone(),
            }),
            cells,
            timestamp: self.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_backup_create_restore() {
        let manager = BackupManager::new("./test_backup");
        
        let mut backup = FullBackup::new("test_db");
        backup.add_keyspace(KeyspaceBackup {
            name: "test_ks".to_string(),
            replication_factor: 1,
            tables: vec![TableBackup {
                name: "users".to_string(),
                schema: TableSchemaBackup {
                    partition_key_columns: vec![ColumnBackup {
                        name: "id".to_string(),
                        data_type: "INT".to_string(),
                        is_static: false,
                    }],
                    clustering_key_columns: vec![],
                    regular_columns: vec![ColumnBackup {
                        name: "name".to_string(),
                        data_type: "TEXT".to_string(),
                        is_static: false,
                    }],
                    static_columns: vec![],
                },
                rows: vec![RowBackup {
                    partition_key: vec![CassandraValue::Int(1)],
                    clustering_key: None,
                    cells: {
                        let mut cells = HashMap::new();
                        cells.insert("name".to_string(), CellBackup {
                            value: CassandraValue::Text("Alice".to_string()),
                            timestamp: 0,
                            ttl: None,
                        });
                        cells
                    },
                    timestamp: 0,
                }],
                indexes: vec![],
            }],
        });
        
        // JSON 백업
        let path = manager.create_backup(&backup, "test_backup", BackupFormat::JsonPretty).unwrap();
        assert!(path.exists());
        
        // 복원
        let restored = manager.restore_backup("test_backup", BackupFormat::JsonPretty).unwrap();
        assert_eq!(restored.keyspaces.len(), 1);
        assert_eq!(restored.keyspaces[0].tables.len(), 1);
        assert_eq!(restored.keyspaces[0].tables[0].rows.len(), 1);
        
        // 검증
        let verification = manager.verify_backup("test_backup", BackupFormat::JsonPretty).unwrap();
        assert!(verification.is_valid);
        assert_eq!(verification.row_count, 1);
        
        // 정리
        std::fs::remove_dir_all("./test_backup").ok();
    }
    
    #[test]
    fn test_backup_binary_format() {
        let manager = BackupManager::new("./test_backup_bin");
        
        let mut backup = FullBackup::new("test_db");
        backup.add_keyspace(KeyspaceBackup {
            name: "test_ks".to_string(),
            replication_factor: 1,
            tables: vec![],
        });
        
        // Binary 백업
        let path = manager.create_backup(&backup, "test_bin", BackupFormat::Binary).unwrap();
        assert!(path.exists());
        
        // 복원
        let restored = manager.restore_backup("test_bin", BackupFormat::Binary).unwrap();
        assert_eq!(restored.keyspaces.len(), 1);
        
        // 정리
        std::fs::remove_dir_all("./test_backup_bin").ok();
    }
}
