//! 백업 & 복원 테스트

use coredb::database::{CoreDB, DatabaseConfig};
use coredb::persistence::backup::BackupFormat;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 CoreDB 백업 & 복원 테스트\n");
    
    // === 1. 원본 데이터베이스 생성 ===
    println!("📁 1. 원본 데이터베이스 생성");
    let config = DatabaseConfig {
        data_directory: std::path::PathBuf::from("./test_data_backup_src"),
        commitlog_directory: std::path::PathBuf::from("./test_commitlog_backup_src"),
        ..Default::default()
    };
    
    let db = CoreDB::new(config).await?;
    
    // 키스페이스 & 테이블 생성
    db.execute_cql("CREATE KEYSPACE myapp WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await?;
    db.execute_cql("CREATE TABLE myapp.users (id INT PRIMARY KEY, name TEXT, email TEXT)").await?;
    db.execute_cql("CREATE TABLE myapp.orders (id INT PRIMARY KEY, user_id INT, amount INT)").await?;
    
    // 데이터 삽입
    db.execute_cql("INSERT INTO myapp.users (id, name, email) VALUES (1, 'Alice', 'alice@example.com')").await?;
    db.execute_cql("INSERT INTO myapp.users (id, name, email) VALUES (2, 'Bob', 'bob@example.com')").await?;
    db.execute_cql("INSERT INTO myapp.users (id, name, email) VALUES (3, 'Charlie', 'charlie@example.com')").await?;
    
    db.execute_cql("INSERT INTO myapp.orders (id, user_id, amount) VALUES (101, 1, 5000)").await?;
    db.execute_cql("INSERT INTO myapp.orders (id, user_id, amount) VALUES (102, 2, 3000)").await?;
    
    // 인덱스 생성
    db.execute_cql("CREATE INDEX idx_email ON myapp.users (email)").await?;
    
    println!("   ✅ 2개 테이블, 5개 행, 1개 인덱스 생성 완료\n");
    
    // === 2. 백업 생성 ===
    println!("💾 2. 백업 생성");
    
    // JSON 백업 (사람이 읽을 수 있음)
    let json_path = db.create_backup("./backups", "myapp_backup", BackupFormat::JsonPretty).await?;
    println!("   JSON 백업: {:?}", json_path);
    
    // Binary 백업 (작고 빠름)
    let bin_path = db.create_backup("./backups", "myapp_backup", BackupFormat::Binary).await?;
    println!("   Binary 백업: {:?}\n", bin_path);
    
    // === 3. 백업 목록 확인 ===
    println!("📋 3. 백업 목록");
    let backups = db.list_backups("./backups")?;
    for backup in &backups {
        println!("   - {} ({} bytes)", backup.name, backup.size);
    }
    println!();
    
    // === 4. 새 데이터베이스에 복원 ===
    println!("🔄 4. 새 데이터베이스에 복원");
    
    let config2 = DatabaseConfig {
        data_directory: std::path::PathBuf::from("./test_data_backup_dst"),
        commitlog_directory: std::path::PathBuf::from("./test_commitlog_backup_dst"),
        ..Default::default()
    };
    
    let db2 = CoreDB::new(config2).await?;
    
    let result = db2.restore_from_backup(json_path.to_str().unwrap()).await?;
    println!("   복원 결과:");
    println!("     - 키스페이스: {} 개", result.keyspaces);
    println!("     - 테이블: {} 개", result.tables);
    println!("     - 행: {} 개", result.rows);
    println!("     - 인덱스: {} 개\n", result.indexes);
    
    // === 5. 복원된 데이터 확인 ===
    println!("✅ 5. 복원된 데이터 확인");
    
    let result = db2.execute_cql("SELECT COUNT(*) FROM myapp.users").await?;
    println!("   users COUNT(*): {:?}", result);
    
    let result = db2.execute_cql("SELECT COUNT(*) FROM myapp.orders").await?;
    println!("   orders COUNT(*): {:?}", result);
    
    let result = db2.execute_cql("SELECT * FROM myapp.users WHERE id = 1").await?;
    println!("   SELECT id=1: {:?}", result);
    
    // 인덱스 확인
    let has_index = db2.can_use_index("myapp", "users", "email");
    println!("   email 인덱스 복원됨: {}\n", has_index);
    
    // === 6. 정리 ===
    println!("🧹 6. 테스트 데이터 정리");
    std::fs::remove_dir_all("./test_data_backup_src").ok();
    std::fs::remove_dir_all("./test_commitlog_backup_src").ok();
    std::fs::remove_dir_all("./test_data_backup_dst").ok();
    std::fs::remove_dir_all("./test_commitlog_backup_dst").ok();
    std::fs::remove_dir_all("./backups").ok();
    println!("   ✅ 완료\n");
    
    println!("🎉 백업 & 복원 테스트 완료!");
    
    Ok(())
}
