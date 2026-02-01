//! Sam의 메모리 시스템 - CoreDB 테스트
//! 
//! 실행: cargo run --example sam_memory

use coredb::{CoreDB, DatabaseConfig};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦊 Sam의 CoreDB 메모리 시스템 테스트\n");
    
    // 데이터베이스 설정
    let config = DatabaseConfig {
        data_directory: PathBuf::from("./sam_data"),
        commitlog_directory: PathBuf::from("./sam_commitlog"),
        memtable_flush_threshold_mb: 16,
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
    };
    
    // 데이터베이스 초기화
    println!("📦 CoreDB 초기화...");
    let db = CoreDB::new(config).await?;
    
    // 1. 키스페이스 생성
    println!("📦 sam 키스페이스 생성...");
    let result = db.execute_cql(
        "CREATE KEYSPACE sam WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}"
    ).await;
    match result {
        Ok(_) => println!("✅ 키스페이스 생성 완료!"),
        Err(e) => println!("⚠️ 키스페이스: {:?}", e),
    }
    
    // 2. memories 테이블 생성
    println!("\n📊 memories 테이블 생성...");
    let result = db.execute_cql(
        "CREATE TABLE sam.memories (id TEXT PRIMARY KEY, category TEXT, content TEXT, importance INT)"
    ).await;
    match result {
        Ok(_) => println!("✅ 테이블 생성 완료!"),
        Err(e) => println!("⚠️ 테이블: {:?}", e),
    }
    
    // 3. 메모리 저장
    println!("\n💾 메모리 저장 중...");
    
    let memories = vec![
        ("mem001", "lesson", "Paul은 반말 선호 친구처럼 대화", 5),
        ("mem002", "project", "CoreDB Rust로 만든 Cassandra 스타일 DB", 4),
        ("mem003", "project", "memory-brain 벡터 검색 망각 기능", 4),
        ("mem004", "preference", "Paul GitHub yhc007 주 언어 Rust", 3),
        ("mem005", "event", "2026-01-30 CoreDB Pekko Actor 통합 성공", 5),
    ];
    
    for (id, category, content, importance) in memories {
        let query = format!(
            "INSERT INTO sam.memories (id, category, content, importance) VALUES ('{}', '{}', '{}', {})",
            id, category, content, importance
        );
        match db.execute_cql(&query).await {
            Ok(_) => println!("  📝 저장: {} - {}", category, content.chars().take(20).collect::<String>()),
            Err(e) => println!("  ❌ 에러: {:?}", e),
        }
    }
    
    // 4. 메모리 조회
    println!("\n🔍 저장된 메모리 조회:");
    match db.execute_cql("SELECT * FROM sam.memories").await {
        Ok(result) => println!("{:?}", result),
        Err(e) => println!("❌ 조회 에러: {:?}", e),
    }
    
    // 5. 특정 메모리 조회
    println!("\n⭐ mem001 조회:");
    match db.execute_cql("SELECT * FROM sam.memories WHERE id = 'mem001'").await {
        Ok(result) => println!("{:?}", result),
        Err(e) => println!("⚠️ 에러: {:?}", e),
    }
    
    println!("\n✅ Sam CoreDB 메모리 테스트 완료! 🦊");
    
    Ok(())
}
