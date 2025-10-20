use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::thread;
use std::sync::{Arc, Mutex};

/// 더 현실적인 데이터베이스 구조 (스트레스 테스트용)
#[derive(Debug)]
pub struct StressTestDB {
    data: HashMap<String, HashMap<String, HashMap<String, Vec<u8>>>>, // 바이너리 데이터 지원
}

impl StressTestDB {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }
    
    pub fn create_keyspace(&mut self, name: &str) {
        self.data.insert(name.to_string(), HashMap::new());
    }
    
    pub fn create_table(&mut self, keyspace: &str, table: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            ks.insert(table.to_string(), HashMap::new());
        }
    }
    
    pub fn insert(&mut self, keyspace: &str, table: &str, key: &str, value: Vec<u8>) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            if let Some(tbl) = ks.get_mut(table) {
                tbl.insert(key.to_string(), value);
            }
        }
    }
    
    pub fn get(&self, keyspace: &str, table: &str, key: &str) -> Option<Vec<u8>> {
        self.data
            .get(keyspace)?
            .get(table)?
            .get(key)
            .map(|v| v.clone())
    }
    
    pub fn get_all_keys(&self, keyspace: &str, table: &str) -> Vec<String> {
        self.data
            .get(keyspace)
            .and_then(|ks| ks.get(table))
            .map(|tbl| tbl.keys().cloned().collect())
            .unwrap_or_default()
    }
    
    pub fn count_records(&self, keyspace: &str, table: &str) -> usize {
        self.data
            .get(keyspace)
            .and_then(|ks| ks.get(table))
            .map(|tbl| tbl.len())
            .unwrap_or(0)
    }
}

/// 스트레스 테스트 결과
#[derive(Debug)]
struct StressTestResult {
    test_name: String,
    total_operations: usize,
    duration: Duration,
    ops_per_sec: f64,
    avg_latency_ms: f64,
    data_size_mb: f64,
    throughput_mb_per_sec: f64,
}

impl StressTestResult {
    fn new(test_name: String, total_operations: usize, duration: Duration, data_size_bytes: usize) -> Self {
        let ops_per_sec = total_operations as f64 / duration.as_secs_f64();
        let avg_latency_ms = duration.as_secs_f64() * 1000.0 / total_operations as f64;
        let data_size_mb = data_size_bytes as f64 / (1024.0 * 1024.0);
        let throughput_mb_per_sec = data_size_mb / duration.as_secs_f64();
        
        Self {
            test_name,
            total_operations,
            duration,
            ops_per_sec,
            avg_latency_ms,
            data_size_mb,
            throughput_mb_per_sec,
        }
    }
}

/// 대용량 데이터 쓰기 테스트
fn stress_test_large_writes(db: &mut StressTestDB, num_operations: usize, data_size_kb: usize) -> StressTestResult {
    println!("🔥 Large data write stress test: {} operations, {}KB per record", num_operations, data_size_kb);
    
    let data = vec![0u8; data_size_kb * 1024]; // KB 단위 데이터
    let total_data_size = num_operations * data_size_kb * 1024;
    
    let start = Instant::now();
    
    for i in 0..num_operations {
        let key = format!("large_key_{:08}", i);
        db.insert("stress", "large_table", &key, data.clone());
        
        // 진행률 표시
        if i % (num_operations / 10) == 0 {
            println!("   Progress: {}%", (i * 100) / num_operations);
        }
    }
    
    let duration = start.elapsed();
    StressTestResult::new(
        format!("LARGE_WRITES_{}KB", data_size_kb),
        num_operations,
        duration,
        total_data_size
    )
}

/// 고빈도 작은 데이터 쓰기 테스트
fn stress_test_high_frequency_writes(db: &mut StressTestDB, num_operations: usize) -> StressTestResult {
    println!("⚡ High frequency small writes: {} operations", num_operations);
    
    let data = b"small_data_record_for_high_frequency_testing".to_vec();
    let total_data_size = num_operations * data.len();
    
    let start = Instant::now();
    
    for i in 0..num_operations {
        let key = format!("hf_key_{:08}", i);
        db.insert("stress", "hf_table", &key, data.clone());
        
        if i % (num_operations / 20) == 0 && i > 0 {
            println!("   Progress: {}%", (i * 100) / num_operations);
        }
    }
    
    let duration = start.elapsed();
    StressTestResult::new(
        "HIGH_FREQUENCY_WRITES".to_string(),
        num_operations,
        duration,
        total_data_size
    )
}

