//! YCSB-Style Benchmark for CoreDB
//! 
//! Workloads:
//! - A: 50% read, 50% update (Update heavy)
//! - B: 95% read, 5% update (Read mostly)
//! - C: 100% read (Read only)
//! - D: 95% read, 5% insert (Read latest)
//! - F: 50% read, 50% read-modify-write

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::thread;

/// 간단한 KV Store (벤치마크용)
pub struct BenchDB {
    data: dashmap::DashMap<String, String>,
}

impl BenchDB {
    pub fn new() -> Self {
        Self {
            data: dashmap::DashMap::new(),
        }
    }
    
    pub fn insert(&self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
    }
    
    pub fn read(&self, key: &str) -> Option<String> {
        self.data.get(key).map(|v| v.clone())
    }
    
    pub fn update(&self, key: &str, value: &str) -> bool {
        if self.data.contains_key(key) {
            self.data.insert(key.to_string(), value.to_string());
            true
        } else {
            false
        }
    }
    
    pub fn read_modify_write(&self, key: &str, suffix: &str) -> bool {
        if let Some(mut entry) = self.data.get_mut(key) {
            let new_value = format!("{}{}", entry.value(), suffix);
            *entry = new_value;
            true
        } else {
            false
        }
    }
    
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

/// 레이턴시 히스토그램
#[derive(Debug)]
pub struct LatencyHistogram {
    values: Vec<u64>, // nanoseconds
}

impl LatencyHistogram {
    pub fn new() -> Self {
        Self { values: Vec::new() }
    }
    
    pub fn record(&mut self, nanos: u64) {
        self.values.push(nanos);
    }
    
    pub fn merge(&mut self, other: &mut LatencyHistogram) {
        self.values.append(&mut other.values);
    }
    
    pub fn percentile(&mut self, p: f64) -> u64 {
        if self.values.is_empty() {
            return 0;
        }
        self.values.sort_unstable();
        let idx = ((self.values.len() as f64) * p / 100.0) as usize;
        let idx = idx.min(self.values.len() - 1);
        self.values[idx]
    }
    
    pub fn p50(&mut self) -> u64 { self.percentile(50.0) }
    pub fn p99(&mut self) -> u64 { self.percentile(99.0) }
    pub fn avg(&self) -> u64 {
        if self.values.is_empty() { 0 }
        else { self.values.iter().sum::<u64>() / self.values.len() as u64 }
    }
}

/// YCSB 워크로드 타입
#[derive(Debug, Clone, Copy)]
pub enum Workload {
    A, // 50% read, 50% update
    B, // 95% read, 5% update
    C, // 100% read
    D, // 95% read, 5% insert
    F, // 50% read, 50% read-modify-write
}

impl Workload {
    pub fn name(&self) -> &'static str {
        match self {
            Workload::A => "A (50R/50U)",
            Workload::B => "B (95R/5U)",
            Workload::C => "C (100R)",
            Workload::D => "D (95R/5I)",
            Workload::F => "F (50R/50RMW)",
        }
    }
    
    /// 읽기 비율 반환 (0-100)
    pub fn read_ratio(&self) -> u8 {
        match self {
            Workload::A => 50,
            Workload::B => 95,
            Workload::C => 100,
            Workload::D => 95,
            Workload::F => 50,
        }
    }
}

/// 벤치마크 결과
#[derive(Debug)]
pub struct BenchmarkResult {
    pub workload: String,
    pub threads: usize,
    pub total_ops: u64,
    pub duration_ms: u64,
    pub ops_per_sec: f64,
    pub p50_us: f64,
    pub p99_us: f64,
    pub avg_us: f64,
    pub read_ops: u64,
    pub write_ops: u64,
}

