use coredb::{CoreDB, DatabaseConfig};
use std::path::PathBuf;
use tokio;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 데이터베이스 설정
    let config = DatabaseConfig {
        data_directory: PathBuf::from("./example_data"),
        commitlog_directory: PathBuf::from("./example_commitlog"),
        memtable_flush_threshold_mb: 16, // 작은 값으로 설정
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
    };
    
    // 데이터베이스 초기화
    println!("Initializing CoreDB...");
    let db = CoreDB::new(config).await?;
    
    // 키스페이스 생성
    println!("Creating keyspace...");
    db.create_keyspace("demo".to_string(), 1).await?;
    
    // 테이블 생성
    println!("Creating table...");
    use coredb::schema::{TableSchema, ColumnDefinition, CassandraDataType};
    
    let schema = TableSchema::new(
        "users".to_string(),
        "demo".to_string(),
        vec![ColumnDefinition {
            name: "id".to_string(),
            data_type: CassandraDataType::Int,
            is_static: false,
        }],
        vec![], // 클러스터링 키 없음
        vec![
            ColumnDefinition {
                name: "name".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            },
            ColumnDefinition {
                name: "email".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            },
            ColumnDefinition {
                name: "age".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            },
        ],
        vec![], // 정적 컬럼 없음
    );
    
    db.create_table("demo".to_string(), "users".to_string(), schema).await?;
    
    // 샘플 데이터 삽입
    println!("Inserting sample data...");
    use coredb::schema::{Row, PartitionKey, Cell, CassandraValue};
    use std::collections::HashMap;
    
    let users = vec![
        (1, "John Doe", "john@example.com", 30),
        (2, "Jane Smith", "jane@example.com", 25),
        (3, "Bob Johnson", "bob@example.com", 35),
        (4, "Alice Brown", "alice@example.com", 28),
        (5, "Charlie Wilson", "charlie@example.com", 42),
    ];
    
    for (id, name, email, age) in users {
        let mut cells = HashMap::new();
        cells.insert("name".to_string(), Cell {
            value: CassandraValue::Text(name.to_string()),
            timestamp: chrono::Utc::now().timestamp_micros(),
            ttl: None,
            is_deleted: false,
        });
        cells.insert("email".to_string(), Cell {
            value: CassandraValue::Text(email.to_string()),
            timestamp: chrono::Utc::now().timestamp_micros(),
            ttl: None,
            is_deleted: false,
        });
        cells.insert("age".to_string(), Cell {
            value: CassandraValue::Int(age),
            timestamp: chrono::Utc::now().timestamp_micros(),
            ttl: None,
            is_deleted: false,
        });
        
        let row = Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(id)],
            },
            clustering_key: None,
            cells,
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        db.insert_row("demo", "users", row).await?;
        println!("Inserted user {}: {}", id, name);
    }
    
    // 데이터 조회
    println!("\nRetrieving data...");
    
    // 모든 사용자 조회
    for id in 1..=5 {
        let partition_key = PartitionKey {
            components: vec![CassandraValue::Int(id)],
        };
        
        if let Some(row) = db.get_row("demo", "users", &partition_key, &None).await? {
            let name = row.cells.get("name").unwrap().value.clone();
            let email = row.cells.get("email").unwrap().value.clone();
            let age = row.cells.get("age").unwrap().value.clone();
            
            println!("User {}: name={:?}, email={:?}, age={:?}", id, name, email, age);
        }
    }
    
    // 통계 출력
    println!("\nDatabase statistics:");
    let stats = db.get_stats().await;
    println!("Keyspaces: {}", stats.keyspace_count);
    println!("Tables: {}", stats.table_count);
    println!("Memtables: {}", stats.memtable_count);
    println!("SSTables: {}", stats.sstable_count);
    println!("Total size: {:.2} MB", stats.total_size_bytes as f64 / 1024.0 / 1024.0);
    
    // CQL 쿼리 실행 예제
    println!("\nExecuting CQL queries...");
    
    // 키스페이스 생성
    let result = db.execute_cql("CREATE KEYSPACE test_ks WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await?;
    println!("Create keyspace result: {:?}", result);
    
    // 테이블 생성
    let result = db.execute_cql("CREATE TABLE test_ks.test_table (id INT PRIMARY KEY, name TEXT)").await?;
    println!("Create table result: {:?}", result);
    
    // 데이터 삽입
    let result = db.execute_cql("INSERT INTO test_ks.test_table (id, name) VALUES (1, 'Test User')").await?;
    println!("Insert result: {:?}", result);
    
    // 데이터 조회
    let result = db.execute_cql("SELECT * FROM test_ks.test_table WHERE id = 1").await?;
    println!("Select result: {:?}", result);
    
    println!("\nExample completed successfully!");
    
    Ok(())
}
