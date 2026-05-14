//! 요청 처리 핸들러

use std::collections::HashMap;
use std::sync::Arc;
use bytes::{Bytes, BytesMut};
use crate::database::CoreDB;
use crate::protocol::codec::*;
use crate::protocol::types::*;
use crate::schema::CassandraValue;
use crate::query::QueryResult;

/// 요청 핸들러
pub struct RequestHandler {
    db: Arc<CoreDB>,
    prepared_statements: HashMap<Bytes, String>,
}

impl RequestHandler {
    pub fn new(db: Arc<CoreDB>) -> Self {
        Self {
            db,
            prepared_statements: HashMap::new(),
        }
    }
    
    /// 요청 처리
    pub async fn handle(&mut self, request: Request) -> Response {
        match request {
            Request::Startup(startup) => self.handle_startup(startup),
            Request::Options => self.handle_options(),
            Request::Query(query) => self.handle_query(query).await,
            Request::Prepare(prepare) => self.handle_prepare(prepare),
            Request::Execute(execute) => self.handle_execute(execute).await,
            Request::Batch(batch) => self.handle_batch(batch).await,
            Request::Register(_) => Response::Ready, // 이벤트 등록은 현재 무시
            Request::AuthResponse(_) => Response::AuthSuccess(None),
        }
    }
    
    fn handle_startup(&self, _startup: StartupRequest) -> Response {
        // 인증 없이 바로 Ready 응답
        Response::Ready
    }
    
    fn handle_options(&self) -> Response {
        let mut options = HashMap::new();
        options.insert("CQL_VERSION".to_string(), vec!["3.4.5".to_string()]);
        options.insert("COMPRESSION".to_string(), vec![]); // 압축 미지원
        Response::Supported(options)
    }
    
    async fn handle_query(&self, query: QueryRequest) -> Response {
        match self.db.execute_cql(&query.query).await {
            Ok(result) => self.convert_result(result),
            Err(e) => Response::Error(ErrorResponse::syntax_error(e.to_string())),
        }
    }
    
    fn handle_prepare(&mut self, prepare: PrepareRequest) -> Response {
        // 간단한 prepared statement 구현
        // 실제로는 쿼리를 파싱하고 ID를 생성해야 함
        let id = Bytes::from(format!("{:016x}", self.prepared_statements.len()));
        self.prepared_statements.insert(id.clone(), prepare.query);
        
        Response::Result(ResultResponse::Prepared(PreparedResult {
            id,
            metadata: RowsMetadata {
                flags: 0,
                columns_count: 0,
                paging_state: None,
                keyspace: None,
                table: None,
                columns: vec![],
            },
            result_metadata: RowsMetadata {
                flags: 0,
                columns_count: 0,
                paging_state: None,
                keyspace: None,
                table: None,
                columns: vec![],
            },
        }))
    }
    
    async fn handle_execute(&self, execute: ExecuteRequest) -> Response {
        // Prepared statement 실행
        if let Some(query_template) = self.prepared_statements.get(&execute.id) {
            // 바인드 변수 처리
            let bound_query = self.bind_values(query_template, &execute.values);
            
            match self.db.execute_cql(&bound_query).await {
                Ok(result) => self.convert_result(result),
                Err(e) => Response::Error(ErrorResponse::syntax_error(e.to_string())),
            }
        } else {
            Response::Error(ErrorResponse::new(
                crate::protocol::frame::ErrorCode::Unprepared,
                "Prepared statement not found",
            ))
        }
    }
    
    /// 바인드 변수를 쿼리에 적용
    fn bind_values(&self, query: &str, values: &[Option<Bytes>]) -> String {
        let mut result = query.to_string();
        
        for value in values {
            if let Some(pos) = result.find('?') {
                let replacement = match value {
                    Some(bytes) => {
                        // 바이트를 값으로 변환
                        self.bytes_to_cql_literal(bytes)
                    },
                    None => "NULL".to_string(),
                };
                result.replace_range(pos..pos+1, &replacement);
            }
        }
        
        result
    }
    