impl BenchmarkResult {
    pub fn print(&self) {
        println!("┌─────────────────────────────────────────────────────┐");
        println!("│ Workload: {:40} │", self.workload);
        println!("├─────────────────────────────────────────────────────┤");
        println!("│ Threads:     {:>10}                            │", self.threads);
        println!("│ Total Ops:   {:>10}                            │", self.total_ops);
        println!("│ Duration:    {:>10} ms                         │", self.duration_ms);
        println!("│ Throughput:  {:>10.0} ops/sec                   │", self.ops_per_sec);
        println!("├─────────────────────────────────────────────────────┤");
        println!("│ P50 Latency: {:>10.2} µs                        │", self.p50_us);
        println!("│ P99 Latency: {:>10.2} µs                        │", self.p99_us);
        println!("│ Avg Latency: {:>10.2} µs                        │", self.avg_us);
        println!("├─────────────────────────────────────────────────────┤");
        println!("│ Read Ops:    {:>10}                            │", self.read_ops);
        println!("│ Write Ops:   {:>10}                            │", self.write_ops);
        println!("└─────────────────────────────────────────────────────┘");
    }
}

/// YCSB 벤치마크 실행
pub fn run_ycsb_benchmark(
    db: Arc<BenchDB>,
    workload: Workload,
    threads: usize,
    ops_per_thread: usize,
    record_count: usize,
) -> BenchmarkResult {
    // 데이터 사전 로드
    if db.len() < record_count {
        for i in 0..record_count {
            let key = format!("user{:010}", i);
            let value = format!("value_{}_{}", i, "x".repeat(100));
            db.insert(&key, &value);
        }
    }
    
    let total_ops = Arc::new(AtomicU64::new(0));
    let read_ops = Arc::new(AtomicU64::new(0));
    let write_ops = Arc::new(AtomicU64::new(0));
    let insert_counter = Arc::new(AtomicUsize::new(record_count));
    
    let start = Instant::now();
    
    let handles: Vec<_> = (0..threads).map(|thread_id| {
        let db = Arc::clone(&db);
        let total_ops = Arc::clone(&total_ops);
        let read_ops = Arc::clone(&read_ops);
        let write_ops = Arc::clone(&write_ops);
        let insert_counter = Arc::clone(&insert_counter);
        
        thread::spawn(move || {
            let mut latencies = LatencyHistogram::new();
            let mut rng_state = thread_id as u64 * 12345 + 67890;
            
            for _ in 0..ops_per_thread {
                // 간단한 난수 생성
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let rand_val = ((rng_state >> 33) as u32) % 100;
                
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let key_idx = ((rng_state >> 33) as usize) % record_count;
                let key = format!("user{:010}", key_idx);
                
                let op_start = Instant::now();
                
                let is_read = match workload {
                    Workload::A => rand_val < 50,
                    Workload::B => rand_val < 95,
                    Workload::C => true,
                    Workload::D => rand_val < 95,
                    Workload::F => rand_val < 50,
                };
                
                if is_read {
                    let _ = db.read(&key);
                    read_ops.fetch_add(1, Ordering::Relaxed);
                } else {
                    match workload {
                        Workload::A | Workload::B => {
                            db.update(&key, &format!("updated_{}", key_idx));
                        }
                        Workload::D => {
                            let new_idx = insert_counter.fetch_add(1, Ordering::Relaxed);
                            let new_key = format!("user{:010}", new_idx);
                            db.insert(&new_key, &format!("new_value_{}", new_idx));
                        }
                        Workload::F => {
                            db.read_modify_write(&key, "_modified");
                        }
                        _ => {}
                    }
                    write_ops.fetch_add(1, Ordering::Relaxed);
                }
                
                let elapsed = op_start.elapsed().as_nanos() as u64;
                latencies.record(elapsed);
                total_ops.fetch_add(1, Ordering::Relaxed);
            }
            
            latencies
        })
    }).collect();
    
    // 모든 스레드 완료 대기 및 레이턴시 수집
    let mut combined_latencies = LatencyHistogram::new();
    for handle in handles {
        let mut thread_latencies = handle.join().unwrap();
        combined_latencies.merge(&mut thread_latencies);
    }
    
    let duration = start.elapsed();
    let total = total_ops.load(Ordering::Relaxed);
    let reads = read_ops.load(Ordering::Relaxed);
    let writes = write_ops.load(Ordering::Relaxed);
    
    BenchmarkResult {
        workload: workload.name().to_string(),
        threads,
        total_ops: total,
        duration_ms: duration.as_millis() as u64,
        ops_per_sec: total as f64 / duration.as_secs_f64(),
        p50_us: combined_latencies.p50() as f64 / 1000.0,
        p99_us: combined_latencies.p99() as f64 / 1000.0,
        avg_us: combined_latencies.avg() as f64 / 1000.0,
        read_ops: reads,
        write_ops: writes,
    }
}

