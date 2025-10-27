use crate::schema::{CassandraValue, CassandraDataType, ColumnDefinition};
use crate::error::*;

/// CQL 문 타입
#[derive(Debug, Clone)]
pub enum CqlStatement {
    CreateKeyspace {
        name: String,
        options: KeyspaceOptions,
    },
    CreateTable {
        keyspace: String,
        name: String,
        columns: Vec<ColumnDefinition>,
        partition_key: Vec<String>,
        clustering_key: Vec<String>,
        options: TableOptions,
    },
    Insert {
        keyspace: String,
        table: String,
        values: Vec<(String, CassandraValue)>,
    },
    Select {
        keyspace: String,
        table: String,
        columns: Vec<String>,
        where_clause: Option<WhereClause>,
        limit: Option<u32>,
    },
    Update {
        keyspace: String,
        table: String,
        values: Vec<(String, CassandraValue)>,
        where_clause: WhereClause,
    },
    Delete {
        keyspace: String,
        table: String,
        where_clause: WhereClause,
    },
    DropTable {
        keyspace: String,
        name: String,
    },
    DropKeyspace {
        name: String,
    },
    Use {
        keyspace: String,
    },
}

/// 키스페이스 옵션
#[derive(Debug, Clone)]
pub struct KeyspaceOptions {
    pub replication_factor: u32,
    pub strategy: String,
}

/// 테이블 옵션
#[derive(Debug, Clone)]
pub struct TableOptions {
    pub compaction_strategy: String,
    pub bloom_filter_fp_chance: f64,
    pub default_time_to_live: Option<u32>,
}

/// WHERE 절 조건
#[derive(Debug, Clone)]
pub struct WhereClause {
    pub conditions: Vec<Condition>,
}

/// WHERE 조건
#[derive(Debug, Clone)]
pub struct Condition {
    pub column: String,
    pub operator: ComparisonOperator,
    pub value: CassandraValue,
}

/// 비교 연산자
#[derive(Debug, Clone)]
pub enum ComparisonOperator {
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    In,
    Like,
}

/// 간단한 CQL 파서 (실제 구현에서는 더 정교한 파서가 필요)
pub struct CqlParser;

impl CqlParser {
    pub fn parse(query: &str) -> Result<CqlStatement> {
        let query = query.trim();
        
        if query.to_uppercase().starts_with("CREATE KEYSPACE") {
            Self::parse_create_keyspace(query)
        } else if query.to_uppercase().starts_with("CREATE TABLE") {
            Self::parse_create_table(query)
        } else if query.to_uppercase().starts_with("INSERT") {
            Self::parse_insert(query)
        } else if query.to_uppercase().starts_with("SELECT") {
            Self::parse_select(query)
        } else if query.to_uppercase().starts_with("UPDATE") {
            Self::parse_update(query)
        } else if query.to_uppercase().starts_with("DELETE") {
            Self::parse_delete(query)
        } else if query.to_uppercase().starts_with("DROP TABLE") {
            Self::parse_drop_table(query)
        } else if query.to_uppercase().starts_with("DROP KEYSPACE") {
            Self::parse_drop_keyspace(query)
        } else if query.to_uppercase().starts_with("USE") {
            Self::parse_use(query)
        } else {
            Err(CoreDBError::QueryParsingError {
                message: format!("Unsupported query type: {}", query),
            })
        }
    }
    
    fn parse_create_keyspace(query: &str) -> Result<CqlStatement> {
        // 간단한 파싱 - 실제로는 더 정교한 파서가 필요
        let re = regex::Regex::new(r"CREATE\s+KEYSPACE\s+(\w+)\s+WITH\s+REPLICATION\s*=\s*\{.*'replication_factor'\s*:\s*(\d+).*\}")?;
        
        if let Some(caps) = re.captures(query) {
            let name = caps.get(1).unwrap().as_str().to_string();
            let replication_factor = caps.get(2).unwrap().as_str().parse::<u32>()?;
            
            Ok(CqlStatement::CreateKeyspace {
                name,
                options: KeyspaceOptions {
                    replication_factor,
                    strategy: "SimpleStrategy".to_string(),
                },
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid CREATE KEYSPACE syntax".to_string(),
            })
        }
    }
    
