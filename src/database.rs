use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use crate::schema::{TableSchema, KeyspaceDefinition, ReplicationStrategy};
use crate::storage::{Memtable, SSTable};
use crate::wal::{CommitLog, Mutation};
use crate::query::{QueryEngine, CqlStatement, QueryResult};
use crate::compaction::{CompactionManager, CompactionConfig};
use crate::error::*;

/// 데이터베이스 설정
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub data_directory: PathBuf,
    pub commitlog_directory: PathBuf,
    pub memtable_flush_threshold_mb: u64,
    pub compaction_throughput_mb_per_sec: u64,
    pub concurrent_reads: usize,
    pub concurrent_writes: usize,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            data_directory: PathBuf::from("./data"),
            commitlog_directory: PathBuf::from("./commitlog"),
            memtable_flush_threshold_mb: 64,
            compaction_throughput_mb_per_sec: 16,
            concurrent_reads: 32,
            concurrent_writes: 32,
        }
    }
}

/// 키스페이스
#[derive(Debug, Clone)]
pub struct Keyspace {
    pub name: String,
    pub definition: KeyspaceDefinition,
    pub tables: Arc<RwLock<HashMap<String, Table>>>,
}

/// 테이블
#[derive(Debug)]
pub struct Table {
    pub schema: Arc<TableSchema>,
    pub memtables: Vec<Arc<Memtable>>,
    pub sstables: Vec<Arc<SSTable>>,
    pub current_memtable: Arc<Memtable>,
}

/// CoreDB 메인 클래스
pub struct CoreDB {
    pub keyspaces: Arc<RwLock<HashMap<String, Keyspace>>>,
    pub commit_log: Arc<RwLock<CommitLog>>,
    pub query_engine: Arc<RwLock<QueryEngine>>,
    pub config: DatabaseConfig,
    pub compaction_manager: Arc<CompactionManager>,
}

impl CoreDB {
    /// 새 데이터베이스 인스턴스 생성
    pub async fn new(config: DatabaseConfig) -> Result<Self, CoreDBError> {
        // 디렉토리 생성
        tokio::fs::create_dir_all(&config.data_directory).await?;
        tokio::fs::create_dir_all(&config.commitlog_directory).await?;
        
        let commit_log = CommitLog::new(config.commitlog_directory.clone()).await?;
        let query_engine = QueryEngine::new();
        
        let compaction_config = CompactionConfig {
            throughput_mb_per_sec: config.compaction_throughput_mb_per_sec,
            max_concurrent_compactions: 2,
            strategy: crate::compaction::CompactionStrategy::SizeTiered {
                min_threshold: 4,
                max_threshold: 32,
            },
            data_directory: config.data_directory.clone(),
        };
        
        let compaction_manager = CompactionManager::new(compaction_config);
        
        let mut db = Self {
            keyspaces: Arc::new(RwLock::new(HashMap::new())),
            commit_log: Arc::new(RwLock::new(commit_log)),
            query_engine: Arc::new(RwLock::new(query_engine)),
            config,
            compaction_manager: Arc::new(compaction_manager),
        };
        
        // 시스템 키스페이스 초기화
        db.create_system_keyspaces().await?;
        
        // 백그라운드 작업 시작
        db.start_background_tasks().await;
        
        Ok(db)
    }
    
    /// CQL 쿼리 실행
    pub async fn execute_cql(&self, query: &str) -> Result<QueryResult, CoreDBError> {
        let parsed = crate::query::parser::CqlParser::parse(query)?;
        
        // 커밋 로그에 기록 (변경 작업인 경우)
        if self.is_mutation(&parsed) {
            self.log_mutation(&parsed).await?;
        }
        
        // 쿼리 엔진에서 실행
        let mut engine = self.query_engine.write().await;
        let result = engine.execute(parsed).await?;
        
        // 메모리 테이블 플러시 체크
        self.check_memtable_flush().await?;
        
        Ok(result)
    }
    
    /// 키스페이스 생성
    pub async fn create_keyspace(&self, name: String, replication_factor: u32) -> Result<(), CoreDBError> {
        let keyspace = Keyspace {
            name: name.clone(),
            definition: KeyspaceDefinition {
                name: name.clone(),
                replication_factor,
                strategy: ReplicationStrategy::SimpleStrategy,
            },
            tables: Arc::new(RwLock::new(HashMap::new())),
        };
        
        let mut keyspaces = self.keyspaces.write().await;
        keyspaces.insert(name, keyspace);
        
        Ok(())
    }
    
    /// 테이블 생성
    pub async fn create_table(&self, keyspace: String, table: String, schema: TableSchema) -> Result<(), CoreDBError> {
        schema.validate()?;
        
        let memtable = Arc::new(Memtable::new(Arc::new(schema.clone())));
        let table_struct = Table {
            schema: Arc::new(schema),
            memtables: Vec::new(),
            sstables: Vec::new(),
            current_memtable: memtable,
        };
        
        let keyspaces = self.keyspaces.read().await;
        if let Some(ks) = keyspaces.get(&keyspace) {
            let mut tables = ks.tables.write().await;
            tables.insert(table, table_struct);
        } else {
            return Err(CoreDBError::KeyspaceNotFound { keyspace });
        }
        
        Ok(())
    }
    
