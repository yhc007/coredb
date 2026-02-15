use coredb::{CoreDB, DatabaseConfig};
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = DatabaseConfig {
        data_directory: PathBuf::from("./bench_data"),
        commitlog_directory: PathBuf::from("./bench_commitlog"),
        memtable_flush_threshold_mb: 16,
        compaction_throughput_mb_per_sec: 16,
        concurrent_reads: 32,
        concurrent_writes: 32,
        ..Default::default()
    };
    
    let db = CoreDB::new(config).await?;
    
    // 키스페이스/테이블 생성
    db.execute_cql("CREATE KEYSPACE bench WITH REPLICATION = {'class': 'SimpleStrategy', 'replication_factor': 1}").await.ok();
    db.execute_cql("CREATE TABLE bench.test (id TEXT PRIMARY KEY, data TEXT)").await.ok();
    
    // 1000개 INSERT 벤치마크
    let start = Instant::now();
    for i in 0..1000 {
        let q = format!("INSERT INTO bench.test (id, data) VALUES ('id{}', 'data{}')", i, i);
        db.execute_cql(&q).await?;
    }
    let elapsed = start.elapsed();
    println!("1000 INSERTs: {:?} ({:.0} ops/sec)", elapsed, 1000.0 / elapsed.as_secs_f64());
    
    // 1000개 SELECT 벤치마크
    let start = Instant::now();
    for i in 0..1000 {
        let q = format!("SELECT * FROM bench.test WHERE id = 'id{}'", i);
        db.execute_cql(&q).await?;
    }
    let elapsed = start.elapsed();
    println!("1000 SELECTs: {:?} ({:.0} ops/sec)", elapsed, 1000.0 / elapsed.as_secs_f64());
    
    Ok(())
}
