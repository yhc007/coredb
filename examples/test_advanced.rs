//! Aggregations, BATCH, Lightweight Transactions 테스트

use coredb::database::{CoreDB, DatabaseConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 CoreDB 고급 기능 테스트\n");
    
    let config = DatabaseConfig {
        data_directory: std::path::PathBuf::from("./test_data_advanced"),
        commitlog_directory: std::path::PathBuf::from("./test_commitlog_advanced"),
        ..Default::default()
    };
    
    let db = CoreDB::new(config).await?;
    
    // === 1. 테이블 생성 ===
    println!("📁 1. 테이블 생성");
    db.execute_cql("CREATE KEYSPACE demo WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await?;
    db.execute_cql("CREATE TABLE demo.users (id INT PRIMARY KEY, name TEXT, age INT)").await?;
    db.execute_cql("CREATE TABLE demo.accounts (id INT PRIMARY KEY, balance INT)").await?;
    println!("   ✅ 테이블 생성 완료\n");
    
    // === 2. 데이터 삽입 ===
    println!("📝 2. 데이터 삽입");
    db.execute_cql("INSERT INTO demo.users (id, name, age) VALUES (1, 'Alice', 25)").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, age) VALUES (2, 'Bob', 30)").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, age) VALUES (3, 'Charlie', 35)").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, age) VALUES (4, 'Diana', 28)").await?;
    db.execute_cql("INSERT INTO demo.users (id, name, age) VALUES (5, 'Eve', 22)").await?;
    println!("   ✅ 5명 삽입 완료\n");
    
    // === 3. Aggregations 테스트 ===
    println!("📊 3. Aggregations 테스트");
    
    // COUNT(*)
    let result = db.execute_cql("SELECT COUNT(*) FROM demo.users").await?;
    println!("   COUNT(*): {:?}", result);
    
    // SUM(age)
    let result = db.execute_cql("SELECT SUM(age) FROM demo.users").await?;
    println!("   SUM(age): {:?}", result);
    
    // AVG(age)
    let result = db.execute_cql("SELECT AVG(age) FROM demo.users").await?;
    println!("   AVG(age): {:?}", result);
    
    // MIN/MAX
    let result = db.execute_cql("SELECT MIN(age), MAX(age) FROM demo.users").await?;
    println!("   MIN/MAX(age): {:?}\n", result);
    
    // === 4. IF NOT EXISTS 테스트 ===
    println!("🔒 4. IF NOT EXISTS 테스트");
    
    // 첫 번째 삽입 - 성공해야 함
    let result = db.execute_cql("INSERT INTO demo.accounts (id, balance) VALUES (1, 1000) IF NOT EXISTS").await?;
    println!("   첫 번째 삽입: {:?}", result);
    
    // 두 번째 삽입 - 실패해야 함 (이미 존재)
    let result = db.execute_cql("INSERT INTO demo.accounts (id, balance) VALUES (1, 2000) IF NOT EXISTS").await?;
    println!("   두 번째 삽입 (중복): {:?}\n", result);
    
    // === 5. UPDATE with IF 테스트 ===
    println!("🔄 5. UPDATE with IF 테스트");
    
    // 조건 충족 - balance가 1000이면 업데이트
    let result = db.execute_cql("UPDATE demo.accounts SET balance = 1500 WHERE id = 1 IF balance = 1000").await?;
    println!("   UPDATE IF balance=1000: {:?}", result);
    
    // 조건 불충족 - balance가 이제 1500이므로 실패
    let result = db.execute_cql("UPDATE demo.accounts SET balance = 2000 WHERE id = 1 IF balance = 1000").await?;
    println!("   UPDATE IF balance=1000 (불충족): {:?}\n", result);
    
    // === 6. BATCH 테스트 ===
    println!("📦 6. BATCH 테스트");
    let batch_query = r#"
        BEGIN BATCH
            INSERT INTO demo.users (id, name, age) VALUES (10, 'BatchUser1', 40);
            INSERT INTO demo.users (id, name, age) VALUES (11, 'BatchUser2', 41);
            INSERT INTO demo.users (id, name, age) VALUES (12, 'BatchUser3', 42)
        APPLY BATCH
    "#;
    let result = db.execute_cql(batch_query).await?;
    println!("   BATCH 결과: {:?}", result);
    
    // BATCH 확인
    let result = db.execute_cql("SELECT COUNT(*) FROM demo.users").await?;
    println!("   BATCH 후 COUNT(*): {:?}\n", result);
    
    // === 7. DELETE 테스트 ===
    println!("🗑️ 7. DELETE 테스트");
    let result = db.execute_cql("DELETE FROM demo.users WHERE id = 5").await?;
    println!("   DELETE id=5: {:?}", result);
    
    let result = db.execute_cql("SELECT COUNT(*) FROM demo.users").await?;
    println!("   DELETE 후 COUNT(*): {:?}\n", result);
    
    // === 정리 ===
    println!("🧹 8. 테스트 데이터 정리");
    std::fs::remove_dir_all("./test_data_advanced").ok();
    std::fs::remove_dir_all("./test_commitlog_advanced").ok();
    println!("   ✅ 완료\n");
    
    println!("🎉 모든 고급 기능 테스트 완료!");
    
    Ok(())
}