    fn parse_create_table(query: &str) -> Result<CqlStatement> {
        // 매우 간단한 파싱 - 실제로는 더 정교한 파서가 필요
        let re = regex::Regex::new(r"CREATE\s+TABLE\s+(\w+)\.(\w+)\s*\((.*)\)")?;
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let name = caps.get(2).unwrap().as_str().to_string();
            let columns_str = caps.get(3).unwrap().as_str();
            
            // 컬럼 파싱 (매우 간단한 버전)
            let mut columns = Vec::new();
            let mut partition_key = Vec::new();
            let mut clustering_key = Vec::new();
            
            for column_def in columns_str.split(',') {
                let parts: Vec<&str> = column_def.trim().split_whitespace().collect();
                if parts.len() >= 2 {
                    let column_name = parts[0].to_string();
                    let data_type = Self::parse_data_type(parts[1])?;
                    
                    let is_static = parts.contains(&"STATIC");
                    let is_partition_key = parts.contains(&"PRIMARY") || parts.contains(&"KEY");
                    
                    columns.push(ColumnDefinition {
                        name: column_name.clone(),
                        data_type,
                        is_static,
                    });
                    
                    if is_partition_key {
                        partition_key.push(column_name);
                    }
                }
            }
            
            Ok(CqlStatement::CreateTable {
                keyspace,
                name,
                columns,
                partition_key,
                clustering_key,
                options: TableOptions {
                    compaction_strategy: "SizeTieredCompactionStrategy".to_string(),
                    bloom_filter_fp_chance: 0.01,
                    default_time_to_live: None,
                },
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid CREATE TABLE syntax".to_string(),
            })
        }
    }
    
    fn parse_insert(query: &str) -> Result<CqlStatement> {
        // 간단한 INSERT 파싱
        let re = regex::Regex::new(r"INSERT\s+INTO\s+(\w+)\.(\w+)\s*\(([^)]+)\)\s*VALUES\s*\(([^)]+)\)")?;
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let table = caps.get(2).unwrap().as_str().to_string();
            let columns_str = caps.get(3).unwrap().as_str();
            let values_str = caps.get(4).unwrap().as_str();
            
            let columns: Vec<&str> = columns_str.split(',').map(|s| s.trim()).collect();
            let values: Vec<&str> = values_str.split(',').map(|s| s.trim()).collect();
            
            if columns.len() != values.len() {
                return Err(CoreDBError::QueryParsingError {
                    message: "Column count doesn't match value count".to_string(),
                });
            }
            
            let mut value_pairs = Vec::new();
            for (column, value) in columns.iter().zip(values.iter()) {
                let parsed_value = Self::parse_value(value)?;
                value_pairs.push((column.to_string(), parsed_value));
            }
            
            Ok(CqlStatement::Insert {
                keyspace,
                table,
                values: value_pairs,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid INSERT syntax".to_string(),
            })
        }
    }
    
    fn parse_select(query: &str) -> Result<CqlStatement> {
        // 간단한 SELECT 파싱
        let re = regex::Regex::new(r"SELECT\s+(.+?)\s+FROM\s+(\w+)\.(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            let columns_str = caps.get(1).unwrap().as_str();
            let keyspace = caps.get(2).unwrap().as_str().to_string();
            let table = caps.get(3).unwrap().as_str().to_string();
            
            let columns = if columns_str == "*" {
                vec!["*".to_string()]
            } else {
                columns_str.split(',').map(|s| s.trim().to_string()).collect()
            };
            
            // WHERE 절 파싱 (간단한 버전)
            let where_clause = if query.to_uppercase().contains("WHERE") {
                Some(Self::parse_where_clause(query)?)
            } else {
                None
            };
            
            // LIMIT 파싱
            let limit = if let Some(limit_match) = regex::Regex::new(r"LIMIT\s+(\d+)")?.captures(query) {
                Some(limit_match.get(1).unwrap().as_str().parse::<u32>()?)
            } else {
                None
            };
            
            Ok(CqlStatement::Select {
                keyspace,
                table,
                columns,
                where_clause,
                limit,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid SELECT syntax".to_string(),
            })
        }
    }
    
    fn parse_update(query: &str) -> Result<CqlStatement> {
        // 간단한 UPDATE 파싱
        Err(CoreDBError::QueryParsingError {
            message: "UPDATE not implemented yet".to_string(),
        })
    }
    
    fn parse_delete(query: &str) -> Result<CqlStatement> {
        // 간단한 DELETE 파싱
        Err(CoreDBError::QueryParsingError {
            message: "DELETE not implemented yet".to_string(),
        })
    }
    
    fn parse_drop_table(query: &str) -> Result<CqlStatement> {
        let re = regex::Regex::new(r"DROP\s+TABLE\s+(\w+)\.(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            Ok(CqlStatement::DropTable {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                name: caps.get(2).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid DROP TABLE syntax".to_string(),
            })
        }
    }
    
    fn parse_drop_keyspace(query: &str) -> Result<CqlStatement> {
        let re = regex::Regex::new(r"DROP\s+KEYSPACE\s+(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            Ok(CqlStatement::DropKeyspace {
                name: caps.get(1).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid DROP KEYSPACE syntax".to_string(),
            })
        }
    }
    
    fn parse_use(query: &str) -> Result<CqlStatement> {
        let re = regex::Regex::new(r"USE\s+(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            Ok(CqlStatement::Use {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid USE syntax".to_string(),
            })
        }
    }
    
    fn parse_where_clause(query: &str) -> Result<WhereClause> {
        let re = regex::Regex::new(r"WHERE\s+(\w+)\s*=\s*([^\\s]+)")?;
        
        if let Some(caps) = re.captures(query) {
            let column = caps.get(1).unwrap().as_str().to_string();
            let value_str = caps.get(2).unwrap().as_str();
            let value = Self::parse_value(value_str)?;
            
            Ok(WhereClause {
                conditions: vec![Condition {
                    column,
                    operator: ComparisonOperator::Equal,
                    value,
                }],
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid WHERE clause syntax".to_string(),
            })
        }
    }
    
    fn parse_data_type(type_str: &str) -> Result<CassandraDataType> {
        match type_str.to_uppercase().as_str() {
            "TEXT" | "VARCHAR" => Ok(CassandraDataType::Text),
            "INT" => Ok(CassandraDataType::Int),
            "BIGINT" => Ok(CassandraDataType::BigInt),
            "UUID" => Ok(CassandraDataType::UUID),
            "TIMESTAMP" => Ok(CassandraDataType::Timestamp),
            "BOOLEAN" | "BOOL" => Ok(CassandraDataType::Boolean),
            "DOUBLE" | "FLOAT" => Ok(CassandraDataType::Double),
            "BLOB" => Ok(CassandraDataType::Blob),
            _ => Err(CoreDBError::QueryParsingError {
                message: format!("Unsupported data type: {}", type_str),
            }),
        }
    }
    
    fn parse_value(value_str: &str) -> Result<CassandraValue> {
        let value = value_str.trim();
        
        if value == "NULL" {
            Ok(CassandraValue::Null)
        } else if value.starts_with('\'') && value.ends_with('\'') {
            // 문자열
            let string_value = value[1..value.len()-1].to_string();
            Ok(CassandraValue::Text(string_value))
        } else if value.parse::<i32>().is_ok() {
            Ok(CassandraValue::Int(value.parse::<i32>()?))
        } else if value.parse::<i64>().is_ok() {
            Ok(CassandraValue::BigInt(value.parse::<i64>()?))
        } else if value.parse::<f64>().is_ok() {
            Ok(CassandraValue::Double(value.parse::<f64>()?))
        } else if value.to_lowercase() == "true" || value.to_lowercase() == "false" {
            Ok(CassandraValue::Boolean(value.parse::<bool>()?))
        } else if let Ok(uuid) = uuid::Uuid::parse_str(value) {
            Ok(CassandraValue::UUID(uuid))
        } else {
            // 기본적으로 문자열로 처리
            Ok(CassandraValue::Text(value.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_create_keyspace() {
        let query = "CREATE KEYSPACE test_ks WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}";
        let result = CqlParser::parse(query);
        assert!(result.is_ok());
        
        if let Ok(CqlStatement::CreateKeyspace { name, options }) = result {
            assert_eq!(name, "test_ks");
            assert_eq!(options.replication_factor, 1);
        }
    }
    
    #[test]
    fn test_parse_create_table() {
        let query = "CREATE TABLE test_ks.test_table (id INT PRIMARY KEY, name TEXT, age INT)";
        let result = CqlParser::parse(query);
        assert!(result.is_ok());
        
        if let Ok(CqlStatement::CreateTable { keyspace, name, columns, .. }) = result {
            assert_eq!(keyspace, "test_ks");
            assert_eq!(name, "test_table");
            assert_eq!(columns.len(), 3);
        }
    }
    
    #[test]
    fn test_parse_insert() {
        let query = "INSERT INTO test_ks.test_table (id, name, age) VALUES (1, 'John', 30)";
        let result = CqlParser::parse(query);
        assert!(result.is_ok());
        
        if let Ok(CqlStatement::Insert { keyspace, table, values, .. }) = result {
            assert_eq!(keyspace, "test_ks");
            assert_eq!(table, "test_table");
            assert_eq!(values.len(), 3);
        }
    }
    
    #[test]
    fn test_parse_select() {
        let query = "SELECT * FROM test_ks.test_table WHERE id = 1 LIMIT 10";
        let result = CqlParser::parse(query);
        assert!(result.is_ok());
        
        if let Ok(CqlStatement::Select { keyspace, table, columns, where_clause, limit }) = result {
            assert_eq!(keyspace, "test_ks");
            assert_eq!(table, "test_table");
            assert_eq!(columns, vec!["*"]);
            assert!(where_clause.is_some());
            assert_eq!(limit, Some(10));
        }
    }
}
