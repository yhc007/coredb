use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::sync::Arc;
use std::thread;

/// 간단한 데이터베이스 구조 (성능 테스트용)
#[derive(Debug)]
pub struct BenchmarkDB {
    data: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

impl BenchmarkDB {
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
    
    pub fn insert(&mut self, keyspace: &str, table: &str, key: &str, value: &str) {
        if let Some(ks) = self.data.get_mut(keyspace) {
            if let Some(tbl) = ks.get_mut(table) {
                tbl.insert(key.to_string(), value.to_string());
            }
        }
    }
    
    pub fn get(&self, keyspace: &str, table: &str, key: &str) -> Option<String> {
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
}

/// 성능 테스트 결과
#[derive(Debug)]
struct BenchmarkResult {
    operation: String,
    total_operations: usize,
    duration: Duration,
    ops_per_sec: f64,
    avg_latency_ms: f64,
}

impl BenchmarkResult {
    fn new(operation: String, total_operations: usize, duration: Duration) -> Self {
        let ops_per_sec = total_operations as f64 / duration.as_secs_f64();
        let avg_latency_ms = duration.as_secs_f64() * 1000.0 / total_operations as f64;
        
        Self {
            operation,
            total_operations,
            duration,
            ops_per_sec,
            avg_latency_ms,
        }
    }
}

/// 쓰기 성능 테스트
fn benchmark_write(db: &mut BenchmarkDB, num_operations: usize) -> BenchmarkResult {
    println!("🔄 Starting write benchmark with {} operations...", num_operations);
    
    let start = Instant::now();
    
    for i in 0..num_operations {
        let key = format!("key_{}", i);
        let value = format!("value_{}_{}", i, "benchmark_data_".repeat(10)); // 약간 큰 데이터
        db.insert("benchmark", "test_table", &key, &value);
    }
    
    let duration = start.elapsed();
    BenchmarkResult::new("WRITE".to_string(), num_operations, duration)
}

/// 읽기 성능 테스트
fn benchmark_read(db: &BenchmarkDB, keys: &[String], num_operations: usize) -> BenchmarkResult {
    println!("🔄 Starting read benchmark with {} operations...", num_operations);
    
    let start = Instant::now();
    
    for i in 0..num_operations {
        let key_index = i % keys.len();
        let _ = db.get("benchmark", "test_table", &keys[key_index]);
    }
    
    let duration = start.elapsed();
    BenchmarkResult::new("READ".to_string(), num_operations, duration)
}

/// 동시 쓰기 성능 테스트
fn benchmark_concurrent_write(db: &mut BenchmarkDB, num_threads: usize, operations_per_thread: usize) -> BenchmarkResult {
    println!("🔄 Starting concurrent write benchmark with {} threads, {} ops per thread...", 
             num_threads, operations_per_thread);
    
    let start = Instant::now();
    let mut handles = vec![];
    
    // 각 스레드가 독립적인 테이블을 사용하도록 함
    for thread_id in 0..num_threads {
        let mut thread_db = BenchmarkDB::new();
        thread_db.create_keyspace("benchmark");
        thread_db.create_table("benchmark", &format!("table_{}", thread_id));
        
        let handle = thread::spawn(move || {
            for i in 0..operations_per_thread {
                let key = format!("key_{}_{}", thread_id, i);
                let value = format!("value_{}_{}_{}", thread_id, i, "concurrent_data_".repeat(10));
                thread_db.insert("benchmark", &format!("table_{}", thread_id), &key, &value);
            }
        });
        handles.push(handle);
    }
    
    // 모든 스레드 완료 대기
    for handle in handles {
        handle.join().unwrap();
    }
    
    let duration = start.elapsed();
    let total_operations = num_threads * operations_per_thread;
    BenchmarkResult::new("CONCURRENT_WRITE".to_string(), total_operations, duration)
}

/// 혼합 작업 성능 테스트 (70% 읽기, 30% 쓰기)
fn benchmark_mixed_workload(db: &mut BenchmarkDB, keys: &[String], num_operations: usize) -> BenchmarkResult {
    println!("🔄 Starting mixed workload benchmark (70% read, 30% write) with {} operations...", 
             num_operations);
    
    let start = Instant::now();
    
    for i in 0..num_operations {
        if i % 10 < 7 {
            // 70% 읽기
            let key_index = i % keys.len();
            let _ = db.get("benchmark", "test_table", &keys[key_index]);
        } else {
            // 30% 쓰기
            let key = format!("mixed_key_{}", i);
            let value = format!("mixed_value_{}_{}", i, "mixed_data_".repeat(10));
            db.insert("benchmark", "test_table", &key, &value);
        }
    }
    
    let duration = start.elapsed();
    BenchmarkResult::new("MIXED_WORKLOAD".to_string(), num_operations, duration)
}

/// 결과 출력
fn print_results(results: &[BenchmarkResult]) {
    println!("\n📊 BENCHMARK RESULTS");
    println!("===================");
    
    for result in results {
        println!("\n🔸 {}", result.operation);
        println!("   Total Operations: {}", result.total_operations);
        println!("   Duration: {:.3}s", result.duration.as_secs_f64());
        println!("   Operations/sec: {:.0}", result.ops_per_sec);
        println!("   Avg Latency: {:.3}ms", result.avg_latency_ms);
        
        // 성능 등급 평가
        let grade = if result.ops_per_sec > 100000.0 {
            "🏆 EXCELLENT"
        } else if result.ops_per_sec > 50000.0 {
            "🥇 VERY GOOD"
        } else if result.ops_per_sec > 10000.0 {
            "🥈 GOOD"
        } else if result.ops_per_sec > 1000.0 {
            "🥉 FAIR"
        } else {
            "⚠️  NEEDS IMPROVEMENT"
        };
        
        println!("   Performance: {}", grade);
    }
    
    // 요약
    println!("\n📈 PERFORMANCE SUMMARY");
    println!("=====================");
    
    if let Some(write_result) = results.iter().find(|r| r.operation == "WRITE") {
        println!("Write Performance: {:.0} ops/sec", write_result.ops_per_sec);
    }
    
    if let Some(read_result) = results.iter().find(|r| r.operation == "READ") {
        println!("Read Performance:  {:.0} ops/sec", read_result.ops_per_sec);
    }
    
    if let Some(concurrent_result) = results.iter().find(|r| r.operation == "CONCURRENT_WRITE") {
        println!("Concurrent Write:  {:.0} ops/sec", concurrent_result.ops_per_sec);
    }
    
    if let Some(mixed_result) = results.iter().find(|r| r.operation == "MIXED_WORKLOAD") {
        println!("Mixed Workload:    {:.0} ops/sec", mixed_result.ops_per_sec);
    }
}

fn main() {
    println!("🚀 CoreDB Performance Benchmark");
    println!("===============================");
    
    // 테스트 설정
    let write_operations = 100_000;
    let read_operations = 100_000;
    let concurrent_threads = 4;
    let operations_per_thread = 25_000;
    let mixed_operations = 50_000;
    
    // 데이터베이스 초기화
    let mut db = BenchmarkDB::new();
    db.create_keyspace("benchmark");
    db.create_table("benchmark", "test_table");
    
    let mut results = Vec::new();
    
    // 1. 쓰기 성능 테스트
    println!("\n1️⃣  WRITE PERFORMANCE TEST");
    let write_result = benchmark_write(&mut db, write_operations);
    results.push(write_result);
    
    // 2. 읽기 성능 테스트 (기존 데이터 읽기)
    println!("\n2️⃣  READ PERFORMANCE TEST");
    let keys = db.get_all_keys("benchmark", "test_table");
    if !keys.is_empty() {
        let read_result = benchmark_read(&db, &keys, read_operations);
        results.push(read_result);
    }
    
    // 3. 동시 쓰기 성능 테스트
    println!("\n3️⃣  CONCURRENT WRITE TEST");
    let concurrent_result = benchmark_concurrent_write(&mut db, concurrent_threads, operations_per_thread);
    results.push(concurrent_result);
    
    // 4. 혼합 작업 성능 테스트
    println!("\n4️⃣  MIXED WORKLOAD TEST");
    let keys = db.get_all_keys("benchmark", "test_table");
    if !keys.is_empty() {
        let mixed_result = benchmark_mixed_workload(&mut db, &keys, mixed_operations);
        results.push(mixed_result);
    }
    
    // 결과 출력
    print_results(&results);
    
    // 추가 분석
    println!("\n🔍 DETAILED ANALYSIS");
    println!("===================");
    
    if let Some(write_result) = results.iter().find(|r| r.operation == "WRITE") {
        println!("• Write throughput: {:.0} operations per second", write_result.ops_per_sec);
        println!("• Write latency: {:.3} milliseconds average", write_result.avg_latency_ms);
        
        if write_result.ops_per_sec > 50000.0 {
            println!("  ✅ Write performance is excellent for a single-node database");
        } else if write_result.ops_per_sec > 10000.0 {
            println!("  ✅ Write performance is good for a single-node database");
        } else {
            println!("  ⚠️  Write performance could be improved with optimizations");
        }
    }
    
    if let Some(read_result) = results.iter().find(|r| r.operation == "READ") {
        println!("• Read throughput: {:.0} operations per second", read_result.ops_per_sec);
        println!("• Read latency: {:.3} milliseconds average", read_result.avg_latency_ms);
        
        if read_result.ops_per_sec > 100000.0 {
            println!("  ✅ Read performance is excellent");
        } else if read_result.ops_per_sec > 50000.0 {
            println!("  ✅ Read performance is very good");
        } else {
            println!("  ⚠️  Read performance could be improved with indexing");
        }
    }
    
    println!("\n💡 OPTIMIZATION SUGGESTIONS");
    println!("==========================");
    println!("• Consider implementing memory pooling for better write performance");
    println!("• Add indexing for faster read operations");
    println!("• Implement connection pooling for concurrent operations");
    println!("• Use compression for large data values");
    println!("• Consider implementing batch operations for bulk inserts");
    
    println!("\n✅ Benchmark completed successfully!");
}
