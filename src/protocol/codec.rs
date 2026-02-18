//! 요청/응답 코덱

use bytes::{Bytes, BytesMut, Buf, BufMut};
use std::io::{self, Cursor};
use std::collections::HashMap;
use crate::protocol::frame::{Opcode, ResultKind, ErrorCode};
use crate::protocol::types::*;
use crate::schema::CassandraValue;

/// 요청 메시지
#[derive(Debug, Clone)]
pub enum Request {
    Startup(StartupRequest),
    Options,
    Query(QueryRequest),
    Prepare(PrepareRequest),
    Execute(ExecuteRequest),
    Batch(BatchRequest),
    Register(RegisterRequest),
    AuthResponse(AuthResponseRequest),
}

/// STARTUP 요청
#[derive(Debug, Clone)]
pub struct StartupRequest {
    pub options: HashMap<String, String>,
}

impl StartupRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        let options = read_string_map(&mut cursor)?;
        Ok(Self { options })
    }
}

/// QUERY 요청
#[derive(Debug, Clone)]
pub struct QueryRequest {
    pub query: String,
    pub consistency: Consistency,
    pub flags: u8,
    pub values: Vec<Option<Bytes>>,
    pub page_size: Option<i32>,
    pub paging_state: Option<Bytes>,
    pub serial_consistency: Option<Consistency>,
    pub timestamp: Option<i64>,
}

impl QueryRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        
        let query = read_long_string(&mut cursor)?;
        let consistency = Consistency::try_from(read_short(&mut cursor)?)?;
        let flags = cursor.get_u8();
        
        let mut values = Vec::new();
        let mut page_size = None;
        let mut paging_state = None;
        let mut serial_consistency = None;
        let mut timestamp = None;
        
        // Flags:
        // 0x01 = values
        // 0x02 = skip_metadata
        // 0x04 = page_size
        // 0x08 = paging_state
        // 0x10 = serial_consistency
        // 0x20 = timestamp
        // 0x40 = names (values have names)
        
        if flags & 0x01 != 0 {
            let n = read_short(&mut cursor)? as usize;
            for _ in 0..n {
                values.push(read_bytes(&mut cursor)?);
            }
        }
        
        if flags & 0x04 != 0 {
            page_size = Some(read_int(&mut cursor)?);
        }
        
        if flags & 0x08 != 0 {
            paging_state = read_bytes(&mut cursor)?;
        }
        
        if flags & 0x10 != 0 {
            serial_consistency = Some(Consistency::try_from(read_short(&mut cursor)?)?);
        }
        
        if flags & 0x20 != 0 {
            timestamp = Some(read_long(&mut cursor)?);
        }
        
        Ok(Self {
            query,
            consistency,
            flags,
            values,
            page_size,
            paging_state,
            serial_consistency,
            timestamp,
        })
    }
}

/// PREPARE 요청
#[derive(Debug, Clone)]
pub struct PrepareRequest {
    pub query: String,
}

impl PrepareRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        let query = read_long_string(&mut cursor)?;
        Ok(Self { query })
    }
}

/// EXECUTE 요청
#[derive(Debug, Clone)]
pub struct ExecuteRequest {
    pub id: Bytes,
    pub consistency: Consistency,
    pub flags: u8,
    pub values: Vec<Option<Bytes>>,
}

impl ExecuteRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        
        let id_len = read_short(&mut cursor)? as usize;
        let pos = cursor.position() as usize;
        let id = Bytes::copy_from_slice(&buf[pos..pos + id_len]);
        cursor.advance(id_len);
        
        let consistency = Consistency::try_from(read_short(&mut cursor)?)?;
        let flags = cursor.get_u8();
        
        let mut values = Vec::new();
        if flags & 0x01 != 0 {
            let n = read_short(&mut cursor)? as usize;
            for _ in 0..n {
                values.push(read_bytes(&mut cursor)?);
            }
        }
        
        Ok(Self { id, consistency, flags, values })
    }
}

/// BATCH 요청
#[derive(Debug, Clone)]
pub struct BatchRequest {
    pub batch_type: u8,
    pub queries: Vec<BatchQuery>,
    pub consistency: Consistency,
}

#[derive(Debug, Clone)]
pub enum BatchQuery {
    Simple { query: String, values: Vec<Option<Bytes>> },
    Prepared { id: Bytes, values: Vec<Option<Bytes>> },
}

