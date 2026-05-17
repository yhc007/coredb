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
        // Scylla driver's topology-refresh queries don't have backing
        // tables in CoreDB — without an intercept here, every refresh
        // ticks back a SyntaxError("Table ... does not exist"), and the
        // driver retries ~1×/sec per connection, flooding the journal.
        // Serve synthetic responses for the three known patterns so the
        // driver stops looping. See `system_table_response` for the
        // exact pattern match.
        if let Some(response) = system_table_response(&query.query) {
            return response;
        }
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
                let columns = build_column_specs(&rows);
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

/// Build the response's column-spec list from a SELECT result rowset.
///
/// Why this lives outside `convert_result`: the column-spec build is
/// the bug-prone part of the result-frame builder, so a unit test
/// wants to drive it directly without standing up a `RequestHandler`
/// (which requires `Arc<CoreDB>`).
///
/// Two invariants this guards:
///
/// 1. **Union of row keys**, *not* `first_row.columns.keys()`. Rows
///    written before an `ALTER TABLE ... ADD col` have no cell for
///    the new column; with first-row-only, the new column silently
///    disappears whenever ordering puts a pre-ALTER row first. The
///    union is order-preserving — the first time a column is seen
///    pins its position, and later rows with the same key are
///    ignored, so the output order stays deterministic for an
///    otherwise-deterministic input.
///
/// 2. **Type inference from the first non-NULL sample**. The previous
///    iteration hard-coded `CqlType::Varchar` which made the scylla
///    driver try to UTF-8-decode binary payloads. Falling back to
///    Varchar only when *every* sample is NULL preserves the safe
///    historical behaviour for all-null columns.
fn build_column_specs(rows: &[crate::query::result::Row]) -> Vec<ColumnSpec> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for row in rows {
        for name in row.columns.keys() {
            if seen.insert(name.as_str()) {
                ordered.push(name.clone());
            }
        }
    }
    ordered
        .into_iter()
        .map(|name| {
            let col_type = rows
                .iter()
                .find_map(|r| r.columns.get(&name).and_then(cql_type_for_value))
                .unwrap_or(CqlType::Varchar);
            ColumnSpec {
                keyspace: None,
                table: None,
                name,
                col_type,
            }
        })
        .collect()
}

/// Recognize the scylla driver's three topology-refresh queries and
/// return a synthetic response, or `None` for any query that should
/// fall through to the real CQL engine.
///
/// The driver issues these on every metadata-refresh tick (~1/s per
/// connection). Without an intercept they fall to `execute_cql` →
/// "Table 'X' does not exist" SyntaxError → driver retries
/// immediately → flood. We return successful empty rowsets (and a
/// single synthetic row for `system.local`) so the driver accepts
/// the response, completes its refresh, and waits a full cycle
/// before asking again.
///
/// Pattern match is whitespace-normalized + case-insensitive so a
/// driver that decides to reformat its query verbatim still hits the
/// intercept. The matched queries are fixed strings on the driver
/// side — the patterns here mirror them character-for-character.
///
/// `system.local` is populated with a single deterministic row
/// because the driver uses `host_id` as its self-identity. An empty
/// rowset there triggers a one-time "Initial metadata read failed"
/// fallback warning per connection.  `system.peers` is genuinely
/// empty (single-node deployment) and `system_schema.types` is
/// empty because CoreDB has no UDTs.
fn system_table_response(raw_query: &str) -> Option<Response> {
    let normalized: String = raw_query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    const LOCAL_Q: &str =
        "select host_id, rpc_address, data_center, rack, tokens from system.local";

    if normalized == LOCAL_Q {
        // system.local needs a single populated row (driver uses
        // host_id as self-identity). Every other system /
        // system_schema SELECT is empty — the wildcard below
        // handles them.
        Some(build_system_local_response())
    } else if let Some((keyspace, table, columns)) = parse_system_select(&normalized) {
        // Wildcard fallback for any other `system.*` /
        // `system_schema.*` SELECT we haven't hand-stubbed above.
        // The scylla driver iterates through `system_schema.views`,
        // `system_schema.keyspaces`, `system_schema.functions`,
        // etc. during metadata refresh — too many to enumerate
        // up-front, and they all expect "empty discovery" to be a
        // valid response. We return an empty rowset declaring the
        // requested columns as Varchar; the driver decodes 0 rows
        // (so column-type mismatches at the value level never
        // surface) and continues. Specific shapes like `system.local`
        // that need real values keep their hand-built responses
        // above.
        Some(build_empty_rowset_owned(&keyspace, &table, &columns))
    } else {
        None
    }
}

