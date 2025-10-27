use crossbeam_skiplist::SkipMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::collections::HashMap;
use crate::schema::{PartitionKey, ClusteringKey, Row, TableSchema};
use crate::error::*;

/// 메모리 테이블의 파티션
#[derive(Debug)]
pub struct Partition {
    /// 클러스터링 키로 정렬된 행들
    pub rows: SkipMap<Option<ClusteringKey>, Row>,
    /// 정적 컬럼들
    pub static_columns: HashMap<String, crate::schema::Cell>,
}

impl Partition {
    fn new() -> Self {
        Self {
            rows: SkipMap::new(),
            static_columns: HashMap::new(),
        }
    }
}

/// 메모리 테이블
#[derive(Debug)]
pub struct Memtable {
    /// 파티션별로 데이터 구조화
    partitions: SkipMap<PartitionKey, Partition>,
    /// 메모리 사용량 (바이트)
    size_bytes: AtomicU64,
    /// 생성 시간
    creation_time: i64,
    /// 테이블 스키마
    table_schema: Arc<TableSchema>,
}

impl Memtable {
    pub fn new(schema: Arc<TableSchema>) -> Self {
        Self {
            partitions: SkipMap::new(),
            size_bytes: AtomicU64::new(0),
            creation_time: chrono::Utc::now().timestamp_micros(),
            table_schema: schema,
        }
    }
    
    pub fn put(&self, row: Row) -> Result<()> {
        let partition_key = row.partition_key.clone();
        let clustering_key = row.clustering_key.clone();
        
        // 파티션 가져오거나 생성
        let partition = self.partitions
            .get_or_insert_with(partition_key.clone(), || Partition::new());
        
        // 행 크기 계산
        let row_size = self.calculate_row_size(&row);
        
        // 기존 행이 있다면 크기 차이 계산
        if let Some(existing_entry) = partition.value().rows.get(&clustering_key) {
            let old_row_size = self.calculate_row_size(existing_entry.value());
            let size_delta = row_size as i64 - old_row_size as i64;
            self.size_bytes.fetch_add(size_delta as u64, Ordering::Relaxed);
        } else {
            self.size_bytes.fetch_add(row_size, Ordering::Relaxed);
        }
        
        // 행 삽입/업데이트
        partition.value().rows.insert(clustering_key, row);
        
        Ok(())
    }
    
    pub fn get(&self, partition_key: &PartitionKey, clustering_key: &Option<ClusteringKey>) 
        -> Option<Row> {
        self.partitions.get(partition_key)?
            .value().rows.get(clustering_key)
            .map(|entry| entry.value().clone())
    }
    
    pub fn range_scan(&self, 
        partition_key: &PartitionKey,
        start_clustering: &Option<ClusteringKey>,
        end_clustering: &Option<ClusteringKey>
    ) -> Vec<Row> {
        if let Some(partition) = self.partitions.get(partition_key) {
            partition.value().rows
                .range(start_clustering..=end_clustering)
                .map(|entry| entry.value().clone())
                .collect()
        } else {
            Vec::new()
        }
    }
    
    pub fn get_all_partitions(&self) -> Vec<(PartitionKey, Partition)> {
        self.partitions.iter()
            .map(|entry| {
                let key = entry.key().clone();
                let partition = entry.value();
                // Clone Partition manually since SkipMap doesn't implement Clone
                let mut new_partition = Partition::new();
                new_partition.static_columns = partition.static_columns.clone();
                for row_entry in partition.rows.iter() {
                    new_partition.rows.insert(row_entry.key().clone(), row_entry.value().clone());
                }
                (key, new_partition)
            })
            .collect()
    }
    
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes.load(Ordering::Relaxed)
    }
    
    pub fn partition_count(&self) -> usize {
        self.partitions.len()
    }
    
    pub fn creation_time(&self) -> i64 {
        self.creation_time
    }
    
    pub fn table_schema(&self) -> &Arc<TableSchema> {
        &self.table_schema
    }
    
    fn calculate_row_size(&self, row: &Row) -> u64 {
        // 행 크기 추정 (키 + 값 + 메타데이터)
        let mut size = 0u64;
        
        // 파티션 키 크기
        size += row.partition_key.serialized_size();
        
        // 클러스터링 키 크기
        if let Some(ref ck) = row.clustering_key {
            size += ck.serialized_size();
        }
        
        // 셀들 크기
        for (column_name, cell) in &row.cells {
            size += column_name.len() as u64;
            size += cell.value.serialized_size();
            size += 16; // timestamp + ttl + flags
        }
        
        size
    }
}