impl BatchRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        
        let batch_type = cursor.get_u8();
        let n = read_short(&mut cursor)? as usize;
        
        let mut queries = Vec::with_capacity(n);
        for _ in 0..n {
            let kind = cursor.get_u8();
            if kind == 0 {
                let query = read_long_string(&mut cursor)?;
                let values_count = read_short(&mut cursor)? as usize;
                let mut values = Vec::with_capacity(values_count);
                for _ in 0..values_count {
                    values.push(read_bytes(&mut cursor)?);
                }
                queries.push(BatchQuery::Simple { query, values });
            } else {
                let id_len = read_short(&mut cursor)? as usize;
                let pos = cursor.position() as usize;
                let id = Bytes::copy_from_slice(&cursor.get_ref()[pos..pos + id_len]);
                cursor.advance(id_len);
                
                let values_count = read_short(&mut cursor)? as usize;
                let mut values = Vec::with_capacity(values_count);
                for _ in 0..values_count {
                    values.push(read_bytes(&mut cursor)?);
                }
                queries.push(BatchQuery::Prepared { id, values });
            }
        }
        
        let consistency = Consistency::try_from(read_short(&mut cursor)?)?;
        
        Ok(Self { batch_type, queries, consistency })
    }
}

/// REGISTER 요청
#[derive(Debug, Clone)]
pub struct RegisterRequest {
    pub event_types: Vec<String>,
}

impl RegisterRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        let n = read_short(&mut cursor)? as usize;
        let mut event_types = Vec::with_capacity(n);
        for _ in 0..n {
            event_types.push(read_string(&mut cursor)?);
        }
        Ok(Self { event_types })
    }
}

/// AUTH_RESPONSE 요청
#[derive(Debug, Clone)]
pub struct AuthResponseRequest {
    pub token: Option<Bytes>,
}

impl AuthResponseRequest {
    pub fn decode(buf: &[u8]) -> io::Result<Self> {
        let mut cursor = Cursor::new(buf);
        let token = read_bytes(&mut cursor)?;
        Ok(Self { token })
    }
}