    /// 행 삽입
    pub async fn insert_row(&self, keyspace: &str, table: &str, row: crate::schema::Row) -> Result<(), CoreDBError> {
        // 커밋 로그에 기록
        let commit_entry = crate::wal::CommitLogEntry {
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            mutation: Mutation::Insert(row.clone()),
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        self.commit_log.write().await.append(commit_entry).await?;
        
        // 메모리 테이블에 추가
        let keyspaces = self.keyspaces.read().await;
        if let Some(ks) = keyspaces.get(keyspace) {
            let tables = ks.tables.read().await;
            if let Some(tbl) = tables.get(table) {
                tbl.current_memtable.put(row)?;
            } else {
                return Err(CoreDBError::TableNotFound { table: table.to_string() });
            }
        } else {
            return Err(CoreDBError::KeyspaceNotFound { keyspace: keyspace.to_string() });
        }
        
        // 메모리 테이블 크기 체크 및 플러시
        self.check_memtable_flush().await?;
        
        Ok(())
    }
    
    /// 행 조회
    pub async fn get_row(&self, keyspace: &str, table: &str, partition_key: &crate::schema::PartitionKey, clustering_key: &Option<crate::schema::ClusteringKey>) -> Result<Option<crate::schema::Row>, CoreDBError> {
        let keyspaces = self.keyspaces.read().await;
        if let Some(ks) = keyspaces.get(keyspace) {
            let tables = ks.tables.read().await;
            if let Some(tbl) = tables.get(table) {
                // 메모리 테이블에서 먼저 검색
                if let Some(row) = tbl.current_memtable.get(partition_key, clustering_key) {
                    return Ok(Some(row));
                }
                
                // SSTable에서 검색
                for sstable in &tbl.sstables {
                    if let Some(row) = sstable.read_partition(partition_key).await? {
                        // 클러스터링 키가 있다면 해당 행만 반환
                        if let Some(ref ck) = clustering_key {
                            // 파티션 내에서 클러스터링 키로 검색하는 로직 필요
                            // 현재는 단순화된 버전
                            return Ok(Some(row));
                        }
                        return Ok(Some(row));
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// 메모리 테이블 플러시 체크
    async fn check_memtable_flush(&self) -> Result<(), CoreDBError> {
        let keyspaces = self.keyspaces.read().await;
        
        for (keyspace_name, keyspace) in keyspaces.iter() {
            let tables = keyspace.tables.read().await;
            
            for (table_name, table) in tables.iter() {
                if table.current_memtable.size_bytes() > self.config.memtable_flush_threshold_mb * 1024 * 1024 {
                    self.flush_memtable(keyspace_name, table_name).await?;
                }
            }
        }
        
        Ok(())
    }
    
    /// 메모리 테이블 플러시
    async fn flush_memtable(&self, keyspace: &str, table: &str) -> Result<(), CoreDBError> {
        let mut keyspaces = self.keyspaces.write().await;
        if let Some(ks) = keyspaces.get_mut(keyspace) {
            let mut tables = ks.tables.write().await;
            if let Some(tbl) = tables.get_mut(table) {
                // 새 메모리 테이블 생성
                let new_memtable = Arc::new(Memtable::new(tbl.schema.clone()));
                let old_memtable = std::mem::replace(&mut tbl.current_memtable, new_memtable);
                
                // 기존 메모리 테이블을 SSTable로 변환
                let sstable_dir = self.config.data_directory
                    .join(keyspace)
                    .join(table);
                tokio::fs::create_dir_all(&sstable_dir).await?;
                
                let sstable = SSTable::create_from_memtable(
                    &old_memtable,
                    &sstable_dir,
                    crate::storage::sstable::CompressionType::LZ4
                ).await?;
                
                tbl.sstables.push(Arc::new(sstable));
                
                // 컴팩션 트리거
                self.compaction_manager.schedule_compaction(keyspace, table).await;
            }
        }
        
        Ok(())
    }
    
    /// 시스템 키스페이스 생성
    async fn create_system_keyspaces(&mut self) -> Result<(), CoreDBError> {
        // 시스템 키스페이스 생성
        self.create_keyspace("system".to_string(), 1).await?;
        self.create_keyspace("system_schema".to_string(), 1).await?;
        
        Ok(())
    }
    
    /// 백그라운드 작업 시작
    async fn start_background_tasks(&self) {
        // 컴팩션 스케줄러
        let compaction_manager = self.compaction_manager.clone();
        tokio::spawn(async move {
            compaction_manager.run_compaction_loop().await;
        });
        
        // TTL 정리 작업
        let keyspaces = self.keyspaces.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                // TTL 만료된 데이터 정리
                Self::cleanup_expired_data(&keyspaces).await;
            }
        });
    }
    
    /// 만료된 데이터 정리
    async fn cleanup_expired_data(keyspaces: &Arc<RwLock<HashMap<String, Keyspace>>>) {
        // TTL 만료된 데이터 정리 로직
        // 현재는 플레이스홀더
        let _keyspaces = keyspaces.read().await;
        // TODO: TTL 체크 및 삭제 로직 구현
    }
    
    /// 뮤테이션인지 확인
    fn is_mutation(&self, statement: &CqlStatement) -> bool {
        matches!(statement, 
            CqlStatement::Insert { .. } | 
            CqlStatement::Update { .. } | 
            CqlStatement::Delete { .. } |
            CqlStatement::CreateKeyspace { .. } |
            CqlStatement::CreateTable { .. } |
            CqlStatement::DropTable { .. } |
            CqlStatement::DropKeyspace { .. }
        )
    }
    
    /// 커밋 로그에 뮤테이션 기록
    async fn log_mutation(&self, statement: &CqlStatement) -> Result<(), CoreDBError> {
        let mutation = match statement {
            CqlStatement::Insert { keyspace, table, values } => {
                // 값들을 Row로 변환하는 로직이 필요
                // 현재는 단순화된 버전
                todo!("Convert INSERT values to Row")
            },
            _ => {
                // 다른 뮤테이션 타입들 처리
                return Ok(());
            }
        };
        
        let commit_entry = crate::wal::CommitLogEntry {
            keyspace: "system".to_string(), // 임시
            table: "mutations".to_string(), // 임시
            mutation,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        self.commit_log.write().await.append(commit_entry).await?;
        Ok(())
    }
    
    /// 데이터베이스 통계
    pub async fn get_stats(&self) -> DatabaseStats {
        let keyspaces = self.keyspaces.read().await;
        let mut total_tables = 0;
        let mut total_memtables = 0;
        let mut total_sstables = 0;
        let mut total_size_bytes = 0u64;
        
        for keyspace in keyspaces.values() {
            let tables = keyspace.tables.read().await;
            total_tables += tables.len();
            
            for table in tables.values() {
                total_memtables += 1; // current_memtable
                total_sstables += table.sstables.len();
                total_size_bytes += table.current_memtable.size_bytes();
                
                for sstable in &table.sstables {
                    total_size_bytes += sstable.size_bytes;
                }
            }
        }
        
        DatabaseStats {
            keyspace_count: keyspaces.len(),
            table_count: total_tables,
            memtable_count: total_memtables,
            sstable_count: total_sstables,
            total_size_bytes,
        }
    }
    
    /// 데이터베이스 종료
    pub async fn shutdown(&self) -> Result<(), CoreDBError> {
        // 모든 메모리 테이블 플러시
        let keyspaces = self.keyspaces.read().await;
        for (keyspace_name, keyspace) in keyspaces.iter() {
            let tables = keyspace.tables.read().await;
            for (table_name, _) in tables.iter() {
                self.flush_memtable(keyspace_name, table_name).await?;
            }
        }
        
        Ok(())
    }
}

/// 데이터베이스 통계
#[derive(Debug)]
pub struct DatabaseStats {
    pub keyspace_count: usize,
    pub table_count: usize,
    pub memtable_count: usize,
    pub sstable_count: usize,
    pub total_size_bytes: u64,
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, CassandraDataType, TableSchema, PartitionKey, CassandraValue};
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_coredb_creation() {
        let config = DatabaseConfig::default();
        let db = CoreDB::new(config).await.unwrap();
        
        let stats = db.get_stats().await;
        assert!(stats.keyspace_count >= 2); // system keyspaces
    }
    
    #[tokio::test]
    async fn test_keyspace_creation() {
        let config = DatabaseConfig::default();
        let db = CoreDB::new(config).await.unwrap();
        
        db.create_keyspace("test_ks".to_string(), 1).await.unwrap();
        
        let stats = db.get_stats().await;
        assert!(stats.keyspace_count >= 3); // system + test_ks
    }
    
    #[tokio::test]
    async fn test_table_creation() {
        let config = DatabaseConfig::default();
        let db = CoreDB::new(config).await.unwrap();
        
        db.create_keyspace("test_ks".to_string(), 1).await.unwrap();
        
        let schema = TableSchema::new(
            "test_table".to_string(),
            "test_ks".to_string(),
            vec![ColumnDefinition {
                name: "id".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            }],
            vec![],
            vec![ColumnDefinition {
                name: "name".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            }],
            vec![],
        );
        
        db.create_table("test_ks".to_string(), "test_table".to_string(), schema).await.unwrap();
        
        let stats = db.get_stats().await;
        assert!(stats.table_count >= 1);
    }
    
    #[tokio::test]
    async fn test_cql_execution() {
        let config = DatabaseConfig::default();
        let db = CoreDB::new(config).await.unwrap();
        
        let result = db.execute_cql("CREATE KEYSPACE test_ks WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql("CREATE TABLE test_ks.test_table (id INT PRIMARY KEY, name TEXT)").await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql("INSERT INTO test_ks.test_table (id, name) VALUES (1, 'John')").await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql("SELECT * FROM test_ks.test_table WHERE id = 1").await.unwrap();
        assert!(result.is_success());
    }
}
