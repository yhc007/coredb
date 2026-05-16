use crate::schema::{TableSchema, PartitionKey, ClusteringKey, CassandraValue, Row as SchemaRow, Cell};
use crate::storage::{Memtable, SSTable};
use crate::query::{CqlStatement, QueryResult, Row as QueryRow};
use crate::wal::{CommitLog, CommitLogEntry, Mutation};
use crate::error::*;
use std::sync::Arc;
use std::collections::{HashMap, BTreeMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::database::Keyspace;
use tokio::sync::RwLock;

/// TTL 만료 여부 체크
fn is_cell_expired(cell: &Cell) -> bool {
    if let Some(ttl) = cell.ttl {
        if ttl == 0 {
            return false; // TTL 0은 영구 저장
        }
        
        let now_micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as i64;
        
        let expiry_time = cell.timestamp + (ttl as i64 * 1_000_000); // ttl is in seconds, timestamp is in microseconds
        
        now_micros > expiry_time
    } else {
        false // No TTL means never expires
    }
}

/// 쿼리 엔진
pub struct QueryEngine {
    keyspaces: Arc<RwLock<HashMap<String, Keyspace>>>,
    /// Optional WAL handle. When set, INSERT / UPDATE / DELETE
    /// mutations get appended to the commit log before the in-memory
    /// memtable mutation, so a crash between memtable write and SSTable
    /// flush can be replayed on next startup. Wrapped in Option for the
    /// historical zero-arg `QueryEngine::new` constructor; new callers
    /// (CoreDB::new) should use `with_commit_log` to wire WAL durability.
    commit_log: Option<Arc<RwLock<CommitLog>>>,
    current_keyspace: Option<String>,
}

impl QueryEngine {
    pub fn new(keyspaces: Arc<RwLock<HashMap<String, Keyspace>>>) -> Self {
        Self {
            keyspaces,
            commit_log: None,
            current_keyspace: None,
        }
    }

    /// Variant of `new` that attaches a WAL handle so engine-level
    /// mutations land in the commit log too. Without this the CQL
    /// path (every INSERT going through the native protocol) bypasses
    /// the WAL entirely and any unflushed memtable data is lost on
    /// crash.
    pub fn with_commit_log(
        keyspaces: Arc<RwLock<HashMap<String, Keyspace>>>,
        commit_log: Arc<RwLock<CommitLog>>,
    ) -> Self {
        Self {
            keyspaces,
            commit_log: Some(commit_log),
            current_keyspace: None,
        }
    }

    /// Best-effort WAL append. Failures log + continue: a WAL write
    /// problem at runtime should not bring down the engine (the
    /// memtable mutation that follows is still in memory and the
    /// next successful flush will persist it via the SSTable path).
    async fn wal_append(&self, keyspace: &str, table: &str, mutation: Mutation) {
        let Some(wal) = self.commit_log.as_ref() else {
            return;
        };
        let entry = CommitLogEntry {
            keyspace: keyspace.to_string(),
            table: table.to_string(),
            mutation,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_micros() as i64)
                .unwrap_or(0),
        };
        if let Err(e) = wal.write().await.append(entry).await {
            eprintln!("[wal] append failed: {e}");
        }
    }
    
    /// CQL 문 실행
    pub async fn execute(&mut self, statement: CqlStatement) -> Result<QueryResult> {
        match statement {
            CqlStatement::CreateKeyspace { name, options } => {
                self.create_keyspace(name, options).await
            },
            CqlStatement::CreateTable { keyspace, name, columns, partition_key, clustering_key, options } => {
                self.create_table(keyspace, name, columns, partition_key, clustering_key, options).await
            },
            CqlStatement::Insert { keyspace, table, values, ttl, if_not_exists } => {
                self.insert_row(keyspace, table, values, ttl, if_not_exists).await
            },
            CqlStatement::Select { keyspace, table, columns, where_clause, limit, aggregations, distinct, allow_filtering, order_by, group_by, page_size, paging_state } => {
                self.select_rows(keyspace, table, columns, where_clause, limit, aggregations, distinct, allow_filtering, order_by, group_by, page_size, paging_state).await
            },
            CqlStatement::Update { keyspace, table, values, counter_updates, where_clause, if_conditions } => {
                self.update_row(keyspace, table, values, counter_updates, where_clause, if_conditions).await
            },
            CqlStatement::Delete { keyspace, table, where_clause } => {
                self.delete_row(keyspace, table, where_clause).await
            },
            CqlStatement::Batch { statements } => {
                self.execute_batch(statements).await
            },
            CqlStatement::DropTable { keyspace, name } => {
                self.drop_table(keyspace, name).await
            },
            CqlStatement::DropKeyspace { name } => {
                self.drop_keyspace(name).await
            },
            CqlStatement::Use { keyspace } => {
                self.use_keyspace(keyspace).await
            },
            // CreateIndex and DropIndex are handled at the database level
            CqlStatement::CreateIndex { .. } | CqlStatement::DropIndex { .. } => {
                Ok(QueryResult::Success)
            },
            CqlStatement::Truncate { keyspace, table } => {
                self.truncate_table(keyspace, table).await
            },
            CqlStatement::AlterTable { keyspace, table, operation } => {
                self.alter_table(keyspace, table, operation).await
            },
            CqlStatement::CreateType { keyspace, name, fields } => {
                self.create_type(keyspace, name, fields).await
            },
            CqlStatement::DropType { keyspace, name } => {
                self.drop_type(keyspace, name).await
            },
            CqlStatement::CreateMaterializedView { keyspace, name, base_table, columns, partition_key, clustering_key, where_clause } => {
                self.create_materialized_view(keyspace, name, base_table, columns, partition_key, clustering_key, where_clause).await
            },
            CqlStatement::DropMaterializedView { keyspace, name } => {
                self.drop_materialized_view(keyspace, name).await
            },
            // Authentication & Authorization - handled at database level
            CqlStatement::CreateUser { .. } | CqlStatement::AlterUser { .. } | CqlStatement::DropUser { .. } |
            CqlStatement::ListUsers | CqlStatement::CreateRole { .. } | CqlStatement::DropRole { .. } |
            CqlStatement::Grant { .. } | CqlStatement::Revoke { .. } | CqlStatement::ListRoles { .. } |
            CqlStatement::ListPermissions { .. } => {
                Ok(QueryResult::Success) // Handled in database.rs
            },
            // DESCRIBE - handled at database level
            CqlStatement::DescribeKeyspaces | CqlStatement::DescribeKeyspace { .. } |
            CqlStatement::DescribeTables { .. } | CqlStatement::DescribeTable { .. } => {
                Ok(QueryResult::Success) // Handled in database.rs
            },
        }
    }
    
    /// CREATE TYPE - UDT 생성
    async fn create_type(&mut self, keyspace: String, name: String, fields: Vec<(String, crate::schema::CassandraDataType)>) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        let mut user_types = ks.user_types.write().await;
        
        if user_types.contains_key(&name) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Type '{}' already exists", name),
            });
        }
        
        let udt = crate::schema::UserDefinedType {
            keyspace: keyspace.clone(),
            name: name.clone(),
            fields: fields.into_iter().map(|(n, t)| crate::schema::UDTField {
                name: n,
                data_type: t,
            }).collect(),
        };
        
        user_types.insert(name, udt);
        
        Ok(QueryResult::Success)
    }
    
    /// DROP TYPE - UDT 삭제
    async fn drop_type(&mut self, keyspace: String, name: String) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        let mut user_types = ks.user_types.write().await;
        
        if !user_types.contains_key(&name) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Type '{}' does not exist", name),
            });
        }
        
        user_types.remove(&name);
        
        Ok(QueryResult::Success)
    }
    
    /// CREATE MATERIALIZED VIEW
    async fn create_materialized_view(
        &mut self, 
        keyspace: String, 
        name: String, 
        base_table: String,
        columns: Vec<String>,
        partition_key: Vec<String>,
        clustering_key: Vec<String>,
        where_clause: Option<String>,
    ) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        // 베이스 테이블 존재 확인
        {
            let tables = ks.tables.read().await;
            if !tables.contains_key(&base_table) {
                return Err(CoreDBError::TableNotFound {
                    table: base_table.clone(),
                });
            }
        }
        
        let mut mvs = ks.materialized_views.write().await;
        
        if mvs.contains_key(&name) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Materialized view '{}' already exists", name),
            });
        }
        
        let mv = crate::schema::MaterializedView {
            name: name.clone(),
            keyspace: keyspace.clone(),
            base_table,
            partition_key,
            clustering_key,
            columns,
            where_clause,
        };
        
        mvs.insert(name, mv);
        
        Ok(QueryResult::Success)
    }
    
    /// DROP MATERIALIZED VIEW
    async fn drop_materialized_view(&mut self, keyspace: String, name: String) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        let mut mvs = ks.materialized_views.write().await;
        
        if !mvs.contains_key(&name) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Materialized view '{}' does not exist", name),
            });
        }
        
        mvs.remove(&name);
        
        Ok(QueryResult::Success)
    }
    
    /// ALTER TABLE - 테이블 스키마 변경
    async fn alter_table(&mut self, keyspace: String, table: String, operation: crate::query::parser::AlterTableOperation) -> Result<QueryResult> {
        use crate::query::parser::AlterTableOperation;
        
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        let mut tables = ks.tables.write().await;
        
        let tbl = tables.get_mut(&table).ok_or_else(|| CoreDBError::TableNotFound {
            table: table.clone(),
        })?;
        
        // 스키마 수정 (Arc를 새로 만들어서 교체)
        let mut new_schema = (*tbl.schema).clone();
        
        // 모든 컬럼 이름 수집 (중복 체크용)
        let all_column_names: Vec<String> = new_schema.partition_key.iter()
            .chain(new_schema.clustering_key.iter())
            .chain(new_schema.regular_columns.iter())
            .chain(new_schema.static_columns.iter())
            .map(|c| c.name.clone())
            .collect();
        
        match operation {
            AlterTableOperation::AddColumn { name, data_type } => {
                // 이미 존재하는지 확인
                if all_column_names.contains(&name) {
                    return Err(CoreDBError::QueryExecutionError {
                        message: format!("Column '{}' already exists", name),
                    });
                }
                // regular_columns에 추가
                new_schema.regular_columns.push(crate::schema::ColumnDefinition {
                    name,
                    data_type,
                    is_static: false,
                });
            },
            AlterTableOperation::DropColumn { name } => {
                // 기본 키는 삭제 불가
                if new_schema.partition_key.iter().any(|c| c.name == name) ||
                   new_schema.clustering_key.iter().any(|c| c.name == name) {
                    return Err(CoreDBError::QueryExecutionError {
                        message: format!("Cannot drop primary key column '{}'", name),
                    });
                }
                // regular_columns에서 삭제
                new_schema.regular_columns.retain(|c| c.name != name);
                new_schema.static_columns.retain(|c| c.name != name);
            },
            AlterTableOperation::RenameColumn { old_name, new_name } => {
                // 컬럼 이름 변경 (모든 컬럼 타입에서)
                for col in &mut new_schema.partition_key {
                    if col.name == old_name {
                        col.name = new_name.clone();
                    }
                }
                for col in &mut new_schema.clustering_key {
                    if col.name == old_name {
                        col.name = new_name.clone();
                    }
                }
                for col in &mut new_schema.regular_columns {
                    if col.name == old_name {
                        col.name = new_name.clone();
                    }
                }
                for col in &mut new_schema.static_columns {
                    if col.name == old_name {
                        col.name = new_name.clone();
                    }
                }
            },
        }
        
        tbl.schema = std::sync::Arc::new(new_schema);
        
        Ok(QueryResult::Success)
    }
    
    /// TRUNCATE - 테이블의 모든 데이터 삭제
    async fn truncate_table(&mut self, keyspace: String, table: String) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        
        let ks = keyspaces.get(&keyspace).ok_or_else(|| CoreDBError::KeyspaceNotFound {
            keyspace: keyspace.clone(),
        })?;
        
        let mut tables = ks.tables.write().await;
        
        let tbl = tables.get_mut(&table).ok_or_else(|| CoreDBError::TableNotFound {
            table: table.clone(),
        })?;
        
        // 새 빈 memtable로 교체
        let new_memtable = Arc::new(crate::storage::Memtable::new(tbl.schema.clone()));
        tbl.current_memtable = new_memtable;
        tbl.memtables.clear();
        
        // SSTable들 삭제
        for sstable in &tbl.sstables {
            let _ = sstable.delete().await;
        }
        tbl.sstables.clear();
        
        Ok(QueryResult::Success)
    }
    
    async fn create_keyspace(&mut self, name: String, options: crate::query::parser::KeyspaceOptions) -> Result<QueryResult> {
        let mut keyspaces = self.keyspaces.write().await;
        
        if keyspaces.contains_key(&name) {
             return Err(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' already exists", name),
            });
        }
        
        let keyspace = Keyspace {
            name: name.clone(),
            definition: crate::schema::KeyspaceDefinition {
                name: name.clone(),
                replication_factor: options.replication_factor,
                strategy: crate::schema::ReplicationStrategy::SimpleStrategy,
            },
            tables: Arc::new(RwLock::new(HashMap::new())),
            user_types: Arc::new(RwLock::new(HashMap::new())),
            materialized_views: Arc::new(RwLock::new(HashMap::new())),
        };
        
        keyspaces.insert(name, keyspace);
        Ok(QueryResult::Success)
    }
    
    async fn create_table(&mut self, keyspace: String, name: String, columns: Vec<crate::schema::ColumnDefinition>, partition_key: Vec<String>, clustering_key: Vec<String>, _options: crate::query::parser::TableOptions) -> Result<QueryResult> {
        // 테이블 스키마 생성
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
        
        let schema = Arc::new(TableSchema::new(
            name.clone(),
            keyspace.clone(),
            pk_columns,
            ck_columns,
            regular_columns,
            static_columns,
        ));
        
        // 스키마 검증
        schema.validate()?;
        
        // 메모리 테이블 생성
        let memtable = Arc::new(Memtable::new(schema.clone()));
        
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace),
        })?;
        
        let mut tables = keyspace_struct.tables.write().await;
        if tables.contains_key(&name) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Table '{}.{}' already exists", keyspace, name),
            });
        }
        
        let table_struct = crate::database::Table {
            schema: schema,
            memtables: Vec::new(),
            sstables: Vec::new(),
            current_memtable: memtable,
        };
        
        tables.insert(name, table_struct);
        
        Ok(QueryResult::Success)
    }
    
    async fn insert_row(&mut self, keyspace: String, table: String, values: Vec<(String, CassandraValue)>, ttl: Option<u32>, if_not_exists: bool) -> Result<QueryResult> {
        let keyspace_name = if keyspace.is_empty() {
            self.current_keyspace.clone().ok_or(CoreDBError::QueryExecutionError {
                message: "No keyspace selected".to_string(),
            })?
        } else {
            keyspace
        };

        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace_name).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace_name),
        })?;

        let tables = keyspace_struct.tables.read().await;
        let table_struct = tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Table '{}' does not exist", table),
        })?;

        // Coerce each parsed literal to its column's schema-declared type
        // up front. parse_value() is type-agnostic — `42` becomes Int even
        // when the column is TIMESTAMP, `0.5` becomes Double even when
        // the column is BigInt, etc. Without this rewrite, the value
        // stored carries the parser's inference, and later SELECTs emit
        // RESULT/Rows metadata reflecting that inferred type instead of
        // the schema type, breaking strongly-typed clients.
        let mut values = values;
        for (col_name, val) in values.iter_mut() {
            if let Some(col_def) = table_struct.schema.get_column(col_name) {
                let owned = std::mem::replace(val, CassandraValue::Null);
                *val = owned.coerce_to(&col_def.data_type);
            } else {
                return Err(CoreDBError::QueryExecutionError {
                    message: format!("Column '{}' does not exist in table '{}'", col_name, table),
                });
            }
        }

        // 파티션 키와 클러스터링 키 추출
        let (partition_key, clustering_key) = self.extract_keys_from_values(values.clone(), &table_struct.schema)?;
        
        // IF NOT EXISTS 체크
        if if_not_exists {
            let existing = table_struct.current_memtable.get(&partition_key, &clustering_key);
            if existing.is_some() {
                // 이미 존재하면 [applied] = false 반환
                let mut result_row = HashMap::new();
                result_row.insert("[applied]".to_string(), CassandraValue::Boolean(false));
                return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
            }
        }
        
        // 행 생성 - cells에는 regular columns만 포함
        let mut cells = HashMap::new();
        
        // PK/CK 컬럼 이름 수집
        let mut key_columns = std::collections::HashSet::new();
        for col in &table_struct.schema.partition_key {
            key_columns.insert(col.name.clone());
        }
        for col in &table_struct.schema.clustering_key {
            key_columns.insert(col.name.clone());
        }
        
        let now = chrono::Utc::now().timestamp_micros();
        
        for (column_name, value) in values {
            // 컬럼 존재 여부 확인
            if table_struct.schema.get_column(&column_name).is_none() {
                return Err(CoreDBError::QueryExecutionError {
                    message: format!("Column '{}' does not exist in table '{}'", column_name, table),
                });
            }
            
            // PK/CK가 아닌 경우만 cells에 추가
            if !key_columns.contains(&column_name) {
                let cell = Cell {
                    value,
                    timestamp: now,
                    ttl,  // TTL 적용
                    is_deleted: false,
                };
                cells.insert(column_name, cell);
            }
        }
        
        let row = SchemaRow {
            partition_key,
            clustering_key,
            cells,
            timestamp: now,
        };
        
        // WAL append before memtable put: if we crash between these
        // two steps the WAL entry is durable and replay will re-apply
        // on startup. Drop the locks held by the keyspaces.read() /
        // tables.read() guards while we await the WAL write so the
        // write doesn't block other CQL traffic.
        drop(tables);
        drop(keyspaces);
        self.wal_append(&keyspace_name, &table, Mutation::Insert(row.clone()))
            .await;
        // Re-acquire to hand the row to the memtable. The keyspace +
        // table must still exist since DROP would require an
        // exclusive lock we can't get while INSERT is in flight.
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces
            .get(&keyspace_name)
            .ok_or(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' disappeared mid-insert", keyspace_name),
            })?;
        let tables = keyspace_struct.tables.read().await;
        let table_struct =
            tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
                message: format!("Table '{}' disappeared mid-insert", table),
            })?;
        table_struct.current_memtable.put(row)?;

        // IF NOT EXISTS 성공 시 [applied] = true 반환
        if if_not_exists {
            let mut result_row = HashMap::new();
            result_row.insert("[applied]".to_string(), CassandraValue::Boolean(true));
            return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
        }
        
        Ok(QueryResult::Success)
    }
    
    async fn select_rows(&mut self, keyspace: String, table: String, columns: Vec<String>, where_clause: Option<crate::query::parser::WhereClause>, limit: Option<u32>, aggregations: Vec<crate::query::parser::Aggregation>, distinct: bool, _allow_filtering: bool, order_by: Option<crate::query::parser::OrderBy>, group_by: Vec<String>, page_size: Option<u32>, paging_state: Option<String>) -> Result<QueryResult> {
        // Note: allow_filtering은 현재 무시됨 (단일 노드 DB에서는 항상 필터링 가능)
        // page_size와 paging_state는 Native Protocol 레벨에서 처리됨
        let keyspace_name = if keyspace.is_empty() {
            self.current_keyspace.clone().ok_or(CoreDBError::QueryExecutionError {
                message: "No keyspace selected".to_string(),
            })?
        } else {
            keyspace
        };
        
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace_name).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace_name),
        })?;
        
        let tables = keyspace_struct.tables.read().await;
        let table_struct = tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Table '{}' does not exist", table),
        })?;
        
        // 파티션 키 추출 (WHERE 절에서)
        let partition_key = if let Some(ref wc) = where_clause {
            self.extract_partition_key(&table_struct.schema, wc)?
        } else {
            None
        };

        // Fast path: `SELECT COUNT(*) FROM ks.t` with no WHERE / no
        // GROUP BY / no DISTINCT. Sums the memtable row count + each
        // SSTable's persisted row_count (via the `-Stats.json`
        // sidecar). If *any* SSTable lacks a stats file (legacy data
        // on disk from before the sidecar landed), fall back to the
        // slow row-materialization path so the count stays correct.
        // New compactions / memtable flushes always write the
        // sidecar, so a busy table self-heals into the fast path
        // within a flush cycle.
        let count_star_eligible = where_clause.is_none()
            && partition_key.is_none()
            && group_by.is_empty()
            && !distinct
            && aggregations.len() == 1
            && matches!(aggregations[0].func, crate::query::parser::AggregationFunc::Count)
            && aggregations[0].column == "*";
        if count_star_eligible {
            let all_have_stats = table_struct
                .sstables
                .iter()
                .all(|s| s.row_count.is_some());

            // Overlap guard: if any partition key appears in more
            // than one of (current memtable ∪ immutable memtables ∪
            // SSTables), the slow path would dedup by (pk, ck) but
            // the fast path's naive sum would over-count the
            // overlap. Bail in that case so overwrite-heavy tables
            // (e.g. `decisions` whose bucket_day repeats across
            // flushes) stay correct. Append-only tables with
            // unique partition keys per flush window (e.g.
            // `btc_ticks` bucketed by hour) still hit the fast path.
            //
            // The check is O(total partitions across the table) —
            // a HashSet lookup per partition key. Cheap relative to
            // even one round-trip CoreDB read; we only run it on
            // the COUNT(*) request which is itself rare.
            let no_overlap = if all_have_stats {
                use std::collections::HashSet;
                let mut seen: HashSet<crate::schema::PartitionKey> = HashSet::new();
                let mut clean = true;
                'check: {
                    let parts = table_struct.current_memtable.get_all_partitions();
                    for (pk, _) in &parts {
                        if !seen.insert(pk.clone()) {
                            clean = false;
                            break 'check;
                        }
                    }
                    for mt in &table_struct.memtables {
                        let parts = mt.get_all_partitions();
                        for (pk, _) in &parts {
                            if !seen.insert(pk.clone()) {
                                clean = false;
                                break 'check;
                            }
                        }
                    }
                    for s in &table_struct.sstables {
                        for pk in s.partition_index.keys() {
                            if !seen.insert(pk.clone()) {
                                clean = false;
                                break 'check;
                            }
                        }
                    }
                }
                clean
            } else {
                false
            };

            if all_have_stats && no_overlap {
                let mut total: u64 = table_struct.current_memtable.row_count();
                for mt in &table_struct.memtables {
                    total += mt.row_count();
                }
                for s in &table_struct.sstables {
                    total += s.row_count.unwrap_or(0);
                }
                let mut cells = HashMap::new();
                let agg_name = format!("count({})", aggregations[0].column);
                cells.insert(agg_name, crate::schema::CassandraValue::BigInt(total as i64));
                return Ok(QueryResult::Rows(vec![QueryRow { columns: cells }]));
            }
        }

        // Fast path: `SELECT * FROM ks.t LIMIT 1` with no WHERE / no
        // aggregation / no GROUP BY / no DISTINCT / no ORDER BY.
        // Used heavily by `verify_tables` (post-migrate smoke check)
        // and any operator running `cqlsh> SELECT * FROM t LIMIT 1`
        // against a big partition. The classic full-scan path reads
        // every partition into memory before truncating to N rows;
        // on the daemon's 206k-row btc_ticks table that wedges at
        // the default 30s query timeout.
        //
        // We just need a single row to satisfy the LIMIT. Take the
        // first one we can find across memtable → immutable
        // memtables → SSTables (in that order — newest data first,
        // so the row we return is the most recently visible one).
        let fast_path_eligible = where_clause.is_none()
            && partition_key.is_none()
            && aggregations.is_empty()
            && group_by.is_empty()
            && order_by.is_none()
            && !distinct
            && limit == Some(1);

        let mut result_rows: Vec<SchemaRow> = Vec::new();

        if fast_path_eligible {
            'fast: {
                // 1. Current memtable.
                let parts = table_struct.current_memtable.get_all_partitions();
                for (_, partition) in parts {
                    if let Some(entry) = partition.rows.iter().next() {
                        result_rows.push(entry.value().clone());
                        break 'fast;
                    }
                }
                // 2. Immutable memtables.
                for mt in &table_struct.memtables {
                    let parts = mt.get_all_partitions();
                    for (_, partition) in parts {
                        if let Some(entry) = partition.rows.iter().next() {
                            result_rows.push(entry.value().clone());
                            break 'fast;
                        }
                    }
                }
                // 3. SSTables — read the very first partition only.
                for sstable in &table_struct.sstables {
                    if let Some(first_pk) = sstable.partition_index.keys().next() {
                        if let Ok(Some(partition)) = sstable.read_partition(first_pk).await {
                            if let Some(entry) = partition.rows.iter().next() {
                                result_rows.push(entry.value().clone());
                                break 'fast;
                            }
                        }
                    }
                }
                // Table genuinely empty — leave result_rows empty
                // and fall through to the regular post-processing
                // tail which handles that case cleanly.
            }
        }
        
        // Skip the regular scan path if the fast path already
        // populated `result_rows` with a single row (or definitively
        // proved the table empty).
        if !fast_path_eligible {
            if let Some(pk) = partition_key {
                // 1. Memtable 검색
                let rows = table_struct.current_memtable.range_scan(&pk, &None, &None);
                for row in rows {
                    result_rows.push(row);
                }

                // 2. Immutable Memtables 검색
                for memtable in &table_struct.memtables {
                    let rows = memtable.range_scan(&pk, &None, &None);
                    for row in rows {
                        result_rows.push(row);
                    }
                }

                // 3. SSTables 검색 — skip SSTables whose persisted
                // [min, max] partition-key range doesn't cover `pk`.
                // The veto is O(1) on the in-memory bounds; without
                // it every SSTable would pay an async dispatch +
                // partition_index BTreeMap lookup even for hopeless
                // candidates. Big win once a table accumulates many
                // bucket-keyed SSTables (decisions, btc_ticks, etc.).
                for sstable in &table_struct.sstables {
                    if sstable.excludes_partition_key(&pk) {
                        continue;
                    }
                    if let Some(partition) = sstable.read_partition(&pk).await? {
                        for entry in partition.rows.iter() {
                            result_rows.push(entry.value().clone());
                        }
                    }
                }
            } else {
                // 전체 스캔.
                //
                // Early-termination guard: when the caller has set
                // a LIMIT and the rest of the query doesn't need a
                // full materialization (no WHERE filter, no
                // ORDER BY, no aggregation / GROUP BY / DISTINCT),
                // we can stop reading sources once we've gathered
                // enough rows to satisfy the LIMIT. With a 2× safety
                // factor for any dedup that the post-scan dedup_map
                // will perform — the floor-of-2 keeps `LIMIT 1`
                // honest while still trimming for large N.
                //
                // SSTable iteration is the part that actually
                // benefits — memtable scans are in-memory anyway,
                // but stopping there too keeps the contract uniform:
                // once we have enough, stop. For tables with cross-
                // source duplicates this may still return fewer than
                // LIMIT rows (the dedup reduces the count); operators
                // who want exactly N back can bump LIMIT to 2N.
                let can_short_circuit = limit.is_some()
                    && where_clause.is_none()
                    && order_by.is_none()
                    && aggregations.is_empty()
                    && group_by.is_empty()
                    && !distinct;
                // Use the safety factor only when limit can actually
                // be hit; otherwise the scan is unbounded.
                let scan_cap: usize = if can_short_circuit {
                    let l = limit.unwrap() as usize;
                    l.saturating_mul(2).max(2)
                } else {
                    usize::MAX
                };

                // 1. Current Memtable
                let partitions = table_struct.current_memtable.get_all_partitions();
                'mem_current: for (_, partition) in partitions {
                    for entry in partition.rows.iter() {
                        result_rows.push(entry.value().clone());
                        if result_rows.len() >= scan_cap {
                            break 'mem_current;
                        }
                    }
                }

                // 2. Immutable Memtables
                if result_rows.len() < scan_cap {
                    'mem_immut: for memtable in &table_struct.memtables {
                        let partitions = memtable.get_all_partitions();
                        for (_, partition) in partitions {
                            for entry in partition.rows.iter() {
                                result_rows.push(entry.value().clone());
                                if result_rows.len() >= scan_cap {
                                    break 'mem_immut;
                                }
                            }
                        }
                    }
                }

                // 3. SSTables - full scan. This is the hot loop the
                // optimization is built for: a table with thousands
                // of bucket-keyed SSTables would otherwise pay
                // O(rows) for a LIMIT 20 query.
                //
                // Iterate SSTables in ascending `min_partition_key`
                // order so a `LIMIT N` over a full scan returns the
                // smallest-key partitions first, deterministically.
                // Without this the order is dictated by the Vec's
                // insertion order — which depends on flush timing,
                // compaction, and restart-load order. Two runs of
                // the same `SELECT * FROM t LIMIT N` could surface
                // different rows; sorting the iteration view (not
                // the underlying Vec) gives operators a stable
                // result for ad-hoc queries.
                //
                // SSTables with unknown bounds (legacy v1 sidecar)
                // sort last — they're the only ones that need to
                // open data to know their key extent, and we'd
                // rather satisfy LIMIT from bounds-known SSTables
                // first when we can.
                if result_rows.len() < scan_cap {
                    let mut sstable_order: Vec<&Arc<SSTable>> = table_struct.sstables.iter().collect();
                    sstable_order.sort_by(|a, b| {
                        match (&a.min_partition_key, &b.min_partition_key) {
                            (Some(ak), Some(bk)) => ak.cmp(bk),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => std::cmp::Ordering::Equal,
                        }
                    });
                    'sstable_scan: for sstable in &sstable_order {
                        for pk in sstable.partition_index.keys() {
                            if let Ok(Some(partition)) = sstable.read_partition(pk).await {
                                for entry in partition.rows.iter() {
                                    result_rows.push(entry.value().clone());
                                    if result_rows.len() >= scan_cap {
                                        break 'sstable_scan;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // 중복 제거 (partition_key + clustering_key 기준)
        // 최신 timestamp 우선 - BTreeMap 사용 (PartitionKey는 Ord 구현됨)
        let mut dedup_map: BTreeMap<(PartitionKey, Option<ClusteringKey>), SchemaRow> = BTreeMap::new();
        for row in result_rows {
            let key = (row.partition_key.clone(), row.clustering_key.clone());
            match dedup_map.get(&key) {
                Some(existing) if existing.timestamp >= row.timestamp => {
                    // 기존 row가 더 최신이면 스킵
                }
                _ => {
                    dedup_map.insert(key, row);
                }
            }
        }
        let result_rows: Vec<SchemaRow> = dedup_map.into_values().collect();

        // WHERE 조건 필터링 (non-PK 조건들)
        let result_rows: Vec<SchemaRow> = if let Some(ref wc) = where_clause {
            // Pre-coerce each condition's literal to the column's schema
            // type so comparisons line up. The parser produces literal-
            // inferred types (`1778716800000` → BigInt) and insert_row
            // now stores values shaped by the schema type
            // (bucket_day → Timestamp). Without this step the filter
            // would compare Timestamp(X) against BigInt(X) and silently
            // drop every otherwise-matching row.
            let coerced_conditions: Vec<crate::query::parser::Condition> = wc
                .conditions
                .iter()
                .map(|c| {
                    let target = table_struct
                        .schema
                        .get_column(&c.column)
                        .map(|cd| cd.data_type.clone());
                    let value = match target {
                        Some(t) => c.value.clone().coerce_to(&t),
                        None => c.value.clone(),
                    };
                    crate::query::parser::Condition {
                        column: c.column.clone(),
                        operator: c.operator.clone(),
                        value,
                        is_token: c.is_token,
                    }
                })
                .collect();
            result_rows.into_iter().filter(|row| {
                for condition in &coerced_conditions {
                    // TOKEN 함수 처리
                    if condition.is_token {
                        let token_value = Self::compute_token(&row.partition_key);
                        let cond_token = match &condition.value {
                            CassandraValue::BigInt(v) => *v,
                            CassandraValue::Int(v) => *v as i64,
                            _ => 0,
                        };
                        let token_cond_value = CassandraValue::BigInt(token_value);
                        let cond_value = CassandraValue::BigInt(cond_token);
                        let temp_condition = crate::query::parser::Condition {
                            column: condition.column.clone(),
                            operator: condition.operator.clone(),
                            value: cond_value,
                            is_token: false,
                        };
                        if !Self::matches_condition(&token_cond_value, &temp_condition) {
                            return false;
                        }
                        continue;
                    }
                    
                    // 해당 컬럼의 값 찾기
                    let value = {
                        // 1. PK에서 찾기
                        let mut found: Option<&CassandraValue> = None;
                        for (i, col_def) in table_struct.schema.partition_key.iter().enumerate() {
                            if col_def.name == condition.column {
                                found = row.partition_key.components.get(i);
                                break;
                            }
                        }
                        // 2. CK에서 찾기
                        if found.is_none() {
                            if let Some(ck) = &row.clustering_key {
                                for (i, col_def) in table_struct.schema.clustering_key.iter().enumerate() {
                                    if col_def.name == condition.column {
                                        found = ck.components.get(i);
                                        break;
                                    }
                                }
                            }
                        }
                        // 3. Regular columns에서 찾기
                        if found.is_none() {
                            if let Some(cell) = row.cells.get(&condition.column) {
                                found = Some(&cell.value);
                            }
                        }
                        found
                    };
                    
                    // 값이 없거나 조건 불일치면 false
                    match value {
                        Some(v) => {
                            if !Self::matches_condition(v, condition) {
                                return false;
                            }
                        },
                        None => return false,
                    }
                }
                true
            }).collect()
        } else {
            result_rows
        };
        
        // 컬럼 필터링 및 변환
        let mut query_rows = Vec::new();
        for row in result_rows {
            let mut cells = HashMap::new();
            
            // 모든 컬럼 값 수집 (PK, CK, Regular)
            // 1. Partition Key
            for (i, col_def) in table_struct.schema.partition_key.iter().enumerate() {
                if let Some(val) = row.partition_key.components.get(i) {
                    cells.insert(col_def.name.clone(), val.clone());
                }
            }
            
            // 2. Clustering Key
            if let Some(ck) = &row.clustering_key {
                for (i, col_def) in table_struct.schema.clustering_key.iter().enumerate() {
                    if let Some(val) = ck.components.get(i) {
                        cells.insert(col_def.name.clone(), val.clone());
                    }
                }
            }
            
            // 3. Regular Columns (Cells) - TTL 만료 체크
            for (col, cell) in &row.cells {
                // TTL 만료된 셀은 건너뛰기
                if is_cell_expired(cell) {
                    continue;
                }
                cells.insert(col.clone(), cell.value.clone());
            }
            
            // 요청된 컬럼만 필터링
            //
            // For an explicit projection, every requested column must
            // appear in the resulting row's cell map — including ones
            // that this particular row has no value for. Otherwise the
            // result-frame builder in protocol/handler.rs has no way
            // to surface the column at all when *every* row predates
            // an `ALTER TABLE ... ADD col`, and the scylla driver's
            // typed-row deserializer rejects the entire response with
            // "values for columns [...] are missing from the DB data
            // but are required by the Rust type". Synthesize a Null
            // cell for the missing column so the column slot stays
            // present and the wire-level shape is stable across
            // pre-/post-ALTER row mixes.
            let mut final_cells = HashMap::new();
            if columns.contains(&"*".to_string()) || !aggregations.is_empty() {
                // aggregation이 있으면 모든 컬럼 필요
                final_cells = cells;
            } else {
                for col in &columns {
                    let val = cells
                        .get(col)
                        .cloned()
                        .unwrap_or(crate::schema::CassandraValue::Null);
                    final_cells.insert(col.clone(), val);
                }
            }

            query_rows.push(QueryRow { columns: final_cells });
        }
        
        // DISTINCT 처리 - 중복 행 제거
        if distinct {
            let mut seen = std::collections::HashSet::new();
            query_rows.retain(|row| {
                // 행의 모든 컬럼 값을 문자열로 변환하여 해시
                let key: String = row.columns.iter()
                    .map(|(k, v)| format!("{}:{:?}", k, v))
                    .collect::<Vec<_>>()
                    .join("|");
                seen.insert(key)
            });
        }
        
        // ORDER BY 정렬
        if let Some(ref ob) = order_by {
            use crate::query::parser::SortOrder;
            query_rows.sort_by(|a, b| {
                let val_a = a.columns.get(&ob.column);
                let val_b = b.columns.get(&ob.column);
                
                let cmp = match (val_a, val_b) {
                    (Some(va), Some(vb)) => Self::compare_values(va, vb).unwrap_or(std::cmp::Ordering::Equal),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                
                if ob.order == SortOrder::Desc {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
        }
        
        // LIMIT 적용
        if let Some(l) = limit {
            query_rows.truncate(l as usize);
        }
        
        // GROUP BY + Aggregation 처리
        if !group_by.is_empty() && !aggregations.is_empty() {
            return self.compute_grouped_aggregations(&query_rows, &group_by, &aggregations);
        }
        
        // Aggregation 처리 (GROUP BY 없이)
        if !aggregations.is_empty() {
            return self.compute_aggregations(&query_rows, &aggregations);
        }
        
        Ok(QueryResult::Rows(query_rows))
    }
    
    /// Aggregation 계산
    fn compute_aggregations(&self, rows: &[QueryRow], aggregations: &[crate::query::parser::Aggregation]) -> Result<QueryResult> {
        use crate::query::parser::AggregationFunc;
        
        let mut result_cells = HashMap::new();
        
        for agg in aggregations {
            let agg_name = match agg.func {
                AggregationFunc::Count => format!("count({})", agg.column),
                AggregationFunc::Sum => format!("sum({})", agg.column),
                AggregationFunc::Avg => format!("avg({})", agg.column),
                AggregationFunc::Min => format!("min({})", agg.column),
                AggregationFunc::Max => format!("max({})", agg.column),
            };
            
            let value = match agg.func {
                AggregationFunc::Count => {
                    if agg.column == "*" {
                        CassandraValue::BigInt(rows.len() as i64)
                    } else {
                        let count = rows.iter()
                            .filter(|r| r.columns.contains_key(&agg.column))
                            .count();
                        CassandraValue::BigInt(count as i64)
                    }
                },
                AggregationFunc::Sum => {
                    let sum: i64 = rows.iter()
                        .filter_map(|r| r.columns.get(&agg.column))
                        .filter_map(|v| match v {
                            CassandraValue::Int(i) => Some(*i as i64),
                            CassandraValue::BigInt(i) => Some(*i),
                            CassandraValue::Double(d) => Some(*d as i64),
                            _ => None,
                        })
                        .sum();
                    CassandraValue::BigInt(sum)
                },
                AggregationFunc::Avg => {
                    let values: Vec<f64> = rows.iter()
                        .filter_map(|r| r.columns.get(&agg.column))
                        .filter_map(|v| match v {
                            CassandraValue::Int(i) => Some(*i as f64),
                            CassandraValue::BigInt(i) => Some(*i as f64),
                            CassandraValue::Double(d) => Some(*d),
                            _ => None,
                        })
                        .collect();
                    
                    if values.is_empty() {
                        CassandraValue::Null
                    } else {
                        let avg = values.iter().sum::<f64>() / values.len() as f64;
                        CassandraValue::Double(avg)
                    }
                },
                AggregationFunc::Min => {
                    let min = rows.iter()
                        .filter_map(|r| r.columns.get(&agg.column))
                        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .cloned();
                    min.unwrap_or(CassandraValue::Null)
                },
                AggregationFunc::Max => {
                    let max = rows.iter()
                        .filter_map(|r| r.columns.get(&agg.column))
                        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                        .cloned();
                    max.unwrap_or(CassandraValue::Null)
                },
            };
            
            result_cells.insert(agg_name, value);
        }
        
        Ok(QueryResult::Rows(vec![QueryRow { columns: result_cells }]))
    }
    
    /// GROUP BY + Aggregation 계산
    fn compute_grouped_aggregations(&self, rows: &[QueryRow], group_by: &[String], aggregations: &[crate::query::parser::Aggregation]) -> Result<QueryResult> {
        use crate::query::parser::AggregationFunc;
        
        // 그룹별로 행 분류
        let mut groups: HashMap<String, Vec<&QueryRow>> = HashMap::new();
        
        for row in rows {
            // 그룹 키 생성
            let group_key: String = group_by.iter()
                .map(|col| {
                    row.columns.get(col)
                        .map(|v| format!("{:?}", v))
                        .unwrap_or_else(|| "NULL".to_string())
                })
                .collect::<Vec<_>>()
                .join("|");
            
            groups.entry(group_key).or_default().push(row);
        }
        
        // 각 그룹에 대해 aggregation 수행
        let mut result_rows = Vec::new();
        
        for (group_key, group_rows) in groups {
            let mut result_cells = HashMap::new();
            
            // 그룹 키 컬럼 값 추가
            if let Some(first_row) = group_rows.first() {
                for col in group_by {
                    if let Some(val) = first_row.columns.get(col) {
                        result_cells.insert(col.clone(), val.clone());
                    }
                }
            }
            
            // Aggregation 계산
            for agg in aggregations {
                let agg_name = match agg.func {
                    AggregationFunc::Count => format!("count({})", agg.column),
                    AggregationFunc::Sum => format!("sum({})", agg.column),
                    AggregationFunc::Avg => format!("avg({})", agg.column),
                    AggregationFunc::Min => format!("min({})", agg.column),
                    AggregationFunc::Max => format!("max({})", agg.column),
                };
                
                let value = match agg.func {
                    AggregationFunc::Count => {
                        if agg.column == "*" {
                            CassandraValue::BigInt(group_rows.len() as i64)
                        } else {
                            let count = group_rows.iter()
                                .filter(|r| r.columns.contains_key(&agg.column))
                                .count();
                            CassandraValue::BigInt(count as i64)
                        }
                    },
                    AggregationFunc::Sum => {
                        let sum: i64 = group_rows.iter()
                            .filter_map(|r| r.columns.get(&agg.column))
                            .filter_map(|v| match v {
                                CassandraValue::Int(i) => Some(*i as i64),
                                CassandraValue::BigInt(i) => Some(*i),
                                CassandraValue::Double(d) => Some(*d as i64),
                                _ => None,
                            })
                            .sum();
                        CassandraValue::BigInt(sum)
                    },
                    AggregationFunc::Avg => {
                        let values: Vec<f64> = group_rows.iter()
                            .filter_map(|r| r.columns.get(&agg.column))
                            .filter_map(|v| match v {
                                CassandraValue::Int(i) => Some(*i as f64),
                                CassandraValue::BigInt(i) => Some(*i as f64),
                                CassandraValue::Double(d) => Some(*d),
                                _ => None,
                            })
                            .collect();
                        
                        if values.is_empty() {
                            CassandraValue::Null
                        } else {
                            let avg = values.iter().sum::<f64>() / values.len() as f64;
                            CassandraValue::Double(avg)
                        }
                    },
                    AggregationFunc::Min => {
                        let min = group_rows.iter()
                            .filter_map(|r| r.columns.get(&agg.column))
                            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                            .cloned();
                        min.unwrap_or(CassandraValue::Null)
                    },
                    AggregationFunc::Max => {
                        let max = group_rows.iter()
                            .filter_map(|r| r.columns.get(&agg.column))
                            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                            .cloned();
                        max.unwrap_or(CassandraValue::Null)
                    },
                };
                
                result_cells.insert(agg_name, value);
            }
            
            result_rows.push(QueryRow { columns: result_cells });
        }
        
        Ok(QueryResult::Rows(result_rows))
    }
    
    async fn update_row(&mut self, keyspace: String, table: String, values: Vec<(String, CassandraValue)>, counter_updates: Vec<crate::query::parser::CounterUpdate>, where_clause: crate::query::parser::WhereClause, if_conditions: Option<Vec<crate::query::parser::Condition>>) -> Result<QueryResult> {
        let keyspace_name = if keyspace.is_empty() {
            self.current_keyspace.clone().ok_or(CoreDBError::QueryExecutionError {
                message: "No keyspace selected".to_string(),
            })?
        } else {
            keyspace
        };
        
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace_name).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace_name),
        })?;
        
        let tables = keyspace_struct.tables.read().await;
        let table_struct = tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Table '{}' does not exist", table),
        })?;
        
        // WHERE 절에서 파티션 키 추출
        let partition_key = self.extract_partition_key(&table_struct.schema, &where_clause)?
            .ok_or(CoreDBError::QueryExecutionError {
                message: "UPDATE requires partition key in WHERE clause".to_string(),
            })?;
        
        // 기존 행 조회
        let existing_row = table_struct.current_memtable.get(&partition_key, &None);
        
        // IF 조건 체크
        if let Some(conditions) = &if_conditions {
            if let Some(ref row) = existing_row {
                for cond in conditions {
                    let cell_value = row.cells.get(&cond.column).map(|c| &c.value);
                    let matches = match cell_value {
                        Some(v) => Self::matches_condition(v, cond),
                        None => false,
                    };
                    
                    if !matches {
                        let mut result_row = HashMap::new();
                        result_row.insert("[applied]".to_string(), CassandraValue::Boolean(false));
                        return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
                    }
                }
            } else {
                // 행이 없으면 IF 조건 실패
                let mut result_row = HashMap::new();
                result_row.insert("[applied]".to_string(), CassandraValue::Boolean(false));
                return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
            }
        }
        
        // 기존 행이 없으면 새로 생성
        let mut cells = if let Some(row) = existing_row {
            row.cells.clone()
        } else {
            HashMap::new()
        };
        
        let now = chrono::Utc::now().timestamp_micros();
        
        // 새 값으로 업데이트
        for (col_name, value) in values {
            cells.insert(col_name, Cell {
                value,
                timestamp: now,
                ttl: None,
                is_deleted: false,
            });
        }
        
        // COUNTER 증감 연산
        for counter_update in counter_updates {
            let current_value = cells.get(&counter_update.column)
                .map(|c| match &c.value {
                    CassandraValue::Counter(v) => *v,
                    CassandraValue::BigInt(v) => *v,
                    CassandraValue::Int(v) => *v as i64,
                    _ => 0,
                })
                .unwrap_or(0);
            
            let new_value = current_value + counter_update.increment;
            cells.insert(counter_update.column, Cell {
                value: CassandraValue::Counter(new_value),
                timestamp: now,
                ttl: None,
                is_deleted: false,
            });
        }
        
        let new_row = SchemaRow {
            partition_key,
            clustering_key: None,
            cells,
            timestamp: now,
        };

        // WAL append before memtable put — same drop-and-reacquire
        // pattern as `insert_row` so the commit log captures every
        // CQL mutation, not just inserts.
        drop(tables);
        drop(keyspaces);
        self.wal_append(
            &keyspace_name,
            &table,
            Mutation::Insert(new_row.clone()),
        )
        .await;
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces
            .get(&keyspace_name)
            .ok_or(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' disappeared mid-update", keyspace_name),
            })?;
        let tables = keyspace_struct.tables.read().await;
        let table_struct =
            tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
                message: format!("Table '{}' disappeared mid-update", table),
            })?;
        table_struct.current_memtable.put(new_row)?;

        // IF 조건이 있었으면 [applied] = true 반환
        if if_conditions.is_some() {
            let mut result_row = HashMap::new();
            result_row.insert("[applied]".to_string(), CassandraValue::Boolean(true));
            return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
        }

        Ok(QueryResult::Success)
    }

    async fn delete_row(&mut self, keyspace: String, table: String, where_clause: crate::query::parser::WhereClause) -> Result<QueryResult> {
        let keyspace_name = if keyspace.is_empty() {
            self.current_keyspace.clone().ok_or(CoreDBError::QueryExecutionError {
                message: "No keyspace selected".to_string(),
            })?
        } else {
            keyspace
        };
        
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace_name).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace_name),
        })?;
        
        let tables = keyspace_struct.tables.read().await;
        let table_struct = tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Table '{}' does not exist", table),
        })?;
        
        // WHERE 절에서 파티션 키 추출
        let partition_key = self.extract_partition_key(&table_struct.schema, &where_clause)?
            .ok_or(CoreDBError::QueryExecutionError {
                message: "DELETE requires partition key in WHERE clause".to_string(),
            })?;
        
        // 삭제 마커로 표시 (tombstone)
        let now = chrono::Utc::now().timestamp_micros();
        let tombstone_row = SchemaRow {
            partition_key,
            clustering_key: None,
            cells: HashMap::new(), // 빈 셀 = 삭제됨
            timestamp: now,
        };

        // WAL append before memtable put. The tombstone row is the
        // in-memory representation of the delete (empty cells under
        // the same PK), so logging it as `Mutation::Insert` and
        // replaying via `memtable.put` reapplies the same tombstone
        // on startup — semantically equivalent to a structured
        // Mutation::Delete and avoids touching the replay path.
        drop(tables);
        drop(keyspaces);
        self.wal_append(
            &keyspace_name,
            &table,
            Mutation::Insert(tombstone_row.clone()),
        )
        .await;
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces
            .get(&keyspace_name)
            .ok_or(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' disappeared mid-delete", keyspace_name),
            })?;
        let tables = keyspace_struct.tables.read().await;
        let table_struct =
            tables.get(&table).ok_or(CoreDBError::QueryExecutionError {
                message: format!("Table '{}' disappeared mid-delete", table),
            })?;
        table_struct.current_memtable.put(tombstone_row)?;

        Ok(QueryResult::Success)
    }
    
    /// BATCH 실행
    async fn execute_batch(&mut self, statements: Vec<CqlStatement>) -> Result<QueryResult> {
        // 모든 문장을 순서대로 실행 (INSERT, UPDATE, DELETE만 허용)
        for stmt in statements {
            match stmt {
                CqlStatement::Insert { keyspace, table, values, ttl, if_not_exists } => {
                    self.insert_row(keyspace, table, values, ttl, if_not_exists).await?;
                },
                CqlStatement::Update { keyspace, table, values, counter_updates, where_clause, if_conditions } => {
                    self.update_row(keyspace, table, values, counter_updates, where_clause, if_conditions).await?;
                },
                CqlStatement::Delete { keyspace, table, where_clause } => {
                    self.delete_row(keyspace, table, where_clause).await?;
                },
                _ => {
                    return Err(CoreDBError::QueryExecutionError {
                        message: "BATCH only supports INSERT, UPDATE, DELETE".to_string(),
                    });
                }
            }
        }
        
        Ok(QueryResult::Success)
    }
    
    async fn drop_table(&mut self, keyspace: String, name: String) -> Result<QueryResult> {
        let keyspace_name = if keyspace.is_empty() {
            self.current_keyspace.clone().ok_or(CoreDBError::QueryExecutionError {
                message: "No keyspace selected".to_string(),
            })?
        } else {
            keyspace
        };
        
        let keyspaces = self.keyspaces.read().await;
        let keyspace_struct = keyspaces.get(&keyspace_name).ok_or(CoreDBError::QueryExecutionError {
            message: format!("Keyspace '{}' does not exist", keyspace_name),
        })?;
        
        let mut tables = keyspace_struct.tables.write().await;
        if tables.remove(&name).is_none() {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Table '{}' does not exist", name),
            });
        }
        
        Ok(QueryResult::Success)
    }
    
    async fn drop_keyspace(&mut self, name: String) -> Result<QueryResult> {
        let mut keyspaces = self.keyspaces.write().await;
        if keyspaces.remove(&name).is_none() {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' does not exist", name),
            });
        }
        
        if let Some(current) = &self.current_keyspace {
            if current == &name {
                self.current_keyspace = None;
            }
        }
        
        Ok(QueryResult::Success)
    }
    
    async fn use_keyspace(&mut self, keyspace: String) -> Result<QueryResult> {
        let keyspaces = self.keyspaces.read().await;
        if !keyspaces.contains_key(&keyspace) {
            return Err(CoreDBError::QueryExecutionError {
                message: format!("Keyspace '{}' does not exist", keyspace),
            });
        }
        
        self.current_keyspace = Some(keyspace);
        Ok(QueryResult::Success)
    }
    
    fn extract_partition_key(&self, schema: &TableSchema, where_clause: &crate::query::parser::WhereClause) -> Result<Option<PartitionKey>> {
        let mut partition_components = Vec::new();

        // WHERE 절에서 파티션 키 컬럼 찾기
        // 간단한 구현: 파티션 키의 첫 번째 컬럼만 확인 (복합 파티션 키 미지원)
        //
        // The WHERE literal arrived from the parser with its inferred type
        // (e.g. `1778630400000` → BigInt). insert_row now coerces stored
        // values to the column's schema type (Timestamp for that column),
        // so without the same coercion here the partition lookup compares
        // BigInt(123) against Timestamp(123) and silently misses every row.
        if let Some(first_pk) = schema.partition_key.first() {
            for condition in &where_clause.conditions {
                if condition.column == first_pk.name {
                    let coerced = condition.value.clone().coerce_to(&first_pk.data_type);
                    partition_components.push(coerced);
                    break;
                }
            }
        }

        if partition_components.is_empty() {
            return Ok(None);
        }

        Ok(Some(PartitionKey {
            components: partition_components,
        }))
    }
    
    fn extract_keys_from_values(&self, values: Vec<(String, CassandraValue)>, schema: &TableSchema) -> Result<(PartitionKey, Option<ClusteringKey>)> {
        let mut partition_components = Vec::new();
        let mut clustering_components = Vec::new();
        
        let value_map: HashMap<String, CassandraValue> = values.into_iter().collect();
        
        // 파티션 키 구성
        for pk_column in &schema.partition_key {
            if let Some(value) = value_map.get(&pk_column.name) {
                partition_components.push(value.clone());
            } else {
                return Err(CoreDBError::InvalidSchema {
                    message: format!("Missing partition key column: {}", pk_column.name),
                });
            }
        }
        
        // 클러스터링 키 구성 (있는 경우)
        if !schema.clustering_key.is_empty() {
            for ck_column in &schema.clustering_key {
                if let Some(value) = value_map.get(&ck_column.name) {
                    clustering_components.push(value.clone());
                } else {
                    return Err(CoreDBError::InvalidSchema {
                        message: format!("Missing clustering key column: {}", ck_column.name),
                    });
                }
            }
        }
        
        let partition_key = PartitionKey {
            components: partition_components,
        };
        
        let clustering_key = if clustering_components.is_empty() {
            None
        } else {
            Some(ClusteringKey {
                components: clustering_components,
            })
        };
        
        Ok((partition_key, clustering_key))
    }
    

    


    /// 조건이 값과 매칭되는지 확인
    fn matches_condition(value: &CassandraValue, condition: &crate::query::parser::Condition) -> bool {
        use crate::query::parser::ComparisonOperator;
        
        match &condition.operator {
            ComparisonOperator::Equal => value == &condition.value,
            ComparisonOperator::NotEqual => value != &condition.value,
            ComparisonOperator::GreaterThan => {
                Self::compare_values(value, &condition.value).map(|o| o == std::cmp::Ordering::Greater).unwrap_or(false)
            },
            ComparisonOperator::GreaterThanOrEqual => {
                Self::compare_values(value, &condition.value).map(|o| o != std::cmp::Ordering::Less).unwrap_or(false)
            },
            ComparisonOperator::LessThan => {
                Self::compare_values(value, &condition.value).map(|o| o == std::cmp::Ordering::Less).unwrap_or(false)
            },
            ComparisonOperator::LessThanOrEqual => {
                Self::compare_values(value, &condition.value).map(|o| o != std::cmp::Ordering::Greater).unwrap_or(false)
            },
            ComparisonOperator::Like => {
                if let (CassandraValue::Text(text), CassandraValue::Text(pattern)) = (value, &condition.value) {
                    let regex_pattern = pattern
                        .replace('%', ".*")
                        .replace('_', ".");
                    regex::Regex::new(&format!("^{}$", regex_pattern))
                        .map(|re| re.is_match(text))
                        .unwrap_or(false)
                } else {
                    false
                }
            },
            ComparisonOperator::In => {
                // condition.value는 List이고, value가 그 리스트에 포함되어 있는지 확인
                match &condition.value {
                    CassandraValue::List(values) => values.contains(value),
                    _ => value == &condition.value,
                }
            },
        }
    }
    
    /// 두 값 비교
    fn compare_values(a: &CassandraValue, b: &CassandraValue) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (CassandraValue::Int(a), CassandraValue::Int(b)) => Some(a.cmp(b)),
            (CassandraValue::BigInt(a), CassandraValue::BigInt(b)) => Some(a.cmp(b)),
            (CassandraValue::Double(a), CassandraValue::Double(b)) => a.partial_cmp(b),
            (CassandraValue::Text(a), CassandraValue::Text(b)) => Some(a.cmp(b)),
            (CassandraValue::Timestamp(a), CassandraValue::Timestamp(b)) => Some(a.cmp(b)),
            (CassandraValue::Int(a), CassandraValue::BigInt(b)) => Some((*a as i64).cmp(b)),
            (CassandraValue::BigInt(a), CassandraValue::Int(b)) => Some(a.cmp(&(*b as i64))),
            (CassandraValue::Int(a), CassandraValue::Double(b)) => (*a as f64).partial_cmp(b),
            (CassandraValue::Double(a), CassandraValue::Int(b)) => a.partial_cmp(&(*b as f64)),
            (CassandraValue::BigInt(a), CassandraValue::Double(b)) => (*a as f64).partial_cmp(b),
            (CassandraValue::Double(a), CassandraValue::BigInt(b)) => a.partial_cmp(&(*b as f64)),
            _ => None,
        }
    }
    
    /// TOKEN 함수 - 파티션 키의 해시 값 계산
    fn compute_token(partition_key: &crate::schema::PartitionKey) -> i64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        
        let mut hasher = DefaultHasher::new();
        for component in &partition_key.components {
            match component {
                CassandraValue::Int(v) => v.hash(&mut hasher),
                CassandraValue::BigInt(v) => v.hash(&mut hasher),
                CassandraValue::Text(v) => v.hash(&mut hasher),
                CassandraValue::UUID(v) => v.hash(&mut hasher),
                CassandraValue::Boolean(v) => v.hash(&mut hasher),
                CassandraValue::Blob(v) => v.hash(&mut hasher),
                _ => 0i64.hash(&mut hasher),
            }
        }
        
        // i64 범위로 변환 (Cassandra token 범위: -2^63 ~ 2^63-1)
        hasher.finish() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, CassandraDataType};
    
    #[tokio::test]
    async fn test_create_keyspace_and_table() {
        let keyspaces = Arc::new(RwLock::new(HashMap::new()));
        let mut engine = QueryEngine::new(keyspaces);
        
        // 키스페이스 생성
        let create_ks = CqlStatement::CreateKeyspace {
            name: "test_ks".to_string(),
            options: crate::query::parser::KeyspaceOptions {
                replication_factor: 1,
                strategy: "SimpleStrategy".to_string(),
            },
        };
        
        let result = engine.execute(create_ks).await.unwrap();
        assert!(result.is_success());
        
        // 테이블 생성
        let create_table = CqlStatement::CreateTable {
            keyspace: "test_ks".to_string(),
            name: "test_table".to_string(),
            columns: vec![
                ColumnDefinition {
                    name: "id".to_string(),
                    data_type: CassandraDataType::Int,
                    is_static: false,
                },
                ColumnDefinition {
                    name: "name".to_string(),
                    data_type: CassandraDataType::Text,
                    is_static: false,
                },
            ],
            partition_key: vec!["id".to_string()],
            clustering_key: vec![],
            options: crate::query::parser::TableOptions {
                compaction_strategy: "SizeTiered".to_string(),
                bloom_filter_fp_chance: 0.01,
                default_time_to_live: None,
            },
        };
        
        let result = engine.execute(create_table).await.unwrap();
        assert!(result.is_success());
    }
    
    #[tokio::test]
    async fn test_insert_and_select() {
        let keyspaces = Arc::new(RwLock::new(HashMap::new()));
        let mut engine = QueryEngine::new(keyspaces);
        
        // 키스페이스와 테이블 생성
        engine.execute(CqlStatement::CreateKeyspace {
            name: "test_ks".to_string(),
            options: crate::query::parser::KeyspaceOptions {
                replication_factor: 1,
                strategy: "SimpleStrategy".to_string(),
            },
        }).await.unwrap();
        
        engine.execute(CqlStatement::CreateTable {
            keyspace: "test_ks".to_string(),
            name: "test_table".to_string(),
            columns: vec![
                ColumnDefinition {
                    name: "id".to_string(),
                    data_type: CassandraDataType::Int,
                    is_static: false,
                },
                ColumnDefinition {
                    name: "name".to_string(),
                    data_type: CassandraDataType::Text,
                    is_static: false,
                },
            ],
            partition_key: vec!["id".to_string()],
            clustering_key: vec![],
            options: crate::query::parser::TableOptions {
                compaction_strategy: "SizeTiered".to_string(),
                bloom_filter_fp_chance: 0.01,
                default_time_to_live: None,
            },
        }).await.unwrap();
        
        // 데이터 삽입
        let insert = CqlStatement::Insert {
            keyspace: "test_ks".to_string(),
            table: "test_table".to_string(),
            values: vec![
                ("id".to_string(), CassandraValue::Int(1)),
                ("name".to_string(), CassandraValue::Text("John".to_string())),
            ],
            ttl: None,
            if_not_exists: false,
        };
        
        let result = engine.execute(insert).await.unwrap();
        assert!(result.is_success());
        
        // 데이터 조회
        let select = CqlStatement::Select {
            keyspace: "test_ks".to_string(),
            table: "test_table".to_string(),
            columns: vec!["*".to_string()],
            where_clause: Some(crate::query::parser::WhereClause {
                conditions: vec![crate::query::parser::Condition {
                    column: "id".to_string(),
                    operator: crate::query::parser::ComparisonOperator::Equal,
                    value: CassandraValue::Int(1),
                    is_token: false,
                }],
            }),
            limit: None,
            aggregations: vec![],
            distinct: false,
            allow_filtering: false,
            order_by: None,
            group_by: vec![],
            page_size: None,
            paging_state: None,
        };
        
        let result = engine.execute(select).await.unwrap();
        if let QueryResult::Rows(rows) = result {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get_column("name"), Some(&CassandraValue::Text("John".to_string())));
        } else {
            panic!("Expected rows result");
        }
    }
}