impl Request {
    pub fn decode(opcode: Opcode, body: &[u8]) -> io::Result<Self> {
        match opcode {
            Opcode::Startup => Ok(Request::Startup(StartupRequest::decode(body)?)),
            Opcode::Options => Ok(Request::Options),
            Opcode::Query => Ok(Request::Query(QueryRequest::decode(body)?)),
            Opcode::Prepare => Ok(Request::Prepare(PrepareRequest::decode(body)?)),
            Opcode::Execute => Ok(Request::Execute(ExecuteRequest::decode(body)?)),
            Opcode::Batch => Ok(Request::Batch(BatchRequest::decode(body)?)),
            Opcode::Register => Ok(Request::Register(RegisterRequest::decode(body)?)),
            Opcode::AuthResponse => Ok(Request::AuthResponse(AuthResponseRequest::decode(body)?)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unknown request opcode: {:?}", opcode))),
        }
    }
}

/// 응답 메시지
#[derive(Debug, Clone)]
pub enum Response {
    Ready,
    Supported(HashMap<String, Vec<String>>),
    Result(ResultResponse),
    Error(ErrorResponse),
    Authenticate(String),
    AuthSuccess(Option<Bytes>),
}

/// RESULT 응답
#[derive(Debug, Clone)]
pub enum ResultResponse {
    Void,
    Rows(RowsResult),
    SetKeyspace(String),
    Prepared(PreparedResult),
    SchemaChange(SchemaChangeResult),
}

/// Rows 결과
#[derive(Debug, Clone)]
pub struct RowsResult {
    pub metadata: RowsMetadata,
    pub rows: Vec<Vec<Option<Bytes>>>,
}

/// Rows 메타데이터
#[derive(Debug, Clone)]
pub struct RowsMetadata {
    pub flags: i32,
    pub columns_count: i32,
    pub paging_state: Option<Bytes>,
    pub keyspace: Option<String>,
    pub table: Option<String>,
    pub columns: Vec<ColumnSpec>,
}

/// 컬럼 명세
#[derive(Debug, Clone)]
pub struct ColumnSpec {
    pub keyspace: Option<String>,
    pub table: Option<String>,
    pub name: String,
    pub col_type: CqlType,
}

/// Prepared 결과
#[derive(Debug, Clone)]
pub struct PreparedResult {
    pub id: Bytes,
    pub metadata: RowsMetadata,
    pub result_metadata: RowsMetadata,
}

/// Schema Change 결과
#[derive(Debug, Clone)]
pub struct SchemaChangeResult {
    pub change_type: String,
    pub target: String,
    pub keyspace: String,
    pub name: Option<String>,
}

/// Error 응답
#[derive(Debug, Clone)]
pub struct ErrorResponse {
    pub code: i32,
    pub message: String,
}

impl Response {
    /// 응답을 바이트로 인코딩
    pub fn encode(&self) -> (Opcode, BytesMut) {
        let mut buf = BytesMut::new();
        
        let opcode = match self {
            Response::Ready => Opcode::Ready,
            
            Response::Supported(options) => {
                write_string_multimap(&mut buf, options);
                Opcode::Supported
            },
            
            Response::Result(result) => {
                match result {
                    ResultResponse::Void => {
                        write_int(&mut buf, ResultKind::Void as i32);
                    },
                    ResultResponse::Rows(rows) => {
                        write_int(&mut buf, ResultKind::Rows as i32);
                        encode_rows_metadata(&mut buf, &rows.metadata);
                        write_int(&mut buf, rows.rows.len() as i32);
                        for row in &rows.rows {
                            for cell in row {
                                write_bytes(&mut buf, cell.as_ref().map(|b| b.as_ref()));
                            }
                        }
                    },
                    ResultResponse::SetKeyspace(ks) => {
                        write_int(&mut buf, ResultKind::SetKeyspace as i32);
                        write_string(&mut buf, ks);
                    },
                    ResultResponse::Prepared(prepared) => {
                        write_int(&mut buf, ResultKind::Prepared as i32);
                        write_short(&mut buf, prepared.id.len() as u16);
                        buf.extend_from_slice(&prepared.id);
                        encode_rows_metadata(&mut buf, &prepared.metadata);
                        encode_rows_metadata(&mut buf, &prepared.result_metadata);
                    },
                    ResultResponse::SchemaChange(change) => {
                        write_int(&mut buf, ResultKind::SchemaChange as i32);
                        write_string(&mut buf, &change.change_type);
                        write_string(&mut buf, &change.target);
                        write_string(&mut buf, &change.keyspace);
                        if let Some(name) = &change.name {
                            write_string(&mut buf, name);
                        }
                    },
                }
                Opcode::Result
            },
            
            Response::Error(error) => {
                write_int(&mut buf, error.code);
                write_string(&mut buf, &error.message);
                Opcode::Error
            },
            
            Response::Authenticate(authenticator) => {
                write_string(&mut buf, authenticator);
                Opcode::Authenticate
            },
            
            Response::AuthSuccess(token) => {
                write_bytes(&mut buf, token.as_ref().map(|b| b.as_ref()));
                Opcode::AuthSuccess
            },
        };
        
        (opcode, buf)
    }
}

fn encode_rows_metadata(buf: &mut BytesMut, metadata: &RowsMetadata) {
    write_int(buf, metadata.flags);
    write_int(buf, metadata.columns_count);
    
    // Flags:
    // 0x0001 = Global_tables_spec
    // 0x0002 = Has_more_pages
    // 0x0004 = No_metadata
    
    if metadata.flags & 0x0002 != 0 {
        write_bytes(buf, metadata.paging_state.as_ref().map(|b| b.as_ref()));
    }
    
    if metadata.flags & 0x0004 == 0 {
        if metadata.flags & 0x0001 != 0 {
            // Global table spec
            write_string(buf, metadata.keyspace.as_ref().unwrap_or(&String::new()));
            write_string(buf, metadata.table.as_ref().unwrap_or(&String::new()));
        }
        
        for col in &metadata.columns {
            if metadata.flags & 0x0001 == 0 {
                write_string(buf, col.keyspace.as_ref().unwrap_or(&String::new()));
                write_string(buf, col.table.as_ref().unwrap_or(&String::new()));
            }
            write_string(buf, &col.name);
            write_short(buf, col.col_type as u16);
        }
    }
}

impl ErrorResponse {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code as i32,
            message: message.into(),
        }
    }
    
    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ServerError, message)
    }
    
    pub fn syntax_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::SyntaxError, message)
    }
    
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }
}
