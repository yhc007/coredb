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
        ttl: Option<u32>,
        if_not_exists: bool,
    },
    Select {
        keyspace: String,
        table: String,
        columns: Vec<String>,
        where_clause: Option<WhereClause>,
        limit: Option<u32>,
        aggregations: Vec<Aggregation>,
        distinct: bool,
        allow_filtering: bool,
        order_by: Option<OrderBy>,
        group_by: Vec<String>,
        page_size: Option<u32>,
        paging_state: Option<String>,
    },
    Update {
        keyspace: String,
        table: String,
        values: Vec<(String, CassandraValue)>,
        where_clause: WhereClause,
        if_conditions: Option<Vec<Condition>>,
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
    CreateIndex {
        name: Option<String>,
        keyspace: String,
        table: String,
        column: String,
    },
    DropIndex {
        keyspace: String,
        name: String,
    },
    /// BATCH 작업
    Batch {
        statements: Vec<CqlStatement>,
    },
    /// TRUNCATE - 테이블 전체 삭제
    Truncate {
        keyspace: String,
        table: String,
    },
    /// ALTER TABLE
    AlterTable {
        keyspace: String,
        table: String,
        operation: AlterTableOperation,
    },
}

/// ALTER TABLE 연산
#[derive(Debug, Clone)]
pub enum AlterTableOperation {
    AddColumn { name: String, data_type: CassandraDataType },
    DropColumn { name: String },
    RenameColumn { old_name: String, new_name: String },
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

/// Aggregation 함수
#[derive(Debug, Clone)]
pub enum AggregationFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

/// Aggregation 정의
#[derive(Debug, Clone)]
pub struct Aggregation {
    pub func: AggregationFunc,
    pub column: String, // "*" for COUNT(*)
}

/// ORDER BY 정렬 방향
#[derive(Debug, Clone, PartialEq)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// ORDER BY 정의
#[derive(Debug, Clone)]
pub struct OrderBy {
    pub column: String,
    pub order: SortOrder,
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
        } else if query.to_uppercase().starts_with("CREATE INDEX") {
            Self::parse_create_index(query)
        } else if query.to_uppercase().starts_with("DROP INDEX") {
            Self::parse_drop_index(query)
        } else if query.to_uppercase().starts_with("BEGIN BATCH") {
            Self::parse_batch(query)
        } else if query.to_uppercase().starts_with("TRUNCATE") {
            Self::parse_truncate(query)
        } else if query.to_uppercase().starts_with("ALTER TABLE") {
            Self::parse_alter_table(query)
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
        let re = regex::Regex::new(r"(?i)CREATE\s+TABLE\s+(\w+)\.(\w+)\s*\(([\s\S]*)\)")?;
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let name = caps.get(2).unwrap().as_str().to_string();
            let columns_str = caps.get(3).unwrap().as_str();
            
            // 컬럼 파싱 (매우 간단한 버전)
            let mut columns = Vec::new();
            let mut partition_key = Vec::new();
            let clustering_key = Vec::new();
            
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
        // INSERT 파싱 (USING TTL + IF NOT EXISTS 지원)
        let re = regex::Regex::new(r"(?i)INSERT\s+INTO\s+(\w+)\.(\w+)\s*\(([^)]+)\)\s*VALUES\s*\((.+?)\)(?:\s+USING\s+TTL\s+(\d+))?(?:\s+IF\s+NOT\s+EXISTS)?\s*;?\s*$")?;
        
        // IF NOT EXISTS 체크
        let if_not_exists = query.to_uppercase().contains("IF NOT EXISTS");
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let table = caps.get(2).unwrap().as_str().to_string();
            let columns_str = caps.get(3).unwrap().as_str();
            let values_str = caps.get(4).unwrap().as_str();
            let ttl = caps.get(5).map(|m| m.as_str().parse::<u32>().unwrap_or(0));
            
            let columns: Vec<&str> = columns_str.split(',').map(|s| s.trim()).collect();
            let values = Self::split_values_respecting_quotes(values_str);
            
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
                ttl,
                if_not_exists,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid INSERT syntax".to_string(),
            })
        }
    }
    
    /// Split VALUES by comma, but respect quoted strings
    fn split_values_respecting_quotes(s: &str) -> Vec<String> {
        let mut values = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut chars = s.chars().peekable();
        
        while let Some(c) = chars.next() {
            match c {
                '\'' if !in_quotes => {
                    in_quotes = true;
                    current.push(c);
                }
                '\'' if in_quotes => {
                    current.push(c);
                    // Check for escaped quote ('')
                    if chars.peek() == Some(&'\'') {
                        current.push(chars.next().unwrap());
                    } else {
                        in_quotes = false;
                    }
                }
                ',' if !in_quotes => {
                    values.push(current.trim().to_string());
                    current = String::new();
                }
                _ => {
                    current.push(c);
                }
            }
        }
        
        // Don't forget the last value
        if !current.is_empty() {
            values.push(current.trim().to_string());
        }
        
        values
    }
    
    fn parse_select(query: &str) -> Result<CqlStatement> {
        // DISTINCT 체크
        let distinct = query.to_uppercase().contains("SELECT DISTINCT");
        
        // 간단한 SELECT 파싱
        let re = regex::Regex::new(r"(?i)SELECT\s+(?:DISTINCT\s+)?(.+?)\s+FROM\s+(\w+)\.(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            let columns_str = caps.get(1).unwrap().as_str();
            let keyspace = caps.get(2).unwrap().as_str().to_string();
            let table = caps.get(3).unwrap().as_str().to_string();
            
            // Aggregation 파싱
            let mut aggregations = Vec::new();
            let mut columns = Vec::new();
            
            let agg_re = regex::Regex::new(r"(?i)(COUNT|SUM|AVG|MIN|MAX)\s*\(\s*(\*|\w+)\s*\)")?;
            
            for part in columns_str.split(',') {
                let part = part.trim();
                if let Some(agg_caps) = agg_re.captures(part) {
                    let func_name = agg_caps.get(1).unwrap().as_str().to_uppercase();
                    let col = agg_caps.get(2).unwrap().as_str().to_string();
                    
                    let func = match func_name.as_str() {
                        "COUNT" => AggregationFunc::Count,
                        "SUM" => AggregationFunc::Sum,
                        "AVG" => AggregationFunc::Avg,
                        "MIN" => AggregationFunc::Min,
                        "MAX" => AggregationFunc::Max,
                        _ => continue,
                    };
                    
                    aggregations.push(Aggregation { func, column: col });
                } else if part == "*" {
                    columns.push("*".to_string());
                } else {
                    columns.push(part.to_string());
                }
            }
            
            // WHERE 절 파싱 (간단한 버전)
            let where_clause = if query.to_uppercase().contains("WHERE") {
                Some(Self::parse_where_clause(query)?)
            } else {
                None
            };
            
            // GROUP BY 파싱
            let group_by_re = regex::Regex::new(r"(?i)GROUP\s+BY\s+([\w\s,]+?)(?:\s+ORDER|\s+LIMIT|\s+ALLOW|\s*$)")?;
            let group_by = if let Some(group_caps) = group_by_re.captures(query) {
                group_caps.get(1).unwrap().as_str()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                vec![]
            };
            
            // ORDER BY 파싱
            let order_by_re = regex::Regex::new(r"(?i)ORDER\s+BY\s+(\w+)(?:\s+(ASC|DESC))?")?;
            let order_by = if let Some(order_caps) = order_by_re.captures(query) {
                let column = order_caps.get(1).unwrap().as_str().to_string();
                let order = match order_caps.get(2).map(|m| m.as_str().to_uppercase()).as_deref() {
                    Some("DESC") => SortOrder::Desc,
                    _ => SortOrder::Asc,
                };
                Some(OrderBy { column, order })
            } else {
                None
            };
            
            // LIMIT 파싱
            let limit = if let Some(limit_match) = regex::Regex::new(r"LIMIT\s+(\d+)")?.captures(query) {
                Some(limit_match.get(1).unwrap().as_str().parse::<u32>()?)
            } else {
                None
            };
            
            // ALLOW FILTERING 체크
            let allow_filtering = query.to_uppercase().contains("ALLOW FILTERING");
            
            Ok(CqlStatement::Select {
                keyspace,
                table,
                columns,
                where_clause,
                limit,
                aggregations,
                distinct,
                allow_filtering,
                order_by,
                group_by,
                page_size: None, // Native Protocol에서 설정
                paging_state: None,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid SELECT syntax".to_string(),
            })
        }
    }
    
    fn parse_update(query: &str) -> Result<CqlStatement> {
        // UPDATE keyspace.table SET col1 = val1, col2 = val2 WHERE pk = x [IF condition]
        let re = regex::Regex::new(r"(?i)UPDATE\s+(\w+)\.(\w+)\s+SET\s+(.+?)\s+WHERE\s+(.+?)(?:\s+IF\s+(.+?))?\s*;?\s*$")?;
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let table = caps.get(2).unwrap().as_str().to_string();
            let set_str = caps.get(3).unwrap().as_str();
            let where_str = caps.get(4).unwrap().as_str();
            let if_str = caps.get(5).map(|m| m.as_str());
            
            // SET 파싱
            let mut values = Vec::new();
            for set_part in set_str.split(',') {
                let parts: Vec<&str> = set_part.split('=').collect();
                if parts.len() == 2 {
                    let col = parts[0].trim().to_string();
                    let val = Self::parse_value(parts[1].trim())?;
                    values.push((col, val));
                }
            }
            
            // WHERE 파싱
            let where_clause = Self::parse_where_from_str(where_str)?;
            
            // IF 조건 파싱
            let if_conditions = if let Some(if_part) = if_str {
                Some(Self::parse_conditions_from_str(if_part)?)
            } else {
                None
            };
            
            Ok(CqlStatement::Update {
                keyspace,
                table,
                values,
                where_clause,
                if_conditions,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid UPDATE syntax. Expected: UPDATE ks.table SET col=val WHERE pk=x [IF cond]".to_string(),
            })
        }
    }
    
    fn parse_delete(query: &str) -> Result<CqlStatement> {
        // DELETE FROM keyspace.table WHERE pk = x
        let re = regex::Regex::new(r"(?i)DELETE\s+FROM\s+(\w+)\.(\w+)\s+WHERE\s+(.+?)\s*;?\s*$")?;
        
        if let Some(caps) = re.captures(query) {
            let keyspace = caps.get(1).unwrap().as_str().to_string();
            let table = caps.get(2).unwrap().as_str().to_string();
            let where_str = caps.get(3).unwrap().as_str();
            
            let where_clause = Self::parse_where_from_str(where_str)?;
            
            Ok(CqlStatement::Delete {
                keyspace,
                table,
                where_clause,
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid DELETE syntax. Expected: DELETE FROM ks.table WHERE pk=x".to_string(),
            })
        }
    }
    
    fn parse_batch(query: &str) -> Result<CqlStatement> {
        // BEGIN BATCH ... APPLY BATCH
        let re = regex::Regex::new(r"(?is)BEGIN\s+BATCH\s*;?\s*(.+?)\s*APPLY\s+BATCH\s*;?\s*$")?;
        
        if let Some(caps) = re.captures(query) {
            let statements_str = caps.get(1).unwrap().as_str();
            let mut statements = Vec::new();
            
            // 세미콜론으로 분리하여 각 문장 파싱
            for stmt_str in statements_str.split(';') {
                let stmt_str = stmt_str.trim();
                if stmt_str.is_empty() {
                    continue;
                }
                
                // INSERT 또는 UPDATE만 허용
                if stmt_str.to_uppercase().starts_with("INSERT") {
                    statements.push(Self::parse_insert(stmt_str)?);
                } else if stmt_str.to_uppercase().starts_with("UPDATE") {
                    statements.push(Self::parse_update(stmt_str)?);
                } else if stmt_str.to_uppercase().starts_with("DELETE") {
                    statements.push(Self::parse_delete(stmt_str)?);
                }
            }
            
            if statements.is_empty() {
                return Err(CoreDBError::QueryParsingError {
                    message: "BATCH must contain at least one statement".to_string(),
                });
            }
            
            Ok(CqlStatement::Batch { statements })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid BATCH syntax. Expected: BEGIN BATCH ... APPLY BATCH".to_string(),
            })
        }
    }
    
    /// WHERE 문자열에서 WhereClause 파싱
    fn parse_where_from_str(where_str: &str) -> Result<WhereClause> {
        let conditions = Self::parse_conditions_from_str(where_str)?;
        Ok(WhereClause { conditions })
    }
    
    /// 조건 문자열 파싱
    fn parse_conditions_from_str(cond_str: &str) -> Result<Vec<Condition>> {
        let mut conditions = Vec::new();
        
        // AND로 분리
        let and_re = regex::Regex::new(r"(?i)\s+AND\s+")?;
        for part in and_re.split(cond_str) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            
            // IN 절 체크 (col IN (val1, val2, ...))
            let in_re = regex::Regex::new(r"(?i)(\w+)\s+IN\s*\(([^)]+)\)")?;
            if let Some(caps) = in_re.captures(part) {
                let column = caps.get(1).unwrap().as_str().to_string();
                let values_str = caps.get(2).unwrap().as_str();
                
                // 값들 파싱
                let values: Vec<CassandraValue> = values_str
                    .split(',')
                    .map(|v| Self::parse_value(v.trim()))
                    .collect::<Result<Vec<_>>>()?;
                
                conditions.push(Condition {
                    column,
                    operator: ComparisonOperator::In,
                    value: CassandraValue::List(values),
                });
                continue;
            }
            
            // 연산자 매칭
            let operators = [
                (">=", ComparisonOperator::GreaterThanOrEqual),
                ("<=", ComparisonOperator::LessThanOrEqual),
                ("!=", ComparisonOperator::NotEqual),
                ("<>", ComparisonOperator::NotEqual),
                (">", ComparisonOperator::GreaterThan),
                ("<", ComparisonOperator::LessThan),
                ("=", ComparisonOperator::Equal),
            ];
            
            let mut matched = false;
            for (op_str, op) in operators {
                if let Some(idx) = part.find(op_str) {
                    let column = part[..idx].trim().to_string();
                    let value_str = part[idx + op_str.len()..].trim();
                    let value = Self::parse_value(value_str)?;
                    
                    conditions.push(Condition { column, operator: op, value });
                    matched = true;
                    break;
                }
            }
            
            if !matched {
                return Err(CoreDBError::QueryParsingError {
                    message: format!("Invalid condition: {}", part),
                });
            }
        }
        
        Ok(conditions)
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
    
    fn parse_truncate(query: &str) -> Result<CqlStatement> {
        // TRUNCATE [TABLE] keyspace.table
        let re = regex::Regex::new(r"(?i)TRUNCATE\s+(?:TABLE\s+)?(\w+)\.(\w+)\s*;?\s*$")?;
        
        if let Some(caps) = re.captures(query) {
            Ok(CqlStatement::Truncate {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                table: caps.get(2).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid TRUNCATE syntax. Expected: TRUNCATE [TABLE] keyspace.table".to_string(),
            })
        }
    }
    
    fn parse_alter_table(query: &str) -> Result<CqlStatement> {
        // ALTER TABLE keyspace.table ADD column type
        let add_re = regex::Regex::new(r"(?i)ALTER\s+TABLE\s+(\w+)\.(\w+)\s+ADD\s+(\w+)\s+(\w+(?:<[^>]+>)?)\s*;?\s*$")?;
        if let Some(caps) = add_re.captures(query) {
            let data_type = Self::parse_data_type(caps.get(4).unwrap().as_str())?;
            return Ok(CqlStatement::AlterTable {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                table: caps.get(2).unwrap().as_str().to_string(),
                operation: AlterTableOperation::AddColumn {
                    name: caps.get(3).unwrap().as_str().to_string(),
                    data_type,
                },
            });
        }
        
        // ALTER TABLE keyspace.table DROP column
        let drop_re = regex::Regex::new(r"(?i)ALTER\s+TABLE\s+(\w+)\.(\w+)\s+DROP\s+(\w+)\s*;?\s*$")?;
        if let Some(caps) = drop_re.captures(query) {
            return Ok(CqlStatement::AlterTable {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                table: caps.get(2).unwrap().as_str().to_string(),
                operation: AlterTableOperation::DropColumn {
                    name: caps.get(3).unwrap().as_str().to_string(),
                },
            });
        }
        
        // ALTER TABLE keyspace.table RENAME old_col TO new_col
        let rename_re = regex::Regex::new(r"(?i)ALTER\s+TABLE\s+(\w+)\.(\w+)\s+RENAME\s+(\w+)\s+TO\s+(\w+)\s*;?\s*$")?;
        if let Some(caps) = rename_re.captures(query) {
            return Ok(CqlStatement::AlterTable {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                table: caps.get(2).unwrap().as_str().to_string(),
                operation: AlterTableOperation::RenameColumn {
                    old_name: caps.get(3).unwrap().as_str().to_string(),
                    new_name: caps.get(4).unwrap().as_str().to_string(),
                },
            });
        }
        
        Err(CoreDBError::QueryParsingError {
            message: "Invalid ALTER TABLE syntax. Expected: ALTER TABLE ks.tbl ADD/DROP/RENAME ...".to_string(),
        })
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
    
    fn parse_create_index(query: &str) -> Result<CqlStatement> {
        // CREATE INDEX [name] ON keyspace.table (column)
        // CREATE INDEX ON keyspace.table (column)
        let re_with_name = regex::Regex::new(
            r"(?i)CREATE\s+INDEX\s+(\w+)\s+ON\s+(\w+)\.(\w+)\s*\(\s*(\w+)\s*\)"
        )?;
        let re_without_name = regex::Regex::new(
            r"(?i)CREATE\s+INDEX\s+ON\s+(\w+)\.(\w+)\s*\(\s*(\w+)\s*\)"
        )?;
        
        if let Some(caps) = re_with_name.captures(query) {
            Ok(CqlStatement::CreateIndex {
                name: Some(caps.get(1).unwrap().as_str().to_string()),
                keyspace: caps.get(2).unwrap().as_str().to_string(),
                table: caps.get(3).unwrap().as_str().to_string(),
                column: caps.get(4).unwrap().as_str().to_string(),
            })
        } else if let Some(caps) = re_without_name.captures(query) {
            Ok(CqlStatement::CreateIndex {
                name: None,
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                table: caps.get(2).unwrap().as_str().to_string(),
                column: caps.get(3).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid CREATE INDEX syntax. Expected: CREATE INDEX [name] ON keyspace.table (column)".to_string(),
            })
        }
    }
    
    fn parse_drop_index(query: &str) -> Result<CqlStatement> {
        // DROP INDEX keyspace.index_name
        let re = regex::Regex::new(r"(?i)DROP\s+INDEX\s+(\w+)\.(\w+)")?;
        
        if let Some(caps) = re.captures(query) {
            Ok(CqlStatement::DropIndex {
                keyspace: caps.get(1).unwrap().as_str().to_string(),
                name: caps.get(2).unwrap().as_str().to_string(),
            })
        } else {
            Err(CoreDBError::QueryParsingError {
                message: "Invalid DROP INDEX syntax. Expected: DROP INDEX keyspace.index_name".to_string(),
            })
        }
    }
    
    fn parse_where_clause(query: &str) -> Result<WhereClause> {
        // WHERE 이후 부분 추출 (LIMIT, ORDER BY 등은 제외)
        let where_re = regex::Regex::new(r"(?i)WHERE\s+(.+?)(?:\s+LIMIT|\s+ORDER|\s+ALLOW|\s*$)")?;
        let where_part = if let Some(caps) = where_re.captures(query) {
            caps.get(1).unwrap().as_str().to_string()
        } else {
            // 간단한 fallback
            let idx = query.to_uppercase().find("WHERE").unwrap();
            query[idx + 5..].to_string()
        };
        
        let mut conditions = Vec::new();
        
        // AND로 분리된 조건들 파싱 (대소문자 무시)
        let and_re = regex::Regex::new(r"(?i)\s+AND\s+")?;
        for cond_str in and_re.split(&where_part) {
            let cond_str = cond_str.trim();
            if cond_str.is_empty() {
                continue;
            }
            
            // 연산자 매칭 (순서 중요: >= 먼저, > 나중에)
            let operators = [
                ("!=", ComparisonOperator::NotEqual),
                ("<>", ComparisonOperator::NotEqual),
                (">=", ComparisonOperator::GreaterThanOrEqual),
                ("<=", ComparisonOperator::LessThanOrEqual),
                (">", ComparisonOperator::GreaterThan),
                ("<", ComparisonOperator::LessThan),
                ("=", ComparisonOperator::Equal),
            ];
            
            let mut matched = false;
            for (op_str, op) in operators {
                if let Some(idx) = cond_str.find(op_str) {
                    let column = cond_str[..idx].trim().to_string();
                    let value_str = cond_str[idx + op_str.len()..].trim();
                    let value = Self::parse_value(value_str)?;
                    
                    conditions.push(Condition {
                        column,
                        operator: op,
                        value,
                    });
                    matched = true;
                    break;
                }
            }
            
            // LIKE 연산자
            if !matched {
                let like_re = regex::Regex::new(r"(?i)(\w+)\s+LIKE\s+(.+)")?;
                if let Some(caps) = like_re.captures(cond_str) {
                    let column = caps.get(1).unwrap().as_str().to_string();
                    let value_str = caps.get(2).unwrap().as_str().trim();
                    let value = Self::parse_value(value_str)?;
                    
                    conditions.push(Condition {
                        column,
                        operator: ComparisonOperator::Like,
                        value,
                    });
                    matched = true;
                }
            }
            
            // IN 연산자
            if !matched {
                let in_re = regex::Regex::new(r"(?i)(\w+)\s+IN\s*\((.+)\)")?;
                if let Some(caps) = in_re.captures(cond_str) {
                    let column = caps.get(1).unwrap().as_str().to_string();
                    let values_str = caps.get(2).unwrap().as_str();
                    let first_value = values_str.split(',').next().unwrap().trim();
                    let value = Self::parse_value(first_value)?;
                    
                    conditions.push(Condition {
                        column,
                        operator: ComparisonOperator::In,
                        value,
                    });
                    matched = true;
                }
            }
            
            if !matched {
                return Err(CoreDBError::QueryParsingError {
                    message: format!("Invalid WHERE condition: {}", cond_str),
                });
            }
        }
        
        if conditions.is_empty() {
            return Err(CoreDBError::QueryParsingError {
                message: "Empty WHERE clause".to_string(),
            });
        }
        
        Ok(WhereClause { conditions })
    }

    
    fn parse_data_type(type_str: &str) -> Result<CassandraDataType> {
        let type_upper = type_str.to_uppercase();
        let type_trimmed = type_upper.trim();
        
        // Collection 타입 체크
        // list<type>
        if let Some(inner) = type_trimmed.strip_prefix("LIST<").and_then(|s| s.strip_suffix(">")) {
            let inner_type = Self::parse_data_type(inner)?;
            return Ok(CassandraDataType::List(Box::new(inner_type)));
        }
        
        // set<type>
        if let Some(inner) = type_trimmed.strip_prefix("SET<").and_then(|s| s.strip_suffix(">")) {
            let inner_type = Self::parse_data_type(inner)?;
            return Ok(CassandraDataType::Set(Box::new(inner_type)));
        }
        
        // map<key_type, value_type>
        if let Some(inner) = type_trimmed.strip_prefix("MAP<").and_then(|s| s.strip_suffix(">")) {
            // 콤마로 분리 (첫 번째 콤마만)
            if let Some(comma_pos) = inner.find(',') {
                let key_type = Self::parse_data_type(&inner[..comma_pos].trim())?;
                let value_type = Self::parse_data_type(&inner[comma_pos+1..].trim())?;
                return Ok(CassandraDataType::Map(Box::new(key_type), Box::new(value_type)));
            }
        }
        
        // frozen<type> - 내부 타입 반환
        if let Some(inner) = type_trimmed.strip_prefix("FROZEN<").and_then(|s| s.strip_suffix(">")) {
            return Self::parse_data_type(inner);
        }
        
        match type_trimmed {
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
        } else if value.starts_with('[') && value.ends_with(']') {
            // List: [val1, val2, ...]
            let inner = &value[1..value.len()-1];
            if inner.is_empty() {
                return Ok(CassandraValue::List(vec![]));
            }
            let items: Vec<CassandraValue> = inner
                .split(',')
                .map(|v| Self::parse_value(v.trim()))
                .collect::<Result<Vec<_>>>()?;
            Ok(CassandraValue::List(items))
        } else if value.starts_with('{') && value.ends_with('}') {
            let inner = &value[1..value.len()-1];
            if inner.is_empty() {
                return Ok(CassandraValue::Set(vec![]));
            }
            // Map인지 Set인지 구분 (콜론 있으면 Map)
            if inner.contains(':') {
                // Map: {key1: val1, key2: val2}
                let mut map = std::collections::HashMap::new();
                for pair in inner.split(',') {
                    if let Some(colon_pos) = pair.find(':') {
                        let key = pair[..colon_pos].trim().trim_matches('\'').to_string();
                        let val = Self::parse_value(&pair[colon_pos+1..])?;
                        map.insert(key, val);
                    }
                }
                Ok(CassandraValue::Map(map))
            } else {
                // Set: {val1, val2, ...}
                let items: Vec<CassandraValue> = inner
                    .split(',')
                    .map(|v| Self::parse_value(v.trim()))
                    .collect::<Result<Vec<_>>>()?;
                Ok(CassandraValue::Set(items))
            }
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
        
        if let Ok(CqlStatement::Select { keyspace, table, columns, where_clause, limit, .. }) = result {
            assert_eq!(keyspace, "test_ks");
            assert_eq!(table, "test_table");
            assert_eq!(columns, vec!["*"]);
            assert!(where_clause.is_some());
            assert_eq!(limit, Some(10));
        }
    }
}
