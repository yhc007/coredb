use std::collections::{BTreeMap, HashSet};
use std::sync::RwLock;
use serde::{Serialize, Deserialize};
use crate::schema::{CassandraValue, PartitionKey};

/// Secondary Index 정의
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDefinition {
    pub name: String,
    pub keyspace: String,
    pub table: String,
    pub column: String,
    pub created_at: i64,
}

/// Secondary Index 데이터 구조
/// column_value -> Set<partition_key>
#[derive(Debug)]
pub struct SecondaryIndex {
    pub definition: IndexDefinition,
    /// 인덱스 데이터: value -> partition keys
    data: RwLock<BTreeMap<CassandraValue, HashSet<PartitionKey>>>,
}

impl SecondaryIndex {
    pub fn new(definition: IndexDefinition) -> Self {
        Self {
            definition,
            data: RwLock::new(BTreeMap::new()),
        }
    }
    
    /// 인덱스에 엔트리 추가
    pub fn insert(&self, value: CassandraValue, partition_key: PartitionKey) {
        let mut data = self.data.write().unwrap();
        data.entry(value)
            .or_insert_with(HashSet::new)
            .insert(partition_key);
    }
    
    /// 인덱스에서 엔트리 삭제
    pub fn remove(&self, value: &CassandraValue, partition_key: &PartitionKey) {
        let mut data = self.data.write().unwrap();
        if let Some(keys) = data.get_mut(value) {
            keys.remove(partition_key);
            if keys.is_empty() {
                data.remove(value);
            }
        }
    }
    
    /// 값으로 파티션 키들 조회
    pub fn lookup(&self, value: &CassandraValue) -> Vec<PartitionKey> {
        let data = self.data.read().unwrap();
        data.get(value)
            .map(|keys| keys.iter().cloned().collect())
            .unwrap_or_default()
    }
    
    /// Range 쿼리 지원 (>=, <=, >, <)
    pub fn range_lookup(
        &self,
        start: Option<&CassandraValue>,
        end: Option<&CassandraValue>,
        include_start: bool,
        include_end: bool,
    ) -> Vec<PartitionKey> {
        let data = self.data.read().unwrap();
        let mut result = HashSet::new();
        
        use std::ops::Bound;
        
        let start_bound = match (start, include_start) {
            (Some(v), true) => Bound::Included(v.clone()),
            (Some(v), false) => Bound::Excluded(v.clone()),
            (None, _) => Bound::Unbounded,
        };
        
        let end_bound = match (end, include_end) {
            (Some(v), true) => Bound::Included(v.clone()),
            (Some(v), false) => Bound::Excluded(v.clone()),
            (None, _) => Bound::Unbounded,
        };
        
        for (_, keys) in data.range((start_bound, end_bound)) {
            result.extend(keys.iter().cloned());
        }
        
        result.into_iter().collect()
    }
    
    /// 인덱스 엔트리 수
    pub fn len(&self) -> usize {
        let data = self.data.read().unwrap();
        data.values().map(|v| v.len()).sum()
    }
    
    /// 인덱스가 비어있는지
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    
    /// 인덱스 클리어
    pub fn clear(&self) {
        let mut data = self.data.write().unwrap();
        data.clear();
    }
}

/// Index Manager - 테이블별 인덱스 관리
#[derive(Debug, Default)]
pub struct IndexManager {
    /// (keyspace, table, column) -> SecondaryIndex
    indexes: RwLock<BTreeMap<(String, String, String), SecondaryIndex>>,
    /// (keyspace, index_name) -> (table, column)
    index_names: RwLock<BTreeMap<(String, String), (String, String)>>,
}

impl IndexManager {
    pub fn new() -> Self {
        Self::default()
    }
    
    /// 인덱스 생성
    pub fn create_index(&self, definition: IndexDefinition) -> Result<(), String> {
        let key = (
            definition.keyspace.clone(),
            definition.table.clone(),
            definition.column.clone(),
        );
        
        let mut indexes = self.indexes.write().unwrap();
        if indexes.contains_key(&key) {
            return Err(format!(
                "Index already exists on {}.{}.{}",
                definition.keyspace, definition.table, definition.column
            ));
        }
        
        let mut names = self.index_names.write().unwrap();
        let name_key = (definition.keyspace.clone(), definition.name.clone());
        if names.contains_key(&name_key) {
            return Err(format!("Index name {} already exists", definition.name));
        }
        
        names.insert(name_key, (definition.table.clone(), definition.column.clone()));
        indexes.insert(key, SecondaryIndex::new(definition));
        
        Ok(())
    }
    
    /// 인덱스 삭제
    pub fn drop_index(&self, keyspace: &str, index_name: &str) -> Result<(), String> {
        let mut names = self.index_names.write().unwrap();
        let name_key = (keyspace.to_string(), index_name.to_string());
        
        if let Some((table, column)) = names.remove(&name_key) {
            let mut indexes = self.indexes.write().unwrap();
            let key = (keyspace.to_string(), table, column);
            indexes.remove(&key);
            Ok(())
        } else {
            Err(format!("Index {} not found in keyspace {}", index_name, keyspace))
        }
    }
    
    /// 컬럼에 대한 인덱스 조회
    pub fn get_index(&self, keyspace: &str, table: &str, column: &str) -> Option<SecondaryIndex> {
        let indexes = self.indexes.read().unwrap();
        let key = (keyspace.to_string(), table.to_string(), column.to_string());
        indexes.get(&key).map(|idx| SecondaryIndex {
            definition: idx.definition.clone(),
            data: RwLock::new(idx.data.read().unwrap().clone()),
        })
    }
    
