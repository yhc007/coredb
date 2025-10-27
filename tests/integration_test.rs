use coredb::*;
use std::path::PathBuf;

#[tokio::test]
async fn test_database_lifecycle() {
    // 테스트 디렉토리 생성
    let test_dir = PathBuf::from("./test_integration_db");
    let config = DatabaseConfig {
        data_directory: test_dir.clone(),
        commitlog_directory: test_dir.join("commitlog"),
        memtable_flush_threshold_mb: 64,
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
    };
    
    // 1. 데이터베이스 생성
    let db = CoreDB::new(config).await.expect("Failed to create database");
    
    // 2. 키스페이스 생성
    db.create_keyspace("test_ks".to_string(), 1)
        .await
        .expect("Failed to create keyspace");
    
    // 3. 데이터베이스 통계 확인
    let stats = db.get_stats().await;
    assert!(stats.keyspace_count >= 1); // system keyspace + test_ks
    
    // 정리
    std::fs::remove_dir_all(&test_dir).ok();
}

#[tokio::test]
async fn test_persistence_save_load() {
    // 1. 데이터베이스 생성 및 저장
    let test_dir = PathBuf::from("./test_persistence_db");
    let config = DatabaseConfig {
        data_directory: test_dir.clone(),
        commitlog_directory: test_dir.join("commitlog"),
        ..Default::default()
    };
    
    let db = CoreDB::new(config).await.expect("Failed to create database");
    db.create_keyspace("persistent_ks".to_string(), 1)
        .await
        .expect("Failed to create keyspace");
    
    // 저장
    db.save_to_disk().await.expect("Failed to save");
    
    // 2. 저장된 파일 확인
    let snapshot_path = test_dir.join("db_snapshot.txt");
    assert!(snapshot_path.exists(), "Snapshot file should exist");
    
    // 정리
    std::fs::remove_dir_all(&test_dir).ok();
}

#[test]
fn test_snapshot_functionality() {
    use coredb::persistence::Snapshot;
    
    let test_dir = "./test_snapshot".to_string();
    let snapshot = Snapshot::new(test_dir.clone());
    
    // 데이터 저장
    let data = "TEST_KEYSPACE:demo\nTEST_TABLE:users";
    snapshot.save_text(data).expect("Failed to save");
    
    // 데이터 로드
    let loaded = snapshot.load_text().expect("Failed to load");
    assert_eq!(loaded, data);
    
    // WAL 테스트
    snapshot.write_wal("INSERT demo.users 1").expect("Failed to write WAL");
    snapshot.write_wal("INSERT demo.users 2").expect("Failed to write WAL");
    
    let wal_ops = snapshot.read_wal().expect("Failed to read WAL");
    assert_eq!(wal_ops.len(), 2);
    
    // 정리
    std::fs::remove_dir_all(&test_dir).ok();
}

