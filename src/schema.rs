use serde::{Serialize, Deserialize};
use std::collections::HashMap;
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
    Counter,  // 분산 카운터 타입
    Map(Box<CassandraDataType>, Box<CassandraDataType>),
    List(Box<CassandraDataType>),
    Set(Box<CassandraDataType>),
    UDT(String, String),  // (keyspace, type_name)
}

/// User Defined Type 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDefinedType {
    pub keyspace: String,
    pub name: String,
    pub fields: Vec<UDTField>,
}

/// UDT 필드
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UDTField {
    pub name: String,
    pub data_type: CassandraDataType,
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

/// Materialized View 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterializedView {
    pub name: String,
    pub keyspace: String,
    pub base_table: String,
    pub partition_key: Vec<String>,
    pub clustering_key: Vec<String>,
    pub columns: Vec<String>,  // SELECT 컬럼들 ("*" 또는 컬럼 목록)
    pub where_clause: Option<String>,  // WHERE 조건 (문자열로 저장)
}

/// Cassandra 값 타입
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CassandraValue {
    Text(String),
    Int(i32),
    BigInt(i64),
    UUID(Uuid),
    Timestamp(i64), // microseconds since epoch
    Boolean(bool),
    Double(f64),
    Blob(Vec<u8>),  // Changed from Bytes to Vec<u8> for serde compatibility
    Counter(i64),   // 분산 카운터 (증감 연산)
    Null,
    Map(HashMap<String, CassandraValue>),  // HashMap doesn't implement Ord
    List(Vec<CassandraValue>),
    Set(Vec<CassandraValue>),
    UDT(HashMap<String, CassandraValue>),  // UDT 필드명 -> 값
}

// Custom Eq implementation for CassandraValue
impl Eq for CassandraValue {}

// Custom PartialOrd implementation
impl PartialOrd for CassandraValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        use CassandraValue::*;
        
        match (self, other) {
            (Text(a), Text(b)) => a.partial_cmp(b),
            (Int(a), Int(b)) => a.partial_cmp(b),
            (BigInt(a), BigInt(b)) => a.partial_cmp(b),
            (UUID(a), UUID(b)) => a.partial_cmp(b),
            (Timestamp(a), Timestamp(b)) => a.partial_cmp(b),
            (Boolean(a), Boolean(b)) => a.partial_cmp(b),
            (Double(a), Double(b)) => a.partial_cmp(b),
            (Blob(a), Blob(b)) => a.partial_cmp(b),
            (Counter(a), Counter(b)) => a.partial_cmp(b),
            (List(a), List(b)) => a.partial_cmp(b),
            (Set(a), Set(b)) => a.partial_cmp(b),
            (Null, Null) => Some(Ordering::Equal),
            (Map(_), Map(_)) => Some(Ordering::Equal), // Maps cannot be ordered
            (UDT(_), UDT(_)) => Some(Ordering::Equal), // UDTs cannot be ordered
            _ => None,
        }
    }
}

// Custom Ord implementation
impl Ord for CassandraValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
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
            CassandraValue::Counter(_) => 8,
            CassandraValue::Null => 1,
            CassandraValue::Map(m) => {
                let mut size = 8; // length prefix
                for (k, v) in m {
                    size += 8 + k.len() as u64 + v.serialized_size(); // String key + value
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
            CassandraValue::UDT(fields) => {
                let mut size = 8; // length prefix
                for (k, v) in fields {
                    size += 8 + k.len() as u64 + v.serialized_size();
                }
                size
            },
        }
    }
}

/// 파티션 키
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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

// ============================================================================
// Authentication & Authorization
// ============================================================================

/// 사용자 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub password_hash: String,
    pub is_superuser: bool,
    pub created_at: i64,
    pub roles: Vec<String>,
}

/// 역할 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub is_superuser: bool,
    pub can_login: bool,
    pub permissions: Vec<Permission>,
}

/// 권한 정의
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Permission {
    pub permission_type: PermissionType,
    pub resource: Resource,
}

/// 권한 타입
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionType {
    All,
    Create,
    Alter,
    Drop,
    Select,
    Modify,  // INSERT, UPDATE, DELETE
    Authorize,
    Describe,
    Execute,
}

/// 리소스 타입
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    AllKeyspaces,
    Keyspace(String),
    Table { keyspace: String, table: String },
    AllRoles,
    Role(String),
    AllFunctions,
    Function { keyspace: String, name: String },
}

impl User {
    pub fn new(name: String, password_hash: String, is_superuser: bool) -> Self {
        Self {
            name,
            password_hash,
            is_superuser,
            created_at: chrono::Utc::now().timestamp_millis(),
            roles: vec![],
        }
    }
    
    pub fn has_permission(&self, permission: &Permission, roles: &std::collections::HashMap<String, Role>) -> bool {
        if self.is_superuser {
            return true;
        }
        
        for role_name in &self.roles {
            if let Some(role) = roles.get(role_name) {
                if role.is_superuser {
                    return true;
                }
                for perm in &role.permissions {
                    if perm.matches(permission) {
                        return true;
                    }
                }
            }
        }
        
        false
    }
}

impl Permission {
    pub fn matches(&self, other: &Permission) -> bool {
        // ALL permission matches everything
        if self.permission_type == PermissionType::All {
            return self.resource.contains(&other.resource);
        }
        
        // Check permission type match
        if self.permission_type != other.permission_type {
            return false;
        }
        
        // Check resource containment
        self.resource.contains(&other.resource)
    }
}

impl Resource {
    pub fn contains(&self, other: &Resource) -> bool {
        match (self, other) {
            (Resource::AllKeyspaces, Resource::Keyspace(_)) => true,
            (Resource::AllKeyspaces, Resource::Table { .. }) => true,
            (Resource::AllKeyspaces, Resource::AllKeyspaces) => true,
            (Resource::Keyspace(ks1), Resource::Keyspace(ks2)) => ks1 == ks2,
            (Resource::Keyspace(ks1), Resource::Table { keyspace, .. }) => ks1 == keyspace,
            (Resource::Table { keyspace: ks1, table: t1 }, Resource::Table { keyspace: ks2, table: t2 }) => {
                ks1 == ks2 && t1 == t2
            }
            (Resource::AllRoles, Resource::Role(_)) => true,
            (Resource::AllRoles, Resource::AllRoles) => true,
            (Resource::Role(r1), Resource::Role(r2)) => r1 == r2,
            _ => self == other,
        }
    }
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
    pub fn get_column(&self, name: &str) -> Option<&ColumnDefinition> {
        self.partition_key.iter().find(|c| c.name == name)
            .or_else(|| self.clustering_key.iter().find(|c| c.name == name))
            .or_else(|| self.regular_columns.iter().find(|c| c.name == name))
            .or_else(|| self.static_columns.iter().find(|c| c.name == name))
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