    /// 바이트를 CQL 리터럴로 변환
    fn bytes_to_cql_literal(&self, bytes: &Bytes) -> String {
        // Native Protocol에서 값은 타입에 따라 인코딩됨
        // 간단한 구현: 문자열로 시도, 실패 시 정수로 시도
        if bytes.is_empty() {
            return "NULL".to_string();
        }
        
        // 문자열로 시도
        if let Ok(s) = std::str::from_utf8(bytes) {
            // 숫자인지 확인
            if s.parse::<i64>().is_ok() || s.parse::<f64>().is_ok() {
                return s.to_string();
            }
            // 문자열은 따옴표로 감싸기
            return format!("'{}'", s.replace('\'', "''"));
        }
        
        // 4바이트 정수
        if bytes.len() == 4 {
            let val = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            return val.to_string();
        }
        
        // 8바이트 정수 (bigint)
        if bytes.len() == 8 {
            let val = i64::from_be_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7]
            ]);
            return val.to_string();
        }
        
        // 기타: hex로 반환
        let hex_str: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
        format!("0x{}", hex_str)
    }
    
    async fn handle_batch(&self, batch: BatchRequest) -> Response {
        // BATCH 실행
        for query in batch.queries {
            match query {
                BatchQuery::Simple { query, .. } => {
                    if let Err(e) = self.db.execute_cql(&query).await {
                        return Response::Error(ErrorResponse::syntax_error(e.to_string()));
                    }
                },
                BatchQuery::Prepared { id, .. } => {
                    if let Some(query) = self.prepared_statements.get(&id) {
                        if let Err(e) = self.db.execute_cql(query).await {
                            return Response::Error(ErrorResponse::syntax_error(e.to_string()));
                        }
                    }
                },
            }
        }
        
        Response::Result(ResultResponse::Void)
    }
    
    fn convert_result(&self, result: QueryResult) -> Response {
        match result {
            QueryResult::Success => Response::Result(ResultResponse::Void),
            QueryResult::Error(msg) => Response::Error(ErrorResponse::invalid(msg)),
            QueryResult::Schema(_) => Response::Result(ResultResponse::Void),
            QueryResult::Rows(rows) => {
                // Build column specs by walking the values once per column.
                // The previous version hard-coded col_type = Varchar for every
                // column, which the scylla driver then attempted to decode as
                // UTF-8 text — failing on any DOUBLE / INT / TIMESTAMP / UUID
                // column whose binary payload happens not to be valid UTF-8.
                // For each column we find the first non-NULL value across the
                // rowset and derive the matching CqlType. All-NULL columns
                // (or empty rowsets) fall back to Varchar, which is the
                // historical behaviour and the safest default for a typeless
                // payload (empty bytes decode cleanly as the empty string).
                let columns: Vec<ColumnSpec> = if let Some(first_row) = rows.first() {
                    first_row.columns.keys().map(|name| {
                        let col_type = rows
                            .iter()
                            .find_map(|r| r.columns.get(name).and_then(cql_type_for_value))
                            .unwrap_or(CqlType::Varchar);
                        ColumnSpec {
                            keyspace: None,
                            table: None,
                            name: name.clone(),
                            col_type,
                        }
                    }).collect()
                } else {
                    vec![]
                };
                
                let columns_count = columns.len() as i32;
                
                // 행 데이터 변환
                let result_rows: Vec<Vec<Option<Bytes>>> = rows.iter().map(|row| {
                    columns.iter().map(|col| {
                        row.columns.get(&col.name).map(|value| {
                            encode_cassandra_value(value)
                        })
                    }).collect()
                }).collect();
                
                Response::Result(ResultResponse::Rows(RowsResult {
                    metadata: RowsMetadata {
                        flags: 0x0001, // Global_tables_spec
                        columns_count,
                        paging_state: None,
                        keyspace: Some("system".to_string()),
                        table: Some("result".to_string()),
                        columns,
                    },
                    rows: result_rows,
                }))
            },
        }
    }
}

/// Map a non-NULL [`CassandraValue`] to the [`CqlType`] code that the
/// scylla / Cassandra client expects in `RESULT/Rows` metadata.
///
/// `None` for [`CassandraValue::Null`] so callers can skip past null
/// cells when inferring a column's type from sample rows.
///
/// Collection variants (List / Set / Map / UDT) are currently serialized
/// as their Rust `Debug` form — keep them Varchar so the wire-level
/// payload (UTF-8 text) is consistent with what
/// [`encode_cassandra_value`] writes for those variants. Once those get
/// proper structural encoders, switch the mapping to the matching
/// CqlType::{List, Set, Map, Udt} codes.
fn cql_type_for_value(value: &CassandraValue) -> Option<CqlType> {
    Some(match value {
        CassandraValue::Text(_) => CqlType::Varchar,
        CassandraValue::Int(_) => CqlType::Int,
        CassandraValue::BigInt(_) => CqlType::Bigint,
        CassandraValue::Double(_) => CqlType::Double,
        CassandraValue::Boolean(_) => CqlType::Boolean,
        CassandraValue::UUID(_) => CqlType::Uuid,
        CassandraValue::Timestamp(_) => CqlType::Timestamp,
        CassandraValue::Blob(_) => CqlType::Blob,
        CassandraValue::Counter(_) => CqlType::Counter,
        CassandraValue::List(_)
        | CassandraValue::Set(_)
        | CassandraValue::Map(_)
        | CassandraValue::UDT(_) => CqlType::Varchar,
        CassandraValue::Null => return None,
    })
}

fn encode_cassandra_value(value: &CassandraValue) -> Bytes {
    let mut buf = BytesMut::new();
    
    match value {
        CassandraValue::Null => {},
        CassandraValue::Int(v) => buf.extend_from_slice(&v.to_be_bytes()),
        CassandraValue::BigInt(v) => buf.extend_from_slice(&v.to_be_bytes()),
        CassandraValue::Double(v) => buf.extend_from_slice(&v.to_be_bytes()),
        CassandraValue::Boolean(v) => buf.extend_from_slice(&[if *v { 1 } else { 0 }]),
        CassandraValue::Text(s) => buf.extend_from_slice(s.as_bytes()),
        CassandraValue::Blob(b) => buf.extend_from_slice(b),
        CassandraValue::UUID(u) => buf.extend_from_slice(u.as_bytes()),
        CassandraValue::Timestamp(ts) => buf.extend_from_slice(&ts.to_be_bytes()),
        CassandraValue::List(_) | CassandraValue::Set(_) | CassandraValue::Map(_) => {
            // 컬렉션 타입은 간단하게 문자열로 변환
            buf.extend_from_slice(format!("{:?}", value).as_bytes());
        },
        CassandraValue::Counter(c) => buf.extend_from_slice(&c.to_be_bytes()),
        CassandraValue::UDT(fields) => {
            // UDT는 Map과 유사하게 문자열로 변환
            buf.extend_from_slice(format!("{:?}", fields).as_bytes());
        },
    }
    
    buf.freeze()
}
