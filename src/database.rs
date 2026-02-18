use std::path::PathBuf;
use std::sync::Arc;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use crate::schema::{TableSchema, KeyspaceDefinition, ReplicationStrategy, PartitionKey, CassandraValue};
use crate::storage::{Memtable, SSTable, BlockCache, CacheConfig, CacheKey, IndexManager, IndexDefinition};
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
    /// Block Cache 최대 크기 (MB)
    pub block_cache_size_mb: usize,
    /// Block Cache 최대 엔트리 수
    pub block_cache_max_entries: usize,
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
            block_cache_size_mb: 128, // 128MB 기본값
            block_cache_max_entries: 10_000,
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
    /// Block Cache (읽기 성능 최적화)
    pub block_cache: Arc<BlockCache>,
    /// Secondary Index Manager
    pub index_manager: Arc<IndexManager>,
}

impl CoreDB {
    /// 새 데이터베이스 인스턴스 생성
    pub async fn new(config: DatabaseConfig) -> Result<Self> {
        // 디렉토리 생성
        tokio::fs::create_dir_all(&config.data_directory).await?;
        tokio::fs::create_dir_all(&config.commitlog_directory).await?;
        
        let keyspaces = Arc::new(RwLock::new(HashMap::new()));
        
        let commit_log = CommitLog::new(config.commitlog_directory.clone()).await?;
        let query_engine = QueryEngine::new(keyspaces.clone());
        
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
        
        // Block Cache 초기화
        let cache_config = CacheConfig {
            max_size_bytes: config.block_cache_size_mb * 1024 * 1024,
            max_entries: config.block_cache_max_entries,
            num_shards: 16,
        };
        let block_cache = Arc::new(BlockCache::new(cache_config));
        
        // Index Manager 초기화
        let index_manager = Arc::new(IndexManager::new());
        
        let mut db = Self {
            keyspaces,
            commit_log: Arc::new(RwLock::new(commit_log)),
            query_engine: Arc::new(RwLock::new(query_engine)),
            config,
            compaction_manager: Arc::new(compaction_manager),
            block_cache,
            index_manager,
        };
        
        // 시스템 키스페이스 초기화
        db.create_system_keyspaces().await?;
        
        // 기존 데이터 로드
        db.load_existing_data().await?;
        
        // 백그라운드 작업 시작
        db.start_background_tasks().await;
        
        Ok(db)
    }
    