/// Parse a `select COL1, COL2 from system(_schema)?.TBL [WHERE ...]`
/// shape into `(keyspace, table, column_names)`. Returns `None` for
/// any query that doesn't fit this very narrow shape — most user
/// queries fail the prefix check on the first line and pass through
/// to the real engine without further work.
///
/// The input is already lowercased + whitespace-normalized by the
/// caller, so the parser only deals with one canonical form.
fn parse_system_select(normalized: &str) -> Option<(String, String, Vec<String>)> {
    let after_select = normalized.strip_prefix("select ")?;
    let from_idx = after_select.find(" from ")?;
    let columns_part = &after_select[..from_idx];
    let after_from = &after_select[from_idx + " from ".len()..];

    // Take the first whitespace-delimited token after FROM — this
    // skips any trailing WHERE / LIMIT / ORDER BY clauses that
    // Cassandra accepts but we don't need to model.
    let table_qualified = after_from.split_whitespace().next()?;
    let (ks, tbl) = table_qualified.split_once('.')?;
    if ks != "system" && ks != "system_schema" {
        return None;
    }

    let columns: Vec<String> = columns_part
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if columns.is_empty() {
        return None;
    }
    Some((ks.to_string(), tbl.to_string(), columns))
}

/// Same shape as [`build_empty_rowset`] but accepts owned column
/// names so a caller that's parsed them out of a query string can
/// pass them in without a 'static lifetime dance. Column types are
/// looked up in [`schema_column_cql_type`] — most cells get
/// `Varchar`, but well-known structured columns (`replication`,
/// `tokens`, `field_names`, etc.) get the right collection type
/// so the driver's `TypeCheckError` doesn't fire on metadata
/// inspection. The driver type-checks column metadata even for
/// empty rowsets, so getting these right matters.
fn build_empty_rowset_owned(keyspace: &str, table: &str, columns: &[String]) -> Response {
    let column_specs: Vec<ColumnSpec> = columns
        .iter()
        .map(|name| ColumnSpec {
            keyspace: None,
            table: None,
            name: name.clone(),
            col_type: schema_column_cql_type(name),
        })
        .collect();
    let columns_count = column_specs.len() as i32;
    Response::Result(ResultResponse::Rows(RowsResult {
        metadata: RowsMetadata {
            flags: 0x0001,
            columns_count,
            paging_state: None,
            keyspace: Some(keyspace.to_string()),
            table: Some(table.to_string()),
            columns: column_specs,
        },
        rows: Vec::new(),
    }))
}

/// Known column-name → CQL-type mapping for Cassandra-spec
/// `system_schema.*` tables. The scylla driver type-checks
/// these against typed Rust structs (e.g. `replication` →
/// `HashMap<String, String>`), so even an empty rowset must
/// declare the column with the right wire type — declaring
/// `replication` as `Text` triggers a `TypeCheckError` and the
/// metadata refresh keeps retrying.
///
/// Names not in the table default to `Varchar`, which is correct
/// for every other column the driver currently inspects.
fn schema_column_cql_type(name: &str) -> CqlType {
    match name {
        // uuid
        "host_id" => CqlType::Uuid,
        // inet
        "rpc_address" | "broadcast_address" | "listen_address" => CqlType::Inet,
        // map<text, text>
        "replication" => CqlType::Map,
        // set<text>
        "tokens" => CqlType::Set,
        // list<text>
        "field_names" | "field_types" => CqlType::List,
        // int
        "position" => CqlType::Int,
        _ => CqlType::Varchar,
    }
}

