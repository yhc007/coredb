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
        if let Some(query) = self.prepared_statements.get(&execute.id) {
            // TODO: 바인드 변수 처리
            match self.db.execute_cql(query).await {
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
                // 컬럼 정보 추출
                let columns: Vec<ColumnSpec> = if let Some(first_row) = rows.first() {
                    first_row.columns.keys().map(|name| ColumnSpec {
                        keyspace: None,
                        table: None,
                        name: name.clone(),
                        col_type: CqlType::Varchar, // 기본값
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
    }
    
    buf.freeze()
}
