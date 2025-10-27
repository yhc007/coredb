use coredb::*;
use coredb::schema::*;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    println!("🚀 CoreDB Persistence Example");
    println!("==============================\n");

    let data_dir = PathBuf::from("./example_data");
    
    // 1. 데이터베이스 설정
    let config = DatabaseConfig {
        data_directory: data_dir.clone(),
        commitlog_directory: data_dir.join("commitlog"),
        memtable_flush_threshold_mb: 64,
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
    };
    
    println!("1️⃣  Creating database...");
    let db = CoreDB::new(config).await.expect("Failed to create database");
    
    // 2. 키스페이스 생성
    println!("\n2️⃣  Creating keyspace...");
    db.create_keyspace("demo".to_string(), 1)
        .await
        .expect("Failed to create keyspace");
    println!("✓ Created keyspace: demo");
    
    // 3. 테이블 생성
    println!("\n3️⃣  Creating table...");
    let schema = TableSchema::new(
        "users".to_string(),
        "demo".to_string(),
        vec![ColumnDefinition {
            name: "id".to_string(),
            data_type: CassandraDataType::Int,
            is_static: false,
        }],
        vec![],
        vec![ColumnDefinition {
            name: "name".to_string(),
            data_type: CassandraDataType::Text,
            is_static: false,
        }],
        vec![],
    );
    
    db.create_table("demo".to_string(), "users".to_string(), schema)
        .await
        .expect("Failed to create table");
    println!("✓ Created table: demo.users");
    
    // 4. 데이터 삽입
    println!("\n4️⃣  Inserting data...");
    for i in 1..=5 {
        let row = Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(i)],
            },
            clustering_key: None,
            cells: {
                let mut cells = std::collections::HashMap::new();
                cells.insert("name".to_string(), Cell {
                    value: CassandraValue::Text(format!("User #{}", i)),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        db.insert_row("demo", "users", row)
            .await
            .expect("Failed to insert row");
        println!("✓ Inserted: User #{}", i);
    }
    
    // 5. 데이터베이스 통계
    println!("\n5️⃣  Database statistics:");
    let stats = db.get_stats().await;
    println!("  Keyspaces: {}", stats.keyspace_count);
    println!("  Tables: {}", stats.table_count);
    println!("  Memtables: {}", stats.memtable_count);
    
    // 6. Persistence 저장
    println!("\n6️⃣  Saving to disk...");
    db.save_to_disk().await.expect("Failed to save");
    println!("✓ Database saved successfully!");
    
    println!("\n📁 Data directory: {}", data_dir.display());
    println!("✅ Persistence example completed!");
    
    // 정리
    println!("\n🧹 Cleaning up test data...");
    std::fs::remove_dir_all(&data_dir).ok();
}