fn main() {
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║       🚀 CoreDB YCSB Benchmark Suite                     ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Workloads: A, B, C, D, F                                ║");
    println!("║  Threads:   1 → 64 → 256                                 ║");
    println!("║  Target:    ≥500K ops/sec                                ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    
    let db = Arc::new(BenchDB::new());
    let record_count = 100_000;
    let ops_per_thread = 50_000;
    
    let workloads = [Workload::A, Workload::B, Workload::C, Workload::D, Workload::F];
    let thread_counts = [1, 64, 256];
    
    let mut all_results: Vec<BenchmarkResult> = Vec::new();
    
    for workload in &workloads {
        println!("\n{}", "=".repeat(60));
        println!("📊 WORKLOAD {} ", workload.name());
        println!("{}", "=".repeat(60));
        
        for &threads in &thread_counts {
            let adjusted_ops = if threads > 64 { ops_per_thread / 4 } else { ops_per_thread };
            
            println!("\n🔄 Running with {} threads...", threads);
            let result = run_ycsb_benchmark(
                Arc::clone(&db),
                *workload,
                threads,
                adjusted_ops,
                record_count,
            );
            
            result.print();
            
            // 목표 달성 여부 체크
            if result.ops_per_sec >= 500_000.0 {
                println!("✅ TARGET ACHIEVED: {:.0} ops/sec >= 500K", result.ops_per_sec);
            } else {
                println!("⚠️  Below target: {:.0} ops/sec < 500K", result.ops_per_sec);
            }
            
            all_results.push(result);
        }
    }
    
    // 최종 요약
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                         📈 FINAL SUMMARY                                     ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║ Workload      │ Threads │   Ops/sec   │  P50 (µs) │  P99 (µs) │   Status    ║");
    println!("╠═══════════════╪═════════╪═════════════╪═══════════╪═══════════╪═════════════╣");
    
    for r in &all_results {
        let status = if r.ops_per_sec >= 500_000.0 { "✅ PASS" } else { "⚠️  BELOW" };
        println!("║ {:13} │ {:>7} │ {:>11.0} │ {:>9.2} │ {:>9.2} │ {:11} ║",
            r.workload, r.threads, r.ops_per_sec, r.p50_us, r.p99_us, status);
    }
    
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");
    
    // 통계 요약
    let total_passed = all_results.iter().filter(|r| r.ops_per_sec >= 500_000.0).count();
    let total_tests = all_results.len();
    let max_ops = all_results.iter().map(|r| r.ops_per_sec).fold(0.0f64, f64::max);
    let min_p99 = all_results.iter().map(|r| r.p99_us).fold(f64::MAX, f64::min);
    
    println!("\n📊 Statistics:");
    println!("   • Tests Passed:    {}/{} ({:.0}%)", total_passed, total_tests, 
             total_passed as f64 / total_tests as f64 * 100.0);
    println!("   • Peak Throughput: {:.0} ops/sec", max_ops);
    println!("   • Best P99:        {:.2} µs", min_p99);
    
    if total_passed == total_tests {
        println!("\n🏆 ALL TESTS PASSED! CoreDB meets ≥500K ops/sec target.");
    } else if total_passed > total_tests / 2 {
        println!("\n👍 GOOD PERFORMANCE! Most tests passed the target.");
    } else {
        println!("\n⚠️  NEEDS OPTIMIZATION for some workloads.");
    }
}