    /// 기존 SSTable 데이터 로드
    async fn load_existing_data(&mut self) -> Result<()> {
        use tokio::fs;
        
        let data_dir = &self.config.data_directory;
        
        // data 디렉토리가 없으면 스킵
        if !data_dir.exists() {
            return Ok(());
        }
        
        let mut entries = fs::read_dir(data_dir).await?;
        
        while let Some(entry) = entries.next_entry().await? {
            let keyspace_name = entry.file_name().to_string_lossy().to_string();
            
            // 시스템 키스페이스는 스킵
            if keyspace_name.starts_with("system") {
                continue;
            }
            
            let keyspace_path = entry.path();
            if !keyspace_path.is_dir() {
                continue;
            }
            
            // 키스페이스 생성 (없으면)
            self.create_keyspace(keyspace_name.clone(), 1).await.ok();
            
            // 테이블 디렉토리 스캔
            let mut table_entries = fs::read_dir(&keyspace_path).await?;
            
            while let Some(table_entry) = table_entries.next_entry().await? {
                let table_name = table_entry.file_name().to_string_lossy().to_string();
                let table_path = table_entry.path();
                
                if !table_path.is_dir() {
                    continue;
                }
                
                // SSTable 파일 로드
                let mut sstable_files = fs::read_dir(&table_path).await?;
                let mut sstables: Vec<Arc<SSTable>> = Vec::new();
                
                while let Some(file_entry) = sstable_files.next_entry().await? {
                    let file_name = file_entry.file_name().to_string_lossy().to_string();
                    
                    if file_name.ends_with("-Data.db") {
                        if let Ok(sstable) = SSTable::open(&file_entry.path()).await {
                            sstables.push(Arc::new(sstable));
                        }
                    }
                }
                
                // 스키마 로드 및 테이블 생성
                if let Ok(Some(schema)) = self.load_table_schema(&keyspace_name, &table_name).await {
                    let schema_arc = Arc::new(schema);
                    let memtable = Memtable::new(schema_arc.clone());
                    
                    // SSTable 데이터를 Memtable에 로드 (재시작 시 데이터 복구)
                    let mut _loaded_count = 0;
                    for sstable in &sstables {
                        
                        for pk in sstable.partition_index.keys() {
                            match sstable.read_partition(pk).await {
                                Ok(Some(partition)) => {
                                    
                                    for entry in partition.rows.iter() {
                                        let _ = memtable.put(entry.value().clone());
                                        _loaded_count += 1;
                                    }
                                }
                                Ok(None) => eprintln!("[load] Partition not found"),
                                Err(e) => eprintln!("[load] Error reading partition: {}", e),
                            }
                        }
                    }
                    
                    
                    // 테이블 생성
                    let table_struct = Table {
                        schema: schema_arc,
                        memtables: Vec::new(),
                        sstables: sstables.clone(),
                        current_memtable: Arc::new(memtable),
                    };
                    
                    let mut keyspaces = self.keyspaces.write().await;
                    if let Some(ks) = keyspaces.get_mut(&keyspace_name) {
                        let mut tables = ks.tables.write().await;
                        tables.insert(table_name.clone(), table_struct);
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// CQL 쿼리 실행
    pub async fn execute_cql(&self, query: &str) -> Result<QueryResult> {
        let parsed = crate::query::parser::CqlParser::parse(query)?;
        
        // CREATE INDEX 처리
        if let CqlStatement::CreateIndex { name, keyspace, table, column } = &parsed {
            return self.handle_create_index(name.clone(), keyspace, table, column).await;
        }
        
        // DROP INDEX 처리
        if let CqlStatement::DropIndex { keyspace, name } = &parsed {
            return self.handle_drop_index(keyspace, name).await;
        }
        
        // CREATE TABLE인 경우 스키마 저장 정보 추출
        let create_table_info = if let CqlStatement::CreateTable { ref keyspace, ref name, ref columns, ref partition_key, ref clustering_key, .. } = parsed {
            Some((keyspace.clone(), name.clone(), columns.clone(), partition_key.clone(), clustering_key.clone()))
        } else {
            None
        };
        
        // INSERT 시 인덱스 업데이트 정보 추출
        let insert_info = if let CqlStatement::Insert { ref keyspace, ref table, ref values, ref ttl } = parsed {
            Some((keyspace.clone(), table.clone(), values.clone(), *ttl))
        } else {
            None
        };
        
        // 커밋 로그에 기록 (변경 작업인 경우)
        if self.is_mutation(&parsed) {
            self.log_mutation(&parsed).await?;
        }
        
        // 쿼리 엔진에서 실행
        let mut engine = self.query_engine.write().await;
        let result = engine.execute(parsed).await?;
        
        // CREATE TABLE 성공 후 스키마 저장
        if let Some((keyspace, table_name, columns, partition_key, clustering_key)) = create_table_info {
            if matches!(result, QueryResult::Success) {
                // 스키마 재구성
                let mut pk_columns = Vec::new();
                let mut ck_columns = Vec::new();
                let mut regular_columns = Vec::new();
                let mut static_columns = Vec::new();
                
                for column in columns {
                    if partition_key.contains(&column.name) {
                        pk_columns.push(column);
                    } else if clustering_key.contains(&column.name) {
                        ck_columns.push(column);
                    } else if column.is_static {
                        static_columns.push(column);
                    } else {
                        regular_columns.push(column);
                    }
                }
                
                let schema = TableSchema::new(
                    table_name.clone(),
                    keyspace.clone(),
                    pk_columns,
                    ck_columns,
                    regular_columns,
                    static_columns,
                );
                
                self.save_table_schema(&keyspace, &table_name, &schema).await?;
            }
        }
        
        // INSERT 성공 후 인덱스 업데이트
        if let Some((keyspace, table, values, _ttl)) = insert_info {
            if matches!(result, QueryResult::Success) {
                self.update_indexes_on_insert(&keyspace, &table, &values).await;
            }
        }
        
        // 메모리 테이블 플러시 체크
        self.check_memtable_flush().await?;
        
        Ok(result)
    }
    
    /// CREATE INDEX 처리
    async fn handle_create_index(
        &self,
        name: Option<String>,
        keyspace: &str,
        table: &str,
        column: &str,
    ) -> Result<QueryResult> {
        // 테이블 존재 확인
        let keyspaces = self.keyspaces.read().await;
        let ks = keyspaces.get(keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.to_string(),
        })?;
        
        let tables = ks.tables.read().await;
        let tbl = tables.get(table).ok_or_else(|| CoreDBError::TableNotFound {
            table: format!("{}.{}", keyspace, table),
        })?;
        
        // 컬럼 존재 확인
        if tbl.schema.get_column(column).is_none() {
            return Err(CoreDBError::InvalidSchema {
                message: format!("Column '{}' not found in table '{}.{}'", column, keyspace, table),
            });
        }
        
        // 인덱스 이름 생성
        let index_name = name.unwrap_or_else(|| format!("{}_{}_idx", table, column));
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        
        let definition = IndexDefinition {
            name: index_name.clone(),
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            column: column.to_string(),
            created_at: now,
        };
        
        // 인덱스 생성
        self.index_manager.create_index(definition).map_err(|e| {
            CoreDBError::InvalidSchema { message: e }
        })?;
        
        // 기존 데이터로 인덱스 빌드
        self.build_index_from_existing_data(keyspace, table, column).await?;
        
        Ok(QueryResult::Success)
    }
    
    /// DROP INDEX 처리
    async fn handle_drop_index(&self, keyspace: &str, name: &str) -> Result<QueryResult> {
        self.index_manager.drop_index(keyspace, name).map_err(|e| {
            CoreDBError::InvalidSchema { message: e }
        })?;
        
        Ok(QueryResult::Success)
    }
    
    /// 기존 데이터로 인덱스 빌드
    async fn build_index_from_existing_data(
        &self,
        keyspace: &str,
        table: &str,
        column: &str,
    ) -> Result<()> {
        let keyspaces = self.keyspaces.read().await;
        if let Some(ks) = keyspaces.get(keyspace) {
            let tables = ks.tables.read().await;
            if let Some(tbl) = tables.get(table) {
                // Memtable에서 데이터 읽어서 인덱스 빌드
                let partitions = tbl.current_memtable.get_all_partitions();
                for (_, partition) in partitions {
                    for entry in partition.rows.iter() {
                        let row = entry.value();
                        if let Some(cell) = row.cells.get(column) {
                            self.index_manager.insert_to_index(
                                keyspace,
                                table,
                                column,
                                cell.value.clone(),
                                row.partition_key.clone(),
                            );
                        }
                    }
                }
                
                // SSTable에서도 데이터 로드하여 인덱스 빌드
                for sstable in &tbl.sstables {
                    for pk in sstable.partition_index.keys() {
                        if let Ok(Some(partition)) = sstable.read_partition(pk).await {
                            for entry in partition.rows.iter() {
                                let row = entry.value();
                                if let Some(cell) = row.cells.get(column) {
                                    self.index_manager.insert_to_index(
                                        keyspace,
                                        table,
                                        column,
                                        cell.value.clone(),
                                        row.partition_key.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// INSERT 후 인덱스 업데이트
    async fn update_indexes_on_insert(
        &self,
        keyspace: &str,
        table: &str,
        values: &[(String, CassandraValue)],
    ) {
        // 테이블에 정의된 인덱스 확인
        let indexes = self.index_manager.get_table_indexes(keyspace, table);
        
        if indexes.is_empty() {
            return;
        }
        
        // 파티션 키 추출 (첫 번째 값을 파티션 키로 가정 - 단순화)
        let pk = if let Some((_, value)) = values.first() {
            PartitionKey {
                components: vec![value.clone()],
            }
        } else {
            return;
        };
        
        // 각 인덱스에 대해 업데이트
        for index_def in indexes {
            if let Some((_, value)) = values.iter().find(|(col, _)| col == &index_def.column) {
                self.index_manager.insert_to_index(
                    keyspace,
                    table,
                    &index_def.column,
                    value.clone(),
                    pk.clone(),
                );
            }
        }
    }
    
    /// 인덱스를 사용한 쿼리 가능 여부 확인
    pub fn can_use_index(&self, keyspace: &str, table: &str, column: &str) -> bool {
        self.index_manager.has_index(keyspace, table, column)
    }
    
    /// 인덱스를 사용한 파티션 키 조회
    pub fn lookup_by_index(
        &self,
        keyspace: &str,
        table: &str,
        column: &str,
        value: &CassandraValue,
    ) -> Option<Vec<PartitionKey>> {
        self.index_manager.lookup(keyspace, table, column, value)
    }
    
    /// 모든 인덱스 목록 조회
    pub fn list_indexes(&self) -> Vec<IndexDefinition> {
        self.index_manager.list_all_indexes()
    }
    
    /// 키스페이스 생성
    pub async fn create_keyspace(&self, name: String, replication_factor: u32) -> Result<()> {
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
    pub async fn create_table(&self, keyspace: String, table: String, schema: TableSchema) -> Result<()> {
        schema.validate()?;
        
        let memtable = Arc::new(Memtable::new(Arc::new(schema.clone())));
        let table_struct = Table {
            schema: Arc::new(schema.clone()),
            memtables: Vec::new(),
            sstables: Vec::new(),
            current_memtable: memtable,
        };
        
        let keyspaces = self.keyspaces.read().await;
        if let Some(ks) = keyspaces.get(&keyspace) {
            let mut tables = ks.tables.write().await;
            tables.insert(table.clone(), table_struct);
        } else {
            return Err(CoreDBError::KeyspaceNotFound { keyspace: keyspace.clone() });
        }
        
        // 스키마를 파일로 저장
        self.save_table_schema(&keyspace, &table, &schema).await?;
        
        Ok(())
    }
    
    /// 테이블 스키마 저장
    async fn save_table_schema(&self, keyspace: &str, table: &str, schema: &TableSchema) -> Result<()> {
        let schema_dir = self.config.data_directory.join(keyspace).join(table);
        tokio::fs::create_dir_all(&schema_dir).await?;
        
        let schema_path = schema_dir.join("schema.json");
        let schema_json = serde_json::to_string_pretty(schema)?;
        tokio::fs::write(&schema_path, schema_json).await?;
        
        Ok(())
    }
    
    /// 테이블 스키마 로드
    async fn load_table_schema(&self, keyspace: &str, table: &str) -> Result<Option<TableSchema>> {
        let schema_path = self.config.data_directory
            .join(keyspace)
            .join(table)
            .join("schema.json");
        
        if !schema_path.exists() {
            return Ok(None);
        }
        
        let schema_json = tokio::fs::read_to_string(&schema_path).await?;
        let schema: TableSchema = serde_json::from_str(&schema_json)?;
        
        Ok(Some(schema))
    }
    
    /// 행 삽입
    pub async fn insert_row(&self, keyspace: &str, table: &str, row: crate::schema::Row) -> Result<()> {
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
    pub async fn get_row(&self, keyspace: &str, table: &str, partition_key: &crate::schema::PartitionKey, clustering_key: &Option<crate::schema::ClusteringKey>) -> Result<Option<crate::schema::Row>> {
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
                    if let Some(partition) = sstable.read_partition(partition_key).await? {
                        // 클러스터링 키가 있다면 해당 행만 반환
                        if let Some(ref ck) = clustering_key {
                            // 파티션 내에서 클러스터링 키로 검색
                            if let Some(row_entry) = partition.rows.get(&Some(ck.clone())) {
                                return Ok(Some(row_entry.value().clone()));
                            }
                        } else {
                            // 클러스터링 키가 없으면 첫 번째 행 반환
                            if let Some(row_entry) = partition.rows.iter().next() {
                                return Ok(Some(row_entry.value().clone()));
                            }
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// 메모리 테이블 플러시 체크
    async fn check_memtable_flush(&self) -> Result<()> {
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
    async fn flush_memtable(&self, keyspace: &str, table: &str) -> Result<()> {
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
    
    /// 모든 메모리 테이블 강제 플러시 (종료 전 호출)
    pub async fn flush_all(&self) -> Result<()> {
        // 먼저 flush할 테이블 목록 수집
        let mut to_flush: Vec<(String, String)> = Vec::new();
        
        {
            let keyspaces = self.keyspaces.read().await;
            for (keyspace_name, keyspace) in keyspaces.iter() {
                let tables = keyspace.tables.read().await;
                for (table_name, table) in tables.iter() {
                    if table.current_memtable.size_bytes() > 0 {
                        to_flush.push((keyspace_name.clone(), table_name.clone()));
                    }
                }
            }
        }
        
        // 락 해제 후 flush 실행
        for (keyspace, table) in to_flush {
            self.flush_memtable(&keyspace, &table).await?;
        }
        
        Ok(())
    }
    
    /// 시스템 키스페이스 생성
    async fn create_system_keyspaces(&mut self) -> Result<()> {
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
            CqlStatement::DropKeyspace { .. } |
            CqlStatement::CreateIndex { .. } |
            CqlStatement::DropIndex { .. }
        )
    }
    
    /// 커밋 로그에 뮤테이션 기록
    async fn log_mutation(&self, statement: &CqlStatement) -> Result<()> {
        // 현재는 단순화된 버전으로 로깅만 수행
        // 실제 구현에서는 INSERT/UPDATE/DELETE를 Mutation으로 변환해야 함
        match statement {
            CqlStatement::Insert { .. } |
            CqlStatement::Update { .. } |
            CqlStatement::Delete { .. } => {
                // WAL 기록 (현재는 건너뜀 - 단순화된 버전)
                // 실제로는 statement를 Mutation으로 변환하여 CommitLog에 기록
            },
            _ => {}
        }
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
    
    /// 데이터베이스를 디스크에 저장
    pub async fn save_to_disk(&self) -> Result<()> {
        use crate::persistence::Snapshot;
        
        let snapshot = Snapshot::new(self.config.data_directory.to_string_lossy().to_string());
        
        // 현재 데이터베이스 상태를 텍스트로 변환
        let keyspaces = self.keyspaces.read().await;
        let mut snapshot_data = String::new();
        
        for (ks_name, keyspace) in keyspaces.iter() {
            snapshot_data.push_str(&format!("KEYSPACE:{}\n", ks_name));
            
            let tables = keyspace.tables.read().await;
            for (table_name, _table) in tables.iter() {
                snapshot_data.push_str(&format!("TABLE:{}\n", table_name));
            }
        }
        
        snapshot.save_text(&snapshot_data)?;
        Ok(())
    }
    
    /// 데이터베이스 종료
    pub async fn shutdown(&self) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, CassandraDataType, TableSchema};
    
    
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
        
        // Use unique keyspace name to avoid conflicts with parallel tests
        let ks_name = format!("test_ks_{}", std::process::id());
        
        let result = db.execute_cql(&format!("CREATE KEYSPACE {} WITH REPLICATION = {{'class': 'SimpleStrategy', 'replication_factor': 1}}", ks_name)).await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql(&format!("CREATE TABLE {}.test_table (id INT PRIMARY KEY, name TEXT)", ks_name)).await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql(&format!("INSERT INTO {}.test_table (id, name) VALUES (1, 'John')", ks_name)).await.unwrap();
        assert!(result.is_success());
        
        let result = db.execute_cql(&format!("SELECT * FROM {}.test_table WHERE id = 1", ks_name)).await.unwrap();
        assert!(result.is_success());
    }
}