/// Empty `RESULT/Rows` response with the requested column shape but
/// zero rows. Used for `system.peers` and `system_schema.types`,
/// both of which are legitimately empty for a single-node CoreDB
/// deployment. Setting `columns_count` correctly is what makes the
/// driver accept this as a successful response rather than a
/// partial / malformed one.
fn build_empty_rowset(
    keyspace: &str,
    table: &str,
    columns: &[(&str, CqlType)],
) -> Response {
    let column_specs: Vec<ColumnSpec> = columns
        .iter()
        .map(|(name, col_type)| ColumnSpec {
            keyspace: None,
            table: None,
            name: (*name).to_string(),
            col_type: *col_type,
        })
        .collect();
    let columns_count = column_specs.len() as i32;
    Response::Result(ResultResponse::Rows(RowsResult {
        metadata: RowsMetadata {
            flags: 0x0001, // Global_tables_spec
            columns_count,
            paging_state: None,
            keyspace: Some(keyspace.to_string()),
            table: Some(table.to_string()),
            columns: column_specs,
        },
        rows: Vec::new(),
    }))
}

/// Single-row `system.local` response. `host_id` is `Uuid::nil()` —
/// stable across restarts so a driver that caches host identity
/// won't re-bootstrap unnecessarily; deterministic for tests too.
/// `rpc_address` and `tokens` are NULL because CoreDB doesn't have
/// a structured Inet / Set encoder.
fn build_system_local_response() -> Response {
    let host_id_bytes = uuid::Uuid::nil().as_bytes().to_vec();
    // IPv4 127.0.0.1 — 4 bytes, big-endian. The scylla driver
    // deserializes Inet to a non-Optional `IpAddr` in its
    // `NodeInfoRow`, so a NULL cell here triggers a
    // "expected a non-null value, got null" deserialization
    // error and the metadata read keeps retrying. Real
    // Cassandra populates rpc_address with the listener's
    // broadcast address; loopback is the honest default for a
    // single-node CoreDB.
    let rpc_address_bytes: Vec<u8> = vec![127, 0, 0, 1];
    // Set<Varchar> with a single token "0". Cassandra v4
    // collection encoding: `[int count][int len_1][bytes_1]...`.
    // Per-item: `[int len][bytes]`.
    //
    // An EMPTY tokens set triggers the scylla driver's
    // "Bad peers metadata: All peers have empty token lists"
    // check — even on a single-node cluster, the driver wants at
    // least one token so its range-routing math has something to
    // chew on. "0" is a deterministic single-token placeholder.
    let one_token_cell: Vec<u8> = {
        let mut buf = Vec::with_capacity(13);
        buf.extend_from_slice(&1i32.to_be_bytes()); // count = 1
        buf.extend_from_slice(&1i32.to_be_bytes()); // first item length = 1
        buf.push(b'0');                              // first item bytes = "0"
        buf
    };
    let row: Vec<Option<Bytes>> = vec![
        Some(Bytes::from(host_id_bytes)),                       // host_id
        Some(Bytes::from(rpc_address_bytes)),                   // rpc_address (127.0.0.1)
        Some(Bytes::from("coredb-dc".as_bytes().to_vec())),     // data_center
        Some(Bytes::from("coredb-rack".as_bytes().to_vec())),   // rack
        Some(Bytes::from(one_token_cell)),                      // tokens (single-token set<text>)
    ];
    let column_specs = vec![
        ColumnSpec { keyspace: None, table: None, name: "host_id".to_string(),     col_type: CqlType::Uuid },
        ColumnSpec { keyspace: None, table: None, name: "rpc_address".to_string(), col_type: CqlType::Inet },
        ColumnSpec { keyspace: None, table: None, name: "data_center".to_string(), col_type: CqlType::Varchar },
        ColumnSpec { keyspace: None, table: None, name: "rack".to_string(),        col_type: CqlType::Varchar },
        ColumnSpec { keyspace: None, table: None, name: "tokens".to_string(),      col_type: CqlType::Set },
    ];
    Response::Result(ResultResponse::Rows(RowsResult {
        metadata: RowsMetadata {
            flags: 0x0001,
            columns_count: 5,
            paging_state: None,
            keyspace: Some("system".to_string()),
            table: Some("local".to_string()),
            columns: column_specs,
        },
        rows: vec![row],
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::result::Row as ResultRow;
    use std::collections::HashMap;

    fn row(cells: &[(&str, CassandraValue)]) -> ResultRow {
        let mut columns = HashMap::new();
        for (name, value) in cells {
            columns.insert((*name).to_string(), value.clone());
        }
        ResultRow { columns }
    }

    #[test]
    fn empty_rowset_yields_empty_columns() {
        let columns = build_column_specs(&[]);
        assert!(columns.is_empty());
    }

    #[test]
    fn column_type_derived_from_first_non_null_sample() {
        // Column `n` is NULL in row 0, Int(7) in row 1.
        // Expectation: col_type = Int (not Varchar fallback).
        let rows = vec![
            row(&[("n", CassandraValue::Null)]),
            row(&[("n", CassandraValue::Int(7))]),
        ];
        let columns = build_column_specs(&rows);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].name, "n");
        assert_eq!(columns[0].col_type, CqlType::Int);
    }

    #[test]
    fn all_null_column_falls_back_to_varchar() {
        let rows = vec![
            row(&[("x", CassandraValue::Null)]),
            row(&[("x", CassandraValue::Null)]),
        ];
        let columns = build_column_specs(&rows);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].col_type, CqlType::Varchar);
    }

    /// THE regression test for the column-spec union fix.
    ///
    /// Reproduces the scenario that broke `compare-pnl` in production:
    /// a SELECT response contains rows from a table that was ALTERed
    /// to add a new column (here `strategy`), so older rows have no
    /// cell for it. Before the fix, the column-spec list was built
    /// from `rows[0].columns.keys()`. If the older row sorted first,
    /// the new column disappeared from the wire and the scylla
    /// driver's typed-row deserializer rejected the whole batch.
    ///
    /// Post-fix expectation: every column that appears in *any* row
    /// surfaces in the column spec, regardless of which row is first.
    #[test]
    fn column_spec_includes_columns_missing_from_first_row() {
        let rows = vec![
            // Pre-ALTER row: has the original columns but no `strategy`.
            row(&[
                ("id", CassandraValue::Int(1)),
                ("name", CassandraValue::Text("alpha".into())),
            ]),
            // Post-ALTER row: includes the new `strategy` column.
            row(&[
                ("id", CassandraValue::Int(2)),
                ("name", CassandraValue::Text("beta".into())),
                ("strategy", CassandraValue::Text("deepseek".into())),
            ]),
        ];
        let columns = build_column_specs(&rows);
        let names: std::collections::HashSet<&str> =
            columns.iter().map(|c| c.name.as_str()).collect();
        assert!(
            names.contains("strategy"),
            "regression: column spec lost `strategy` because the first row had no cell for it; \
             got columns {:?}",
            names
        );
        // The strategy column should be typed correctly, not Varchar
        // by accident — confirms type inference still scans the
        // whole rowset for non-NULL samples.
        let strategy_col = columns.iter().find(|c| c.name == "strategy").unwrap();
        assert_eq!(strategy_col.col_type, CqlType::Varchar); // Text → Varchar
    }

    #[test]
    fn column_spec_first_seen_position_is_preserved() {
        // Stability matters: a deterministic-input SELECT should
        // produce deterministic column order. Each column's
        // position is fixed by the first row that mentions it.
        let rows = vec![
            row(&[("a", CassandraValue::Int(1)), ("b", CassandraValue::Int(2))]),
            row(&[("c", CassandraValue::Int(3)), ("a", CassandraValue::Int(4))]),
        ];
        let columns = build_column_specs(&rows);
        let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
        // row 0 introduces `a` then `b`; row 1 introduces `c` (but
        // HashMap iteration of row 0 may not be a/b — we can only
        // assert that `c` appears after either `a` or `b`).
        let c_idx = names.iter().position(|n| *n == "c").unwrap();
        let a_idx = names.iter().position(|n| *n == "a").unwrap();
        let b_idx = names.iter().position(|n| *n == "b").unwrap();
        assert!(c_idx > a_idx && c_idx > b_idx,
            "expected c to appear after a and b; got {names:?}");
    }

    /// Pin the three exact query strings the scylla rust-driver
    /// sends on every topology-refresh tick. If the driver changes
    /// any of them, the intercept stops matching, the queries
    /// fall through to `execute_cql`, and the syntax-error flood
    /// returns. This test fails loud before that happens in prod.
    #[test]
    fn system_table_response_matches_all_six_known_queries() {
        let queries: &[(&str, &str)] = &[
            (
                "system.local",
                "select host_id, rpc_address, data_center, rack, tokens from system.local",
            ),
            (
                "system.peers",
                "select host_id, rpc_address, data_center, rack, tokens from system.peers",
            ),
            (
                "system_schema.types",
                "select keyspace_name, type_name, field_names, field_types from system_schema.types",
            ),
            (
                "system_schema.columns",
                "select keyspace_name, table_name, column_name, kind, position, type from system_schema.columns",
            ),
            (
                "system_schema.tables",
                "select keyspace_name, table_name from system_schema.tables",
            ),
            (
                "system_schema.scylla_tables",
                "select keyspace_name, table_name, partitioner from system_schema.scylla_tables",
            ),
        ];
        for (label, q) in queries {
            assert!(
                system_table_response(q).is_some(),
                "{label} query should hit intercept; got None"
            );
        }
    }

    /// Case + run-of-whitespace normalization: a driver that emits
    /// uppercase keywords or extra spaces *between tokens* (but
    /// preserves comma adjacency, which every Cassandra driver does)
    /// still hits the intercept. The SCYLLA rust-driver writes
    /// lowercase + single-space, but the DataStax / Python drivers
    /// emit uppercase SQL — keep the intercept tolerant.
    #[test]
    fn system_table_response_is_case_and_run_whitespace_insensitive() {
        let r = system_table_response(
            "SELECT host_id, rpc_address,\tdata_center, rack,\ttokens  FROM\nSYSTEM.LOCAL",
        );
        assert!(r.is_some(), "normalization should match despite reformatting");
    }

    /// User-keyspace queries fall through to the real CQL engine.
    /// The wildcard fallback IS deliberately keyspace-gated, so a
    /// SELECT against `polymarket_btc.X` or any other user
    /// keyspace must return None.
    ///
    /// Non-SELECT statements (CREATE, INSERT, UPDATE, DELETE)
    /// also fall through unconditionally — the intercept only
    /// handles SELECT.
    #[test]
    fn system_table_response_returns_none_for_unrelated_queries() {
        assert!(system_table_response("SELECT * FROM polymarket_btc.markets").is_none());
        assert!(system_table_response("CREATE TABLE foo.bar (id INT PRIMARY KEY)").is_none());
        assert!(system_table_response("INSERT INTO system.local (x) VALUES (1)").is_none());
        // Note: SELECTs against system.* / system_schema.* (including
        // SELECT *) ARE intercepted by design — empty rowset for
        // anything not specifically hand-stubbed. See
        // `system_select_wildcard_catches_unknown_schema_tables`
        // for that contract.
    }

    /// `system.local` returns exactly one row with 5 columns. Every
    /// non-collection column the driver reads non-Optionally must
    /// be non-null (`host_id` UUID + `rpc_address` IP), or the
    /// driver's `NodeInfoRow` deserialization fails. `tokens` is
    /// `set<text>` which the driver decodes as `Vec<String>`, so
    /// an empty-set cell (count = 0) suffices.
    #[test]
    fn system_local_response_has_one_row_with_five_columns() {
        let response = system_table_response(
            "select host_id, rpc_address, data_center, rack, tokens from system.local",
        )
        .expect("system.local should be intercepted");
        match response {
            Response::Result(ResultResponse::Rows(r)) => {
                assert_eq!(r.metadata.columns_count, 5);
                assert_eq!(r.metadata.columns.len(), 5);
                assert_eq!(r.rows.len(), 1, "system.local must return exactly one row");
                let names: Vec<&str> = r.metadata.columns.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, vec!["host_id", "rpc_address", "data_center", "rack", "tokens"]);
                // host_id: 16-byte UUID, non-null (self-identity).
                assert_eq!(r.rows[0][0].as_ref().unwrap().len(), 16);
                // rpc_address: 4-byte IPv4, non-null. Driver
                // type is `IpAddr` (not Optional), so NULL
                // triggers a deserialization error.
                assert_eq!(r.rows[0][1].as_ref().unwrap().as_ref(), &[127, 0, 0, 1]);
                // data_center / rack: text, non-null for stable identity.
                assert!(r.rows[0][2].is_some());
                assert!(r.rows[0][3].is_some());
                // tokens: Set<Varchar> with a single placeholder
                // token "0". Wire form: count=1, item_len=1,
                // item_bytes='0'. Empty would trigger driver's
                // "All peers have empty token lists" rejection.
                assert_eq!(
                    r.rows[0][4].as_ref().unwrap().as_ref(),
                    &[0, 0, 0, 1, 0, 0, 0, 1, b'0'],
                );
                // Set column type tags Set in metadata so the
                // codec emits the inner element-type byte after it.
                let tokens_col = r.metadata.columns.iter().find(|c| c.name == "tokens").unwrap();
                assert_eq!(tokens_col.col_type, CqlType::Set);
            }
            other => panic!("expected Rows response, got {other:?}"),
        }
    }

    /// `system.peers` and `system_schema.types` return zero rows but
    /// with proper column metadata so the driver parses the response
    /// as "0 results" not "malformed".
    /// The scylla driver iterates through arbitrary system_schema
    /// tables during metadata refresh — `views`, `keyspaces`,
    /// `functions`, `aggregates`, `indexes`, etc. Hand-stubbing
    /// each one is whack-a-mole; the wildcard fallback catches
    /// anything not in the specific list and returns an empty
    /// rowset built from the SELECT's own column list.
    #[test]
    fn system_select_wildcard_catches_unknown_schema_tables() {
        // Pick names we deliberately don't hand-stub.
        let cases = [
            "select keyspace_name, view_name from system_schema.views",
            "select keyspace_name from system_schema.keyspaces",
            "select keyspace_name, function_name from system_schema.functions",
            "select keyspace_name, aggregate_name from system_schema.aggregates",
            "select keyspace_name, index_name from system_schema.indexes",
        ];
        for q in cases {
            let r = system_table_response(q);
            let r = r.unwrap_or_else(|| panic!("wildcard should catch: {q}"));
            match r {
                Response::Result(ResultResponse::Rows(rows)) => {
                    assert_eq!(rows.rows.len(), 0, "{q}: empty rowset");
                    assert!(rows.metadata.columns_count > 0, "{q}: column metadata present");
                }
                other => panic!("{q}: expected Rows, got {other:?}"),
            }
        }
    }

    /// Wildcard fallback must NOT hijack user-keyspace queries.
    /// A SELECT from `polymarket_btc.markets` looks superficially
    /// similar to a system-table SELECT (both `select ... from K.T`),
    /// so the parser must explicitly check the keyspace is
    /// `system` or `system_schema`.
    #[test]
    fn system_select_wildcard_ignores_user_keyspace_queries() {
        assert!(
            system_table_response("select slug from polymarket_btc.markets").is_none(),
            "wildcard must not eat real user queries"
        );
        assert!(
            system_table_response("select * from foo.bar").is_none(),
            "user-keyspace SELECT * must fall through to engine"
        );
    }

    /// `replication` is `map<text, text>` in `system_schema.keyspaces`,
    /// and the scylla driver type-checks even empty rowsets. The
    /// wildcard fallback must declare it as `Map` (codec then emits
    /// the inner element-type bytes) — otherwise the driver
    /// rejects the response with "neither a map".
    #[test]
    fn keyspaces_replication_column_typed_as_map_in_wildcard() {
        let r = system_table_response(
            "select keyspace_name, replication from system_schema.keyspaces",
        )
        .expect("wildcard should catch system_schema.keyspaces");
        if let Response::Result(ResultResponse::Rows(rows)) = r {
            let replication_col = rows
                .metadata
                .columns
                .iter()
                .find(|c| c.name == "replication")
                .expect("replication column present");
            assert_eq!(
                replication_col.col_type,
                CqlType::Map,
                "replication must be Map<varchar,varchar>, not Varchar — the driver \
                 type-checks against HashMap<String,String> even for empty rowsets",
            );
        } else {
            panic!("expected Rows");
        }
    }

    /// Spot-check the column-type lookup table covers the known
    /// non-Text columns. Defaults to Varchar for unknown names.
    #[test]
    fn schema_column_cql_type_known_collections() {
        assert_eq!(schema_column_cql_type("host_id"), CqlType::Uuid);
        assert_eq!(schema_column_cql_type("rpc_address"), CqlType::Inet);
        assert_eq!(schema_column_cql_type("broadcast_address"), CqlType::Inet);
        assert_eq!(schema_column_cql_type("listen_address"), CqlType::Inet);
        assert_eq!(schema_column_cql_type("replication"), CqlType::Map);
        assert_eq!(schema_column_cql_type("tokens"), CqlType::Set);
        assert_eq!(schema_column_cql_type("field_names"), CqlType::List);
        assert_eq!(schema_column_cql_type("field_types"), CqlType::List);
        assert_eq!(schema_column_cql_type("position"), CqlType::Int);
        // Unknown name → Varchar.
        assert_eq!(schema_column_cql_type("arbitrary_unknown"), CqlType::Varchar);
    }

    /// `parse_system_select` round-trip: column names + table
    /// qualifier come back exactly as the SELECT declared.
    #[test]
    fn parse_system_select_extracts_columns_and_table() {
        let (ks, tbl, cols) = parse_system_select(
            "select alpha, beta, gamma from system_schema.functions",
        )
        .unwrap();
        assert_eq!(ks, "system_schema");
        assert_eq!(tbl, "functions");
        assert_eq!(cols, vec!["alpha", "beta", "gamma"]);
    }

    /// Trailing WHERE / LIMIT clauses don't confuse the table
    /// qualifier parser. (Drivers don't actually do this for
    /// metadata refresh, but it's cheap insurance.)
    #[test]
    fn parse_system_select_handles_trailing_clauses() {
        let result = parse_system_select(
            "select keyspace_name from system_schema.views where keyspace_name = 'x'",
        );
        assert!(result.is_some(), "trailing WHERE must not break parse");
    }

    #[test]
    fn system_peers_and_types_responses_are_empty_with_metadata() {
        let peers = system_table_response(
            "select host_id, rpc_address, data_center, rack, tokens from system.peers",
        )
        .unwrap();
        let types = system_table_response(
            "select keyspace_name, type_name, field_names, field_types from system_schema.types",
        )
        .unwrap();
        let columns = system_table_response(
            "select keyspace_name, table_name, column_name, kind, position, type from system_schema.columns",
        )
        .unwrap();
        for (label, r) in [("peers", peers), ("types", types), ("columns", columns)] {
            match r {
                Response::Result(ResultResponse::Rows(rows)) => {
                    assert_eq!(rows.rows.len(), 0, "{label}: must be empty");
                    assert!(
                        rows.metadata.columns_count > 0,
                        "{label}: must declare columns even with 0 rows"
                    );
                    assert_eq!(
                        rows.metadata.columns.len() as i32,
                        rows.metadata.columns_count,
                        "{label}: columns_count must match column-spec length"
                    );
                }
                other => panic!("{label}: expected Rows, got {other:?}"),
            }
        }
    }

    #[test]
    fn duplicate_column_across_rows_appears_once() {
        let rows = vec![
            row(&[("x", CassandraValue::Int(1))]),
            row(&[("x", CassandraValue::Int(2))]),
            row(&[("x", CassandraValue::Int(3))]),
        ];
        let columns = build_column_specs(&rows);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].name, "x");
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
        CassandraValue::Counter(c) => buf.extend_from_slice(&c.to_be_bytes()),
        CassandraValue::UDT(fields) => {
            // UDT는 Map과 유사하게 문자열로 변환
            buf.extend_from_slice(format!("{:?}", fields).as_bytes());
        },
    }
    
    buf.freeze()
}