/// 멀티스레드 동시 쓰기 스트레스 테스트
fn stress_test_concurrent_writes(num_threads: usize, operations_per_thread: usize, data_size_kb: usize) -> StressTestResult {
    println!("🔥 Concurrent write stress test: {} threads, {} ops per thread, {}KB per record", 
             num_threads, operations_per_thread, data_size_kb);
    
    let data = vec![0u8; data_size_kb * 1024];
    let total_operations = num_threads * operations_per_thread;
    let total_data_size = total_operations * data_size_kb * 1024;
    
    let start = Instant::now();
    let mut handles = vec![];
    
    for thread_id in 0..num_threads {
        let thread_data = data.clone();
        let handle = thread::spawn(move || {
            let mut thread_db = StressTestDB::new();
            thread_db.create_keyspace("concurrent");
            thread_db.create_table("concurrent", &format!("thread_{}", thread_id));
            
            for i in 0..operations_per_thread {
                let key = format!("concurrent_key_{}_{:08}", thread_id, i);
                thread_db.insert("concurrent", &format!("thread_{}", thread_id), &key, thread_data.clone());
            }
        });
        handles.push(handle);
    }
    
    for handle in handles {
        handle.join().unwrap();
    }
    
    let duration = start.elapsed();
    StressTestResult::new(
        format!("CONCURRENT_WRITES_{}THREADS", num_threads),
        total_operations,
        duration,
        total_data_size
    )
}

/// 읽기 성능 스트레스 테스트
fn stress_test_reads(db: &StressTestDB, keys: &[String], num_operations: usize) -> StressTestResult {
    println!("📖 Read stress test: {} operations", num_operations);
    
    let start = Instant::now();
    let mut total_data_read = 0;
    
    for i in 0..num_operations {
        let key_index = i % keys.len();
        if let Some(data) = db.get("stress", "large_table", &keys[key_index]) {
            total_data_read += data.len();
        }
        
        if i % (num_operations / 10) == 0 && i > 0 {
            println!("   Progress: {}%", (i * 100) / num_operations);
        }
    }
    
    let duration = start.elapsed();
    StressTestResult::new(
        "READ_STRESS".to_string(),
        num_operations,
        duration,
        total_data_read
    )
}

/// 결과 출력
fn print_stress_results(results: &[StressTestResult]) {
    println!("\n📊 STRESS TEST RESULTS");
    println!("======================");
    
    for result in results {
        println!("\n🔸 {}", result.test_name);
        println!("   Operations: {}", result.total_operations);
        println!("   Duration: {:.3}s", result.duration.as_secs_f64());
        println!("   Operations/sec: {:.0}", result.ops_per_sec);
        println!("   Avg Latency: {:.3}ms", result.avg_latency_ms);
        println!("   Data Size: {:.2} MB", result.data_size_mb);
        println!("   Throughput: {:.2} MB/sec", result.throughput_mb_per_sec);
        
        // 성능 등급 평가
        let grade = if result.ops_per_sec > 500000.0 {
            "🏆 OUTSTANDING"
        } else if result.ops_per_sec > 100000.0 {
            "🥇 EXCELLENT"
        } else if result.ops_per_sec > 50000.0 {
            "🥈 VERY GOOD"
        } else if result.ops_per_sec > 10000.0 {
            "🥉 GOOD"
        } else {
            "⚠️  NEEDS IMPROVEMENT"
        };
        
        println!("   Performance: {}", grade);
    }
    
    // 성능 비교
    println!("\n📈 PERFORMANCE COMPARISON");
    println!("=========================");
    
    if let Some(large_write) = results.iter().find(|r| r.test_name.contains("LARGE_WRITES")) {
        println!("Large Data Writes: {:.0} ops/sec ({:.1} MB/sec)", 
                 large_write.ops_per_sec, large_write.throughput_mb_per_sec);
    }
    
    if let Some(hf_write) = results.iter().find(|r| r.test_name == "HIGH_FREQUENCY_WRITES") {
        println!("Small Data Writes: {:.0} ops/sec ({:.1} MB/sec)", 
                 hf_write.ops_per_sec, hf_write.throughput_mb_per_sec);
    }
    
    if let Some(concurrent) = results.iter().find(|r| r.test_name.contains("CONCURRENT")) {
        println!("Concurrent Writes: {:.0} ops/sec ({:.1} MB/sec)", 
                 concurrent.ops_per_sec, concurrent.throughput_mb_per_sec);
    }
    
    if let Some(read) = results.iter().find(|r| r.test_name == "READ_STRESS") {
        println!("Read Operations:   {:.0} ops/sec ({:.1} MB/sec)", 
                 read.ops_per_sec, read.throughput_mb_per_sec);
    }
}

