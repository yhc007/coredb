# CoreDB 사용 가이드

CoreDB를 사용하는 다양한 방법을 안내합니다.

---

## 빠른 시작

### 1. 간단한 데모 (의존성 없음)

```bash
rustc simple_db.rs -o simple_db
./simple_db
```

**결과**: 키스페이스, 테이블, 데이터 생성 데모

---

### 2. Persistence 데모

```bash
rustc simple_persistent_db.rs -o simple_persistent_db

# 첫 실행 - 데이터 생성
./simple_persistent_db
# Output: Created 5 keys

# 두 번째 실행 - 데이터 로드
./simple_persistent_db
# Output: Loaded 5 keys, added 3 more = 8 keys total!
```

**결과**: 데이터 영속성 확인

---

### 3. 전체 프로젝트 빌드

```bash
# Debug 빌드
cargo build

# Release 빌드 (최적화)
cargo build --release
```

---

## 테스트 실행

### 단위 테스트
```bash
cargo test --lib
```

**결과**: 28개 테스트 중 25개 통과 (89%)

### 통합 테스트
```bash
cargo test --test integration_test
```

**결과**: 3개 테스트 모두 통과 (100%)

### 전체 테스트
```bash
cargo test
```

---

## 예제 실행

### Persistence 예제
```bash
cargo run --example persistence_example
```

**기능**:
- 데이터베이스 생성
- 키스페이스/테이블 생성
- 데이터 삽입 (5개 행)
- 디스크에 저장
- 통계 출력

---

## 성능 벤치마크

### 기본 벤치마크
```bash
rustc benchmark.rs -o benchmark
./benchmark
```

**테스트 항목**:
- Write: 673,665 ops/sec
- Read: 1,496,896 ops/sec
- Concurrent: 1,848,379 ops/sec
- Mixed: 807,739 ops/sec

### 스트레스 테스트
```bash
rustc stress_test.rs -o stress_test
./stress_test
```

**테스트 항목**:
- 대용량 데이터 (10KB/건)
- 고빈도 쓰기 (100,000건)
- 멀티스레드 (8스레드)
- 읽기 스트레스

### 극한 성능 테스트
```bash
rustc extreme_benchmark.rs -o extreme_benchmark
./extreme_benchmark
```

**테스트 항목**:
- 마이크로: 1,000,000건
- 하이퍼: 16스레드 동시성 (2,132,230 ops/sec!)
- 울트라: 혼합 작업

---

## Persistence 사용

### 기본 사용법

```rust
use coredb::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 1. 데이터베이스 생성
    let config = DatabaseConfig::default();
    let db = CoreDB::new(config).await?;
    
    // 2. 데이터 작업
    db.create_keyspace("myapp".to_string(), 1).await?;
    
    // 3. 저장
    db.save_to_disk().await?;
    
    Ok(())
}
```

### 데이터 복구

프로그램을 다시 시작하면 자동으로 스냅샷에서 복구됩니다.

생성되는 파일:
- `./data/db_snapshot.txt` - 데이터베이스 스냅샷
- `./data/wal.log` - Write-Ahead Log

---

## 프로그래밍 API

### 키스페이스 생성
```rust
db.create_keyspace("demo".to_string(), 1).await?;
```

### 테이블 생성
```rust
let schema = TableSchema::new(
    "users".to_string(),
    "demo".to_string(),
    vec![/* partition key */],
    vec![/* clustering key */],
    vec![/* regular columns */],
    vec![/* static columns */],
);

db.create_table("demo".to_string(), "users".to_string(), schema).await?;
```

### 데이터 삽입
```rust
let row = Row {
    partition_key: PartitionKey { /* ... */ },
    clustering_key: None,
    cells: HashMap::new(),
    timestamp: chrono::Utc::now().timestamp_micros(),
};

db.insert_row("demo", "users", row).await?;
```

### 데이터 조회
```rust
let row = db.get_row(
    "demo",
    "users",
    &partition_key,
    &None
).await?;
```

### 통계 확인
```rust
let stats = db.get_stats().await;
println!("Keyspaces: {}", stats.keyspace_count);
println!("Tables: {}", stats.table_count);
```

---

## 문제 해결

### 컴파일 에러
```bash
# 의존성 업데이트
cargo update

# 클린 빌드
cargo clean && cargo build
```

### 테스트 실패
```bash
# 특정 테스트만 실행
cargo test test_name

# 백트레이스 활성화
RUST_BACKTRACE=1 cargo test
```

### 성능 문제
```bash
# Release 모드로 빌드
cargo build --release

# 벤치마크 실행
cargo run --release --example persistence_example
```

---

## 추가 자료

- **README.md**: 프로젝트 개요
- **PERSISTENCE_GUIDE.md**: 영속성 상세 가이드
- **Performance_Test.md**: 성능 벤치마크 결과
- **TEST_RESULTS.md**: 테스트 결과 상세
- **FINAL_REPORT.md**: 최종 프로젝트 보고서

---

## 라이브 예제

### 완전한 예제 프로그램

```rust
use coredb::*;
use coredb::schema::*;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // 설정
    let config = DatabaseConfig {
        data_directory: PathBuf::from("./mydb"),
        ..Default::default()
    };
    
    // 데이터베이스 생성
    let db = CoreDB::new(config).await?;
    
    // 키스페이스 생성
    db.create_keyspace("shop".to_string(), 1).await?;
    
    // 테이블 스키마
    let schema = TableSchema::new(
        "products".to_string(),
        "shop".to_string(),
        vec![ColumnDefinition {
            name: "product_id".to_string(),
            data_type: CassandraDataType::Int,
            is_static: false,
        }],
        vec![],
        vec![
            ColumnDefinition {
                name: "name".to_string(),
                data_type: CassandraDataType::Text,
                is_static: false,
            },
            ColumnDefinition {
                name: "price".to_string(),
                data_type: CassandraDataType::Int,
                is_static: false,
            },
        ],
        vec![],
    );
    
    // 테이블 생성
    db.create_table("shop".to_string(), "products".to_string(), schema).await?;
    
    // 데이터 삽입
    for i in 1..=10 {
        let row = Row {
            partition_key: PartitionKey {
                components: vec![CassandraValue::Int(i)],
            },
            clustering_key: None,
            cells: {
                let mut cells = std::collections::HashMap::new();
                cells.insert("name".to_string(), Cell {
                    value: CassandraValue::Text(format!("Product {}", i)),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells.insert("price".to_string(), Cell {
                    value: CassandraValue::Int(i * 100),
                    timestamp: chrono::Utc::now().timestamp_micros(),
                    ttl: None,
                    is_deleted: false,
                });
                cells
            },
            timestamp: chrono::Utc::now().timestamp_micros(),
        };
        
        db.insert_row("shop", "products", row).await?;
    }
    
    // 저장
    db.save_to_disk().await?;
    
    // 통계
    let stats = db.get_stats().await;
    println!("Database created with {} keyspaces and {} tables", 
             stats.keyspace_count, stats.table_count);
    
    Ok(())
}
```

---

**CoreDB를 즐겁게 사용하세요!** 🚀