    /// 테이블의 모든 인덱스 조회
    pub fn get_table_indexes(&self, keyspace: &str, table: &str) -> Vec<IndexDefinition> {
        let indexes = self.indexes.read().unwrap();
        indexes
            .iter()
            .filter(|((ks, tbl, _), _)| ks == keyspace && tbl == table)
            .map(|(_, idx)| idx.definition.clone())
            .collect()
    }
    
    /// 인덱스에 값 추가
    pub fn insert_to_index(
        &self,
        keyspace: &str,
        table: &str,
        column: &str,
        value: CassandraValue,
        partition_key: PartitionKey,
    ) {
        let indexes = self.indexes.read().unwrap();
        let key = (keyspace.to_string(), table.to_string(), column.to_string());
        if let Some(idx) = indexes.get(&key) {
            idx.insert(value, partition_key);
        }
    }
    
    /// 인덱스에서 값 삭제
    pub fn remove_from_index(
        &self,
        keyspace: &str,
        table: &str,
        column: &str,
        value: &CassandraValue,
        partition_key: &PartitionKey,
    ) {
        let indexes = self.indexes.read().unwrap();
        let key = (keyspace.to_string(), table.to_string(), column.to_string());
        if let Some(idx) = indexes.get(&key) {
            idx.remove(value, partition_key);
        }
    }
    
    /// 인덱스로 파티션 키 조회
    pub fn lookup(
        &self,
        keyspace: &str,
        table: &str,
        column: &str,
        value: &CassandraValue,
    ) -> Option<Vec<PartitionKey>> {
        let indexes = self.indexes.read().unwrap();
        let key = (keyspace.to_string(), table.to_string(), column.to_string());
        indexes.get(&key).map(|idx| idx.lookup(value))
    }
    
    /// 인덱스가 존재하는지 확인
    pub fn has_index(&self, keyspace: &str, table: &str, column: &str) -> bool {
        let indexes = self.indexes.read().unwrap();
        let key = (keyspace.to_string(), table.to_string(), column.to_string());
        indexes.contains_key(&key)
    }
    
    /// 모든 인덱스 목록
    pub fn list_all_indexes(&self) -> Vec<IndexDefinition> {
        let indexes = self.indexes.read().unwrap();
        indexes.values().map(|idx| idx.definition.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_secondary_index_insert_lookup() {
        let def = IndexDefinition {
            name: "idx_email".to_string(),
            keyspace: "test".to_string(),
            table: "users".to_string(),
            column: "email".to_string(),
            created_at: 0,
        };
        
        let idx = SecondaryIndex::new(def);
        
        let pk1 = PartitionKey {
            components: vec![CassandraValue::Int(1)],
        };
        let pk2 = PartitionKey {
            components: vec![CassandraValue::Int(2)],
        };
        
        idx.insert(CassandraValue::Text("test@example.com".to_string()), pk1.clone());
        idx.insert(CassandraValue::Text("test@example.com".to_string()), pk2.clone());
        idx.insert(CassandraValue::Text("other@example.com".to_string()), pk1.clone());
        
        let results = idx.lookup(&CassandraValue::Text("test@example.com".to_string()));
        assert_eq!(results.len(), 2);
        
        let results = idx.lookup(&CassandraValue::Text("other@example.com".to_string()));
        assert_eq!(results.len(), 1);
        
        let results = idx.lookup(&CassandraValue::Text("notfound@example.com".to_string()));
        assert_eq!(results.len(), 0);
    }
    
    #[test]
    fn test_index_manager() {
        let manager = IndexManager::new();
        
        let def = IndexDefinition {
            name: "idx_email".to_string(),
            keyspace: "test".to_string(),
            table: "users".to_string(),
            column: "email".to_string(),
            created_at: 0,
        };
        
        assert!(manager.create_index(def.clone()).is_ok());
        assert!(manager.create_index(def).is_err()); // duplicate
        
        assert!(manager.has_index("test", "users", "email"));
        assert!(!manager.has_index("test", "users", "name"));
        
        let pk = PartitionKey {
            components: vec![CassandraValue::Int(1)],
        };
        
        manager.insert_to_index(
            "test", "users", "email",
            CassandraValue::Text("a@b.com".to_string()),
            pk.clone(),
        );
        
        let results = manager.lookup("test", "users", "email", &CassandraValue::Text("a@b.com".to_string()));
        assert!(results.is_some());
        assert_eq!(results.unwrap().len(), 1);
        
        assert!(manager.drop_index("test", "idx_email").is_ok());
        assert!(!manager.has_index("test", "users", "email"));
    }
    
    #[test]
    fn test_range_lookup() {
        let def = IndexDefinition {
            name: "idx_age".to_string(),
            keyspace: "test".to_string(),
            table: "users".to_string(),
            column: "age".to_string(),
            created_at: 0,
        };
        
        let idx = SecondaryIndex::new(def);
        
        for i in 1..=10 {
            let pk = PartitionKey {
                components: vec![CassandraValue::Int(i)],
            };
            idx.insert(CassandraValue::Int(i * 10), pk);
        }
        
        // age >= 50 AND age <= 80
        let results = idx.range_lookup(
            Some(&CassandraValue::Int(50)),
            Some(&CassandraValue::Int(80)),
            true,
            true,
        );
        assert_eq!(results.len(), 4); // 50, 60, 70, 80
        
        // age > 50 AND age < 80
        let results = idx.range_lookup(
            Some(&CassandraValue::Int(50)),
            Some(&CassandraValue::Int(80)),
            false,
            false,
        );
        assert_eq!(results.len(), 2); // 60, 70
    }
}
