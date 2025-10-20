use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use crate::schema::CassandraValue;

/// 쿼리 결과
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResult {
    Success,
    Rows(Vec<Row>),
    Schema(Vec<ColumnMetadata>),
    Error(String),
}

/// 행 데이터 (결과용)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub columns: HashMap<String, CassandraValue>,
}

/// 컬럼 메타데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMetadata {
    pub name: String,
    pub data_type: String,
    pub is_partition_key: bool,
    pub is_clustering_key: bool,
    pub is_static: bool,
}

impl QueryResult {
    pub fn success() -> Self {
        QueryResult::Success
    }
    
    pub fn error(message: String) -> Self {
        QueryResult::Error(message)
    }
    
    pub fn rows(rows: Vec<Row>) -> Self {
        QueryResult::Rows(rows)
    }
    
    pub fn schema(columns: Vec<ColumnMetadata>) -> Self {
        QueryResult::Schema(columns)
    }
    
    pub fn is_success(&self) -> bool {
        matches!(self, QueryResult::Success)
    }
    
    pub fn is_error(&self) -> bool {
        matches!(self, QueryResult::Error(_))
    }
}

impl Row {
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
        }
    }
    
    pub fn with_column(mut self, name: String, value: CassandraValue) -> Self {
        self.columns.insert(name, value);
        self
    }
    
    pub fn get_column(&self, name: &str) -> Option<&CassandraValue> {
        self.columns.get(name)
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::CassandraValue;
    
    #[test]
    fn test_query_result_success() {
        let result = QueryResult::success();
        assert!(result.is_success());
        assert!(!result.is_error());
    }
    
    #[test]
    fn test_query_result_error() {
        let result = QueryResult::error("Test error".to_string());
        assert!(!result.is_success());
        assert!(result.is_error());
    }
    
    #[test]
    fn test_row_creation() {
        let row = Row::new()
            .with_column("id".to_string(), CassandraValue::Int(42))
            .with_column("name".to_string(), CassandraValue::Text("test".to_string()));
        
        assert_eq!(row.get_column("id"), Some(&CassandraValue::Int(42)));
        assert_eq!(row.get_column("name"), Some(&CassandraValue::Text("test".to_string())));
        assert_eq!(row.get_column("missing"), None);
    }
}