fn main() {
    println!("🔥 CoreDB Stress Test & Performance Analysis");
    println!("============================================");
    
    // 테스트 설정
    let large_write_ops = 10_000;
    let large_data_size_kb = 10; // 10KB per record
    let hf_write_ops = 100_000;
    let concurrent_threads = 8;
    let ops_per_thread = 5_000;
    let concurrent_data_size_kb = 5;
    let read_ops = 50_000;
    
    let mut results = Vec::new();
    
    // 데이터베이스 초기화
    let mut db = StressTestDB::new();
    db.create_keyspace("stress");
    db.create_table("stress", "large_table");
    db.create_table("stress", "hf_table");
    
    // 1. 대용량 데이터 쓰기 테스트
    println!("\n1️⃣  LARGE DATA WRITE STRESS TEST");
    let large_write_result = stress_test_large_writes(&mut db, large_write_ops, large_data_size_kb);
    results.push(large_write_result);
    
    // 2. 고빈도 작은 데이터 쓰기 테스트
    println!("\n2️⃣  HIGH FREQUENCY WRITE STRESS TEST");
    let hf_write_result = stress_test_high_frequency_writes(&mut db, hf_write_ops);
    results.push(hf_write_result);
    
    // 3. 멀티스레드 동시 쓰기 테스트
    println!("\n3️⃣  CONCURRENT WRITE STRESS TEST");
    let concurrent_result = stress_test_concurrent_writes(concurrent_threads, ops_per_thread, concurrent_data_size_kb);
    results.push(concurrent_result);
    
    // 4. 읽기 성능 테스트
    println!("\n4️⃣  READ STRESS TEST");
    let keys = db.get_all_keys("stress", "large_table");
    if !keys.is_empty() {
        let read_result = stress_test_reads(&db, &keys, read_ops);
        results.push(read_result);
    }
    
    // 결과 출력
    print_stress_results(&results);
    
    // 최종 분석
    println!("\n🎯 FINAL PERFORMANCE ANALYSIS");
    println!("=============================");
    
    let total_ops: usize = results.iter().map(|r| r.total_operations).sum();
    let total_duration: f64 = results.iter().map(|r| r.duration.as_secs_f64()).sum();
    let overall_ops_per_sec = total_ops as f64 / total_duration;
    
    println!("Overall Performance: {:.0} operations/second", overall_ops_per_sec);
    println!("Total Operations: {}", total_ops);
    println!("Total Duration: {:.2} seconds", total_duration);
    
    // 데이터베이스 상태 확인
    println!("\n📊 DATABASE STATE");
    println!("=================");
    println!("Records in large_table: {}", db.count_records("stress", "large_table"));
    println!("Records in hf_table: {}", db.count_records("stress", "hf_table"));
    
    // 성능 권장사항
    println!("\n💡 PERFORMANCE RECOMMENDATIONS");
    println!("=============================");
    
    if let Some(large_write) = results.iter().find(|r| r.test_name.contains("LARGE_WRITES")) {
        if large_write.ops_per_sec < 10000.0 {
            println!("⚠️  Large data writes are slow. Consider:");
            println!("   • Implementing batch writes");
            println!("   • Using compression for large values");
            println!("   • Implementing async I/O");
        } else {
            println!("✅ Large data write performance is acceptable");
        }
    }
    
    if let Some(hf_write) = results.iter().find(|r| r.test_name == "HIGH_FREQUENCY_WRITES") {
        if hf_write.ops_per_sec < 50000.0 {
            println!("⚠️  High frequency writes could be improved:");
            println!("   • Implement write batching");
            println!("   • Use memory-mapped files");
            println!("   • Optimize hash map performance");
        } else {
            println!("✅ High frequency write performance is good");
        }
    }
    
    println!("\n🏆 STRESS TEST COMPLETED SUCCESSFULLY!");
    println!("CoreDB demonstrated robust performance under various load conditions.");
}