impl Clone for Memtable {
    fn clone(&self) -> Self {
        // SkipMap과 AtomicU64는 Clone을 지원하지 않으므로
        // 새로운 Memtable을 생성하고 데이터를 복사
        let mut new_memtable = Self::new(self.table_schema.clone());
        
        for entry in self.partitions.iter() {
            let partition_key = entry.key().clone();
            let partition = entry.value();
            
            let mut new_partition = Partition::new();
            
            // 행들 복사
            for row_entry in partition.rows.iter() {
                let clustering_key = row_entry.key().clone();
                let row = row_entry.value().clone();
                new_partition.rows.insert(clustering_key, row);
            }
            
            // 정적 컬럼들 복사
            new_partition.static_columns = partition.static_columns.clone();
            
            new_memtable.partitions.insert(partition_key, new_partition);
        }
        
        new_memtable.size_bytes.store(self.size_bytes.load(Ordering::Relaxed), Ordering::Relaxed);
        new_memtable.creation_time = self.creation_time;
        
        new_memtable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CassandraValue, ColumnDefinition, CassandraDataType, Cell};
    
    fn create_test_schema() -> Arc<TableSchema> {
        Arc::new(crate::schema::TableSchema::new(
            "test_table".to_string(),
            "test_keyspace".to_string(),
            vec![ColumnDefinition {
                name: "id".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            }],
            vec![ColumnDefinition {
                name: "timestamp".to_string(),
                data_type: CassandraDataType::BigInt,
                is_static: false,
            }],
            vec![ColumnDefinition {
                name: "value".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            }],
            vec![],
        ))
    }
    
    fn create_test_row(id: i32, timestamp: i64, value: &str) -> Row {
        use std::collections::HashMap;
        
        Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(id)],
            },
            clustering_key: Some(ClusteringKey {
                components: vec![CassandraValue::BigInt(timestamp)],
            }),
            cells: {
                let mut cells = HashMap::new();
                cells.insert("value".to_string(), Cell {
                    value: CassandraValue::Text(value.to_string()),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        }
    }
    
    #[test]
    fn test_memtable_put_and_get() {
        let schema = create_test_schema();
        let memtable = Memtable::new(schema);
        
        let row = create_test_row(1, 1000, "test_value");
        memtable.put(row.clone()).unwrap();
        
        let retrieved = memtable.get(
            &row.partition_key,
            &row.clustering_key
        );
        
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().cells["value"].value, CassandraValue::Text("test_value".to_string()));
    }
    
    #[test]
    fn test_memtable_range_scan() {
        let schema = create_test_schema();
        let memtable = Memtable::new(schema);
        
        // 여러 행 추가
        for i in 1..=5 {
            let row = create_test_row(1, i * 1000, &format!("value_{}", i));
            memtable.put(row).unwrap();
        }
        
        let start_key = Some(ClusteringKey {
            components: vec![CassandraValue::BigInt(2000)],
        });
        let end_key = Some(ClusteringKey {
            components: vec![CassandraValue::BigInt(4000)],
        });
        
        let results = memtable.range_scan(
            &PartitionKey {
                components: vec![CassandraValue::Int(1)],
            },
            &start_key,
            &end_key
        );
        
        assert_eq!(results.len(), 3); // timestamp 2000, 3000, 4000
    }
    
    #[test]
    fn test_memtable_size_tracking() {
        let schema = create_test_schema();
        let memtable = Memtable::new(schema);
        
        let initial_size = memtable.size_bytes();
        
        let row = create_test_row(1, 1000, "test_value");
        memtable.put(row).unwrap();
        
        assert!(memtable.size_bytes() > initial_size);
    }
}
