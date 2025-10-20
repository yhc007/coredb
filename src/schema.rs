use serde::{Serialize, Deserialize};
use std::collections::{HashMap, BTreeMap, BTreeSet};
use bytes::Bytes;
use uuid::Uuid;
use crate::error::*;

/// Cassandra 데이터 타입 정의
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CassandraDataType {
    Text,
    Int,
    BigInt,
    UUID,
    Timestamp,
    Boolean,
    Double,
    Blob,
    Map(Box<CassandraDataType>, Box<CassandraDataType>),
    List(Box<CassandraDataType>),
    Set(Box<CassandraDataType>),
}

/// 컬럼 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDefinition {
    pub name: String,
    pub data_type: CassandraDataType,
    pub is_static: bool,
}

/// 테이블 옵션
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableOptions {
    pub compaction_strategy: CompactionStrategy,
    pub bloom_filter_fp_chance: f64,
    pub default_time_to_live: Option<u32>,
    pub gc_grace_seconds: u32,
}

/// 컴팩션 전략
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionStrategy {
    SizeTiered,
    Leveled,
    TimeWindow,
}

/// 테이블 스키마
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub keyspace: String,
    pub partition_key: Vec<ColumnDefinition>,
    pub clustering_key: Vec<ColumnDefinition>,
    pub regular_columns: Vec<ColumnDefinition>,
    pub static_columns: Vec<ColumnDefinition>,
    pub options: TableOptions,
}

/// Cassandra 값 타입
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum CassandraValue {
    Text(String),
    Int(i32),
    BigInt(i64),
    UUID(Uuid),
    Timestamp(i64), // microseconds since epoch
    Boolean(bool),
    Double(f64),
    Blob(Bytes),
    Null,
    Map(BTreeMap<CassandraValue, CassandraValue>),
    List(Vec<CassandraValue>),
    Set(BTreeSet<CassandraValue>),
}

impl CassandraValue {
    pub fn serialized_size(&self) -> u64 {
        match self {
            CassandraValue::Text(s) => 8 + s.len() as u64,
            CassandraValue::Int(_) => 4,
            CassandraValue::BigInt(_) => 8,
            CassandraValue::UUID(_) => 16,
            CassandraValue::Timestamp(_) => 8,
            CassandraValue::Boolean(_) => 1,
            CassandraValue::Double(_) => 8,
            CassandraValue::Blob(b) => 8 + b.len() as u64,
            CassandraValue::Null => 1,
            CassandraValue::Map(m) => {
                let mut size = 8; // length prefix
                for (k, v) in m {
                    size += k.serialized_size() + v.serialized_size();
                }
                size
            },
            CassandraValue::List(l) => {
                let mut size = 8; // length prefix
                for item in l {
                    size += item.serialized_size();
                }
                size
            },
            CassandraValue::Set(s) => {
                let mut size = 8; // length prefix
                for item in s {
                    size += item.serialized_size();
                }
                size
            },
        }
    }
}

/// 파티션 키
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct PartitionKey {
    pub components: Vec<CassandraValue>,
}

impl PartitionKey {
    pub fn serialized_size(&self) -> u64 {
        let mut size = 8; // length prefix
        for component in &self.components {
            size += component.serialized_size();
        }
        size
    }
}

/// 클러스터링 키
#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ClusteringKey {
    pub components: Vec<CassandraValue>,
}

impl ClusteringKey {
    pub fn serialized_size(&self) -> u64 {
        let mut size = 8; // length prefix
        for component in &self.components {
            size += component.serialized_size();
        }
        size
    }
}

/// 셀 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    pub value: CassandraValue,
    pub timestamp: i64,
    pub ttl: Option<u32>,
    pub is_deleted: bool,
}

/// 행 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub partition_key: PartitionKey,
    pub clustering_key: Option<ClusteringKey>,
    pub cells: HashMap<String, Cell>,
    pub timestamp: i64, // write timestamp
}

/// 키스페이스 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyspaceDefinition {
    pub name: String,
    pub replication_factor: u32,
    pub strategy: ReplicationStrategy,
}

/// 복제 전략 (단일 노드에서는 단순화)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationStrategy {
    SimpleStrategy,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            compaction_strategy: CompactionStrategy::SizeTiered,
            bloom_filter_fp_chance: 0.01,
            default_time_to_live: None,
            gc_grace_seconds: 864000, // 10 days
        }
    }
}

impl TableSchema {
    pub fn new(
        name: String,
        keyspace: String,
        partition_key: Vec<ColumnDefinition>,
        clustering_key: Vec<ColumnDefinition>,
        regular_columns: Vec<ColumnDefinition>,
        static_columns: Vec<ColumnDefinition>,
    ) -> Self {
        Self {
            name,
            keyspace,
            partition_key,
            clustering_key,
            regular_columns,
            static_columns,
            options: TableOptions::default(),
        }
    }
    
    pub fn validate(&self) -> Result<()> {
        if self.partition_key.is_empty() {
            return Err(CoreDBError::InvalidSchema {
                message: "Partition key cannot be empty".to_string(),
            });
        }
        
        // 파티션 키와 클러스터링 키에 중복 컬럼이 있는지 확인
        let mut all_key_columns = std::collections::HashSet::new();
        
        for col in &self.partition_key {
            if !all_key_columns.insert(&col.name) {
                return Err(CoreDBError::InvalidSchema {
                    message: format!("Duplicate column in key: {}", col.name),
                });
            }
        }
        
        for col in &self.clustering_key {
            if !all_key_columns.insert(&col.name) {
                return Err(CoreDBError::InvalidSchema {
                    message: format!("Duplicate column in key: {}", col.name),
                });
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_table_schema_validation() {
        let schema = TableSchema::new(
            "test_table".to_string(),
            "test_keyspace".to_string(),
            vec![ColumnDefinition {
                name: "id".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            }],
            vec![],
            vec![],
            vec![],
        );
        
        assert!(schema.validate().is_ok());
    }
    
    #[test]
    fn test_invalid_schema_empty_partition_key() {
        let schema = TableSchema::new(
            "test_table".to_string(),
            "test_keyspace".to_string(),
            vec![],
            vec![],
            vec![],
            vec![],
        );
        
        assert!(schema.validate().is_err());
    }
}
