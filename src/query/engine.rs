use crate::schema::{TableSchema, PartitionKey, ClusteringKey, CassandraValue, Row as SchemaRow, Cell};
use crate::storage::{Memtable, SSTable};
use crate::query::{CqlStatement, QueryResult, Row as QueryRow};
use crate::error::*;
use std::sync::Arc;
use std::collections::{HashMap, BTreeMap};

/// 쿼리 엔진
pub struct QueryEngine {
    memtables: HashMap<String, HashMap<String, Arc<Memtable>>>,
    sstables: HashMap<String, HashMap<String, Vec<Arc<SSTable>>>>,
}

impl QueryEngine {
    pub fn new() -> Self {
        Self {
            memtables: HashMap::new(),
            sstables: HashMap::new(),
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
            CqlStatement::Insert { keyspace, table, values } => {
                self.insert_row(keyspace, table, values).await
            },
            CqlStatement::Select { keyspace, table, columns, where_clause, limit } => {
                self.select_rows(keyspace, table, columns, where_clause, limit).await
            },
            CqlStatement::Update { keyspace, table, values, where_clause } => {
                self.update_row(keyspace, table, values, where_clause).await
            },
            CqlStatement::Delete { keyspace, table, where_clause } => {
                self.delete_row(keyspace, table, where_clause).await
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
        }
    }
    
    async fn create_keyspace(&mut self, name: String, _options: crate::query::parser::KeyspaceOptions) -> Result<QueryResult> {
        // 키스페이스 생성 (단순화된 버전)
        if !self.memtables.contains_key(&name) {
            self.memtables.insert(name.clone(), HashMap::new());
            self.sstables.insert(name, HashMap::new());
        }
        Ok(QueryResult::success())
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
        let memtable = Arc::new(Memtable::new(schema));
        
        if let Some(tables) = self.memtables.get_mut(&keyspace) {
            tables.insert(name.clone(), memtable);
        }
        
        if let Some(tables) = self.sstables.get_mut(&keyspace) {
            tables.insert(name, Vec::new());
        }
        
        Ok(QueryResult::success())
    }
    
    async fn insert_row(&mut self, keyspace: String, table: String, values: Vec<(String, CassandraValue)>) -> Result<QueryResult> {
        // 테이블 찾기
        let memtable = self.get_memtable(&keyspace, &table)?;
        let schema = memtable.table_schema();
        
        // 파티션 키와 클러스터링 키 추출
        let (partition_key, clustering_key) = self.extract_keys_from_values(values.clone(), schema)?;
        
        // 행 생성
        let mut cells = HashMap::new();
        for (column_name, value) in values {
            let cell = Cell {
                value,
                timestamp: chrono::Utc::now().timestamp_micros(),
                ttl: None,
                is_deleted: false,
            };
            cells.insert(column_name, cell);
        }
        
        let row = SchemaRow {
            partition_key,
            clustering_key,
            cells,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        // 메모리 테이블에 삽입
        memtable.put(row)?;
        
        Ok(QueryResult::success())
    }
    
    async fn select_rows(&mut self, keyspace: String, table: String, columns: Vec<String>, where_clause: Option<crate::query::parser::WhereClause>, limit: Option<u32>) -> Result<QueryResult> {
        // 테이블 찾기
        let memtable = self.get_memtable(&keyspace, &table)?;
        let schema = memtable.table_schema();
        
        let mut results = Vec::new();
        
        if let Some(where_clause) = where_clause {
            // WHERE 절이 있는 경우
            if where_clause.conditions.len() == 1 {
                let condition = &where_clause.conditions[0];
                if condition.column == schema.partition_key[0].name {
                    // 파티션 키 조건인 경우
                    let partition_key = PartitionKey {
                        components: vec![condition.value.clone()],
                    };
                    
                    if let Some(clustering_condition) = where_clause.conditions.get(1) {
                        // 클러스터링 키 조건도 있는 경우
                        let clustering_key = Some(ClusteringKey {
                            components: vec![clustering_condition.value.clone()],
                        });
                        
                        if let Some(row) = memtable.get(&partition_key, &clustering_key) {
                            results.push(self.convert_schema_row_to_query_row(row, &columns));
                        }
                    } else {
                        // 파티션 전체 스캔
                        let partition_rows = memtable.range_scan(&partition_key, &None, &None);
                        for row in partition_rows {
                            results.push(self.convert_schema_row_to_query_row(row, &columns));
                        }
                    }
                }
            }
        } else {
            // WHERE 절이 없는 경우 - 전체 테이블 스캔 (실제로는 비효율적)
            let all_partitions = memtable.get_all_partitions();
            for (_, partition) in all_partitions {
                for row_entry in partition.rows.iter() {
                    let row = row_entry.value();
                    results.push(self.convert_schema_row_to_query_row(row.clone(), &columns));
                }
            }
        }
        
        // LIMIT 적용
        if let Some(limit) = limit {
            results.truncate(limit as usize);
        }
        
        Ok(QueryResult::rows(results))
    }
    
    async fn update_row(&mut self, _keyspace: String, _table: String, _values: Vec<(String, CassandraValue)>, _where_clause: crate::query::parser::WhereClause) -> Result<QueryResult> {
        // UPDATE는 INSERT로 구현 (Cassandra 스타일)
        Err(CoreDBError::QueryParsingError {
            message: "UPDATE not implemented yet".to_string(),
        })
    }
    
    async fn delete_row(&mut self, _keyspace: String, _table: String, _where_clause: crate::query::parser::WhereClause) -> Result<QueryResult> {
        // DELETE 구현
        Err(CoreDBError::QueryParsingError {
            message: "DELETE not implemented yet".to_string(),
        })
    }
    
    async fn drop_table(&mut self, keyspace: String, name: String) -> Result<QueryResult> {
        if let Some(tables) = self.memtables.get_mut(&keyspace) {
            tables.remove(&name);
        }
        
        if let Some(tables) = self.sstables.get_mut(&keyspace) {
            tables.remove(&name);
        }
        
        Ok(QueryResult::success())
    }
    
    async fn drop_keyspace(&mut self, name: String) -> Result<QueryResult> {
        self.memtables.remove(&name);
        self.sstables.remove(&name);
        Ok(QueryResult::success())
    }
    
    async fn use_keyspace(&mut self, _keyspace: String) -> Result<QueryResult> {
        // 현재 키스페이스 설정 (단순화된 버전)
        Ok(QueryResult::success())
    }
    
    fn get_memtable(&self, keyspace: &str, table: &str) -> Result<Arc<Memtable>> {
        self.memtables
            .get(keyspace)
            .ok_or_else(|| CoreDBError::KeyspaceNotFound { keyspace: keyspace.to_string() })?
            .get(table)
            .ok_or_else(|| CoreDBError::TableNotFound { table: table.to_string() })
            .map(|m| m.clone())
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
    
    fn convert_schema_row_to_query_row(&self, row: SchemaRow, requested_columns: &[String]) -> QueryRow {
        let mut query_row = QueryRow::new();
        
        if requested_columns.contains(&"*".to_string()) {
            // 모든 컬럼 반환
            for (column_name, cell) in row.cells {
                query_row = query_row.with_column(column_name, cell.value);
            }
        } else {
            // 요청된 컬럼만 반환
            for column_name in requested_columns {
                if let Some(cell) = row.cells.get(column_name) {
                    query_row = query_row.with_column(column_name.clone(), cell.value.clone());
                }
            }
        }
        
        query_row
    }
    
    /// 메모리 테이블에 SSTable 추가
    pub fn add_sstable(&mut self, keyspace: String, table: String, sstable: Arc<SSTable>) {
        if let Some(tables) = self.sstables.get_mut(&keyspace) {
            if let Some(sstables) = tables.get_mut(&table) {
                sstables.push(sstable);
            }
        }
    }
    
    /// 메모리 테이블 교체
    pub fn replace_memtable(&mut self, keyspace: String, table: String, memtable: Arc<Memtable>) {
        if let Some(tables) = self.memtables.get_mut(&keyspace) {
            tables.insert(table, memtable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ColumnDefinition, CassandraDataType};
    
    #[tokio::test]
    async fn test_create_keyspace_and_table() {
        let mut engine = QueryEngine::new();
        
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
        let mut engine = QueryEngine::new();
        
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
