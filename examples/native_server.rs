//! Cassandra Native Protocol 서버 실행 예제
//!
//! 실행: cargo run --example native_server
//! 
//! 테스트 (cqlsh):
//!   cqlsh localhost 9042
//!   > CREATE KEYSPACE test WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1};
//!   > USE test;
//!   > CREATE TABLE users (id int PRIMARY KEY, name text);
//!   > INSERT INTO test.users (id, name) VALUES (1, 'Alice');
//!   > SELECT * FROM test.users;

use std::sync::Arc;
use coredb::database::{CoreDB, DatabaseConfig};
use coredb::protocol::server::{NativeServer, ServerConfig};
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 로깅 초기화
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
    
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║        🚀 CoreDB Native Protocol Server                   ║");
    println!("║                                                           ║");
    println!("║   Cassandra 드라이버와 100% 호환!                         ║");
    println!("║                                                           ║");
    println!("║   연결 방법:                                              ║");
    println!("║     cqlsh localhost 9042                                  ║");
    println!("║                                                           ║");
    println!("║   또는 Cassandra 드라이버 사용:                           ║");
    println!("║     - Python: cassandra-driver                            ║");
    println!("║     - Java: DataStax Java Driver                          ║");
    println!("║     - Node.js: cassandra-driver                           ║");
    println!("║     - Rust: scylla-cql / cdrs-tokio                       ║");
    println!("╚═══════════════════════════════════════════════════════════╝");
    println!();
    
    // 데이터베이스 초기화
    let config = DatabaseConfig {
        data_directory: std::path::PathBuf::from("./native_server_data"),
        commitlog_directory: std::path::PathBuf::from("./native_server_commitlog"),
        ..Default::default()
    };
    
    let db = Arc::new(CoreDB::new(config).await?);
    
    // 서버 설정
    let server_config = ServerConfig {
        host: "0.0.0.0".to_string(),
        port: 9042,
        max_connections: 1000,
    };
    
    // 서버 시작
    let server = NativeServer::new(db, server_config);
    server.start().await?;
    
    Ok(())
}
