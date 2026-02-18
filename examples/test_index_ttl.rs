//! Secondary Index와 TTL 기능 테스트

use coredb::database::{CoreDB, DatabaseConfig};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 CoreDB Secondary Index & TTL 테스트\n");
    
    // 임시 디렉토리 사용
    let config = DatabaseConfig {
        data_directory: std::path::PathBuf::from("./test_data_features"),
        commitlog_directory: std::path::PathBuf::from("./test_commitlog_features"),
        ..Default::default()
    };
    
    let db = CoreDB::new(config).await?;
    
    // === 1. 키스페이스 & 테이블 생성 ===
    println!("📁 1. 키스페이스 & 테이블 생성");
    db.execute_cql("CREATE KEYSPACE demo WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await?;
    db.execute_cql("CREATE TABLE demo.users (id INT PRIMARY KEY, name TEXT, email TEXT)").await?;
    println!("   ✅ demo.users 테이블 생성 완료\n");
    
    // === 2. 데이터 삽입 ===
    println!("📝 2. 데이터 삽입");
    db.execute_cql("INSERT INTO demo.users (id, name, email) VALUES (1, 'John', 'john@example.com')").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, email) VALUES (2, 'Jane', 'jane@example.com')").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, email) VALUES (3, 'Bob', 'john@example.com')").await?;
    println!("   ✅ 3개 레코드 삽입 완료\n");
    
    // === 3. Secondary Index 생성 ===
    println!("📇 3. Secondary Index 생성");
    let result = db.execute_cql("CREATE INDEX idx_email ON demo.users (email)").await?;
    println!("   결과: {:?}", result);
    println!("   ✅ email 컬럼에 인덱스 생성 완료\n");
    
    // === 4. 인덱스 존재 확인 ===
    println!("🔍 4. 인덱스 확인");
    let has_index = db.can_use_index("demo", "users", "email");
    println!("   demo.users.email 인덱스 존재: {}", has_index);
    
    let indexes = db.list_indexes();
    println!("   전체 인덱스 목록: {:?}\n", indexes);
    
    // === 5. 일반 SELECT ===
    println!("📊 5. 일반 SELECT (PK 사용)");
    let result = db.execute_cql("SELECT * FROM demo.users WHERE id = 1").await?;
    println!("   결과: {:?}\n", result);
    
    // === 6. TTL 테스트 ===
    println!("⏰ 6. TTL 테스트");
    db.execute_cql("CREATE TABLE demo.cache (key INT PRIMARY KEY, value TEXT)").await?;
    
    // TTL 3초로 삽입
    db.execute_cql("INSERT INTO demo.cache (key, value) VALUES (1, 'temporary') USING TTL 3").await?;
    println!("   ✅ TTL 3초로 데이터 삽입");
    
    // 즉시 조회 - 데이터 있어야 함
    let result = db.execute_cql("SELECT * FROM demo.cache WHERE key = 1").await?;
    println!("   즉시 조회: {:?}", result);
    
    // 4초 대기
    println!("   ⏳ 4초 대기 중...");
    tokio::time::sleep(Duration::from_secs(4)).await;
    
    // 다시 조회 - TTL 만료로 데이터 없어야 함
    let result = db.execute_cql("SELECT * FROM demo.cache WHERE key = 1").await?;
    println!("   4초 후 조회: {:?}", result);
    
    // 결과 확인
    match result {
        coredb::query::QueryResult::Rows(rows) => {
            if rows.is_empty() {
                println!("   ✅ TTL 만료 확인! 행 전체가 삭제됨\n");
            } else if let Some(row) = rows.first() {
                // value 컬럼이 없으면 TTL 만료된 것
                if !row.columns.contains_key("value") {
                    println!("   ✅ TTL 만료 확인! value 셀이 만료됨 (PK는 유지)\n");
                } else {
                    println!("   ⚠️ TTL이 아직 적용되지 않음 - value가 여전히 존재\n");
                }
            }
        },
        _ => println!("   결과 형식 오류\n"),
    }
    
    // === 7. 정리 ===
    println!("🧹 7. 테스트 데이터 정리");
    std::fs::remove_dir_all("./test_data_features").ok();
    std::fs::remove_dir_all("./test_commitlog_features").ok();
    println!("   ✅ 완료\n");
    
    println!("🎉 모든 테스트 완료!");
    
    Ok(())
}
