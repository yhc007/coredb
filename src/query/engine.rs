use crate::schema::{TableSchema, PartitionKey, ClusteringKey, CassandraValue, Row as SchemaRow, Cell};
use crate::storage::Memtable;
use crate::query::{CqlStatement, QueryResult, Row as QueryRow};
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
    current_keyspace: Option<String>,
}

impl QueryEngine {
    pub fn new(keyspaces: Arc<RwLock<HashMap<String, Keyspace>>>) -> Self {
        Self {
            keyspaces,
            current_keyspace: None,
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
            CqlStatement::Select { keyspace, table, columns, where_clause, limit, aggregations } => {
                self.select_rows(keyspace, table, columns, where_clause, limit, aggregations).await
            },
            CqlStatement::Update { keyspace, table, values, where_clause, if_conditions } => {
                self.update_row(keyspace, table, values, where_clause, if_conditions).await
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
        }
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
        
        // 메모리 테이블에 추가
        table_struct.current_memtable.put(row)?;
        
        // IF NOT EXISTS 성공 시 [applied] = true 반환
        if if_not_exists {
            let mut result_row = HashMap::new();
            result_row.insert("[applied]".to_string(), CassandraValue::Boolean(true));
            return Ok(QueryResult::Rows(vec![QueryRow { columns: result_row }]));
        }
        
        Ok(QueryResult::Success)
    }
    
    async fn select_rows(&mut self, keyspace: String, table: String, columns: Vec<String>, where_clause: Option<crate::query::parser::WhereClause>, limit: Option<u32>, aggregations: Vec<crate::query::parser::Aggregation>) -> Result<QueryResult> {
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
        
        let mut result_rows = Vec::new();
        
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
            
            // 3. SSTables 검색
            for sstable in &table_struct.sstables {
                if let Some(partition) = sstable.read_partition(&pk).await? {
                    for entry in partition.rows.iter() {
                        result_rows.push(entry.value().clone());
                    }
                }
            }
        } else {
            // 전체 스캔
            // 1. Current Memtable
            let partitions = table_struct.current_memtable.get_all_partitions();
            for (_, partition) in partitions {
                for entry in partition.rows.iter() {
                    result_rows.push(entry.value().clone());
                }
            }
            
            // 2. Immutable Memtables
            for memtable in &table_struct.memtables {
                let partitions = memtable.get_all_partitions();
                for (_, partition) in partitions {
                    for entry in partition.rows.iter() {
                        result_rows.push(entry.value().clone());
                    }
                }
            }
            
            // 3. SSTables - full scan
            for sstable in &table_struct.sstables {
                for pk in sstable.partition_index.keys() {
                    if let Ok(Some(partition)) = sstable.read_partition(pk).await {
                        for entry in partition.rows.iter() {
                            result_rows.push(entry.value().clone());
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
            result_rows.into_iter().filter(|row| {
                for condition in &wc.conditions {
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
            let mut final_cells = HashMap::new();
            if columns.contains(&"*".to_string()) || !aggregations.is_empty() {
                // aggregation이 있으면 모든 컬럼 필요
                final_cells = cells;
            } else {
                for col in &columns {
                    if let Some(val) = cells.get(col) {
                        final_cells.insert(col.clone(), val.clone());
                    }
                }
            }
            
            query_rows.push(QueryRow { columns: final_cells });
        }
        
        // LIMIT 적용
        if let Some(l) = limit {
            query_rows.truncate(l as usize);
        }
        
        // Aggregation 처리
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
    
    async fn update_row(&mut self, keyspace: String, table: String, values: Vec<(String, CassandraValue)>, where_clause: crate::query::parser::WhereClause, if_conditions: Option<Vec<crate::query::parser::Condition>>) -> Result<QueryResult> {
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
        
        let new_row = SchemaRow {
            partition_key,
            clustering_key: None,
            cells,
            timestamp: now,
        };
        
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
                CqlStatement::Update { keyspace, table, values, where_clause, if_conditions } => {
                    self.update_row(keyspace, table, values, where_clause, if_conditions).await?;
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
        if let Some(first_pk) = schema.partition_key.first() {
            for condition in &where_clause.conditions {
                if condition.column == first_pk.name {
                    partition_components.push(condition.value.clone());
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
                value == &condition.value
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
            _ => None,
        }
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
                }],
            }),
            limit: None,
            aggregations: vec![],
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
